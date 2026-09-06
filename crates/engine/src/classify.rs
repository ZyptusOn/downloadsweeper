//! 混合分类管线：规则优先 → 缓存命中 → LLM 兜底。
//!
//! 设计要点：
//! - 规则（扩展名匹配）零成本解决大部分文件
//! - 内容指纹缓存：文件未变不重复调用 LLM
//! - LLM 兜底：仅对规则和缓存都未命中的歧义文件调用，带 few-shot 示例
//! - "未分类"长尾桶：LLM 不确定的文件归入此桶，禁止强行归类

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::cost::Usage;
use crate::domain::{BatchId, FileEntry};
use crate::fingerprint::{cached_entry, fingerprint, ClassificationCache};
use crate::llm::{ChatOptions, LlmClient, Message};
use crate::trajectory::{EventKind, TrajectoryLogger};
use crate::Result;

/// 一个抽象分类（供 LLM 使用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryDef {
    pub name: String,
    pub subfolder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 规则类型：simple / complex（由目录树节点决定）。
    #[serde(default)]
    pub rule_type: crate::tree::RuleType,
    /// 是否一级目录（根的直属子节点）。
    #[serde(default)]
    pub is_first_level: bool,
    /// 用户备注（复杂规则节点语义提示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 该分类绑定的 few-shot 引用（文件名或路径）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub few_shot_refs: Vec<String>,
}

/// few-shot 示例：文件名（或锚定的具体文件路径） → 类别名。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FewShotExample {
    pub filename: String,
    pub category: String,
    /// 可选：锚定的具体文件路径（文件锚点）。文件被移动后可通过
    /// 文件名 + 内容指纹在扫描结果中重定位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl FewShotExample {
    pub fn new(filename: impl Into<String>, category: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            category: category.into(),
            path: None,
        }
    }
}

/// few-shot 锚点解析结果：把示例解析为"仅文件名"或"文件名+内容锚点"。
#[derive(Debug, Clone)]
pub struct ResolvedAnchor {
    pub filename: String,
    pub category: String,
    /// 权限允许且文件可读时附内容切片。
    pub content: Option<String>,
    /// 锚定的路径（若被解析/重定位）。
    pub resolved_path: Option<std::path::PathBuf>,
}

/// 解析一个 few-shot 为可用锚点：
/// 1) 若示例有 path 且文件存在 → 按权限层决定读内容切片；
/// 2) path 存在但文件被移动（同名+内容指纹命中）→ 在扫描结果中重定位后读；
/// 3) 无 path → 仅文件名（旧行为）。
pub fn resolve_anchor(
    example: &FewShotExample,
    permissions: &crate::permission::PermissionConfig,
    scan_files: &[FileEntry],
) -> ResolvedAnchor {
    use crate::permission::AccessTier;
    let ext_for = |p: &std::path::Path| {
        p.extension()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
    };
    let size_of = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);

    // 收集候选路径：原始 path → 若不存在，按 basename 在扫描结果中追踪
    let original = example.path.as_ref().map(std::path::PathBuf::from);
    let mut resolved_path: Option<std::path::PathBuf> = None;

    if let Some(p) = &original {
        if p.exists() {
            resolved_path = Some(p.clone());
        } else {
            // 文件可能被移动：按 basename 在扫描结果里找
            if let Some(found) = scan_files.iter().find(|f| f.basename() == example.filename) {
                resolved_path = Some(found.path.clone());
            }
        }
    } else {
        // 无 path 时若扫描里有此文件名，也可作为锚点（仅当权限允许内容）
        if let Some(found) = scan_files.iter().find(|f| f.basename() == example.filename) {
            resolved_path = Some(found.path.clone());
        }
    }

    let content = match &resolved_path {
        Some(p) => {
            let tier = permissions.tier_for(ext_for(p).as_deref(), size_of(p));
            if tier == AccessTier::ContentSlice {
                read_text_slice(p, 1024).ok()
            } else {
                None
            }
        }
        None => None,
    };

    ResolvedAnchor {
        filename: example.filename.clone(),
        category: example.category.clone(),
        content,
        resolved_path,
    }
}

/// 分类结果。
#[derive(Debug, Clone)]
pub struct ClassifyResult {
    /// (src, dst) 移动对。
    pub moves: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    /// 未能分类的文件（留在原位）。
    pub uncategorized: Vec<std::path::PathBuf>,
    pub stats: ClassifyStats,
}

#[derive(Debug, Clone, Default)]
pub struct ClassifyStats {
    pub total: usize,
    pub from_rule: usize,
    pub from_cache: usize,
    pub from_llm: usize,
    pub uncategorized: usize,
    pub usage: Usage,
}

/// 混合分类管线的全部输入。
pub struct ClassifyRequest<'a> {
    pub files: &'a [FileEntry],
    /// 扩展名规则：(扩展名列表, 子目录)，零成本。
    pub rules: &'a [(Vec<String>, String)],
    /// LLM 分类类别定义。
    pub categories: &'a [CategoryDef],
    /// few-shot 示例。
    pub examples: &'a [FewShotExample],
    /// 整理目标根目录。
    pub target_dir: &'a Path,
    /// 隐私权限（决定 few-shot 引用能否附带内容切片）。
    pub permissions: &'a crate::permission::PermissionConfig,
    pub cache: &'a ClassificationCache,
    pub client: &'a dyn LlmClient,
    pub cost: &'a crate::cost::CostTracker,
    pub model: &'a str,
    pub temperature: f32,
    /// 是否启用多模态（模型支持 + 用户开启）。图片推理时缩略图入 prompt。
    pub multimodal: bool,
    pub batch_id: BatchId,
    pub trajectory: &'a TrajectoryLogger,
    pub cancel: &'a CancellationToken,
}

/// 执行混合分类：规则 → 缓存 → LLM 兜底。
pub async fn classify_hybrid(req: &ClassifyRequest<'_>) -> Result<ClassifyResult> {
    let mut moves = Vec::new();
    let mut uncategorized = Vec::new();
    let mut from_rule = 0usize;
    let mut from_cache = 0usize;
    let mut from_llm = 0usize;
    let mut total_usage = Usage::default();

    // 阶段 1: 规则匹配（零成本）
    let mut need_llm: Vec<&FileEntry> = Vec::new();
    for entry in req.files {
        let ext = entry.extension.as_deref().unwrap_or("");
        if let Some((_, sub)) = req
            .rules
            .iter()
            .find(|(exts, _)| exts.iter().any(|x| x.eq_ignore_ascii_case(ext)))
        {
            let dst = req.target_dir.join(sub).join(entry.basename());
            moves.push((entry.path.clone(), dst));
            from_rule += 1;
        } else if req
            .permissions
            .tier_for(entry.extension.as_deref(), entry.size)
            == crate::permission::AccessTier::None
        {
            uncategorized.push(entry.path.clone());
        } else {
            need_llm.push(entry);
        }
    }

    // 阶段 2: 缓存命中
    let mut need_llm_after_cache: Vec<&FileEntry> = Vec::new();
    for entry in &need_llm {
        match fingerprint(&entry.path) {
            Ok(fp) => {
                if let Some(cached) = req.cache.get(&fp) {
                    let dst = req
                        .target_dir
                        .join(&cached.subfolder)
                        .join(entry.basename());
                    moves.push((entry.path.clone(), dst));
                    from_cache += 1;
                } else {
                    need_llm_after_cache.push(entry);
                }
            }
            Err(_) => {
                // 指纹计算失败（如权限不足），归入未分类
                uncategorized.push(entry.path.clone());
            }
        }
    }

    // 阶段 3: LLM 兜底
    if !need_llm_after_cache.is_empty() {
        let (llm_moves, llm_uncategorized, usage) =
            classify_with_llm(req, &need_llm_after_cache).await?;
        total_usage.add(&usage);

        // 缓存 LLM 分类结果
        for (entry, category, subfolder) in &llm_moves {
            if let Ok(fp) = fingerprint(&entry.path) {
                req.cache.put(fp, cached_entry(category, subfolder, "llm"));
            }
            let dst = req.target_dir.join(subfolder).join(entry.basename());
            moves.push((entry.path.clone(), dst));
            from_llm += 1;
        }
        for path in llm_uncategorized {
            uncategorized.push(path);
        }
    }

    req.cache.save()?;
    req.trajectory.log(
        EventKind::ScanCompleted,
        Some(req.batch_id),
        json!({
            "total": req.files.len(),
            "from_rule": from_rule,
            "from_cache": from_cache,
            "from_llm": from_llm,
            "uncategorized": uncategorized.len(),
            "usage": { "prompt_tokens": total_usage.prompt_tokens, "completion_tokens": total_usage.completion_tokens },
        }),
    )?;

    let stats = ClassifyStats {
        total: req.files.len(),
        from_rule,
        from_cache,
        from_llm,
        uncategorized: uncategorized.len(),
        usage: total_usage,
    };

    Ok(ClassifyResult {
        moves,
        uncategorized,
        stats,
    })
}

/// LLM 分类子步骤：构造 prompt → 调用 → 解析 JSON 响应。
/// 返回 (分类结果条目, 未分类路径, token 用量)。
/// 每条条目为 (FileEntry 引用, 类别名, 子目录名)。
async fn classify_with_llm<'a>(
    req: &'a ClassifyRequest<'a>,
    files: &'a [&'a FileEntry],
) -> Result<(
    Vec<(&'a FileEntry, String, String)>,
    Vec<std::path::PathBuf>,
    Usage,
)> {
    let system = build_system_prompt(req.categories, req.examples, req.permissions, req.files);
    let user = build_user_prompt(files);

    let options = ChatOptions {
        model: req.model.to_string(),
        temperature: req.temperature,
        max_tokens: None,
        thinking_mode: false,
    };

    // 多模态：权限为 image（或更高）的图片文件，附缩略图进 user 消息
    let user_msg = if req.multimodal {
        let images: Vec<crate::llm::ImageData> = files
            .iter()
            .filter(|f| is_image_ext(f.extension.as_deref()))
            .filter(|f| {
                req.permissions.tier_for(f.extension.as_deref(), f.size)
                    >= crate::permission::AccessTier::Image
            })
            .take(6)
            .filter_map(|f| crate::metadata::image_thumbnail(&f.path, 512))
            .collect();
        if images.is_empty() {
            Message::user(&user)
        } else {
            Message::user_with_images(&user, images)
        }
    } else {
        Message::user(&user)
    };

    let messages = vec![Message::system(&system), user_msg];

    req.trajectory.log(
        EventKind::LlmRequest,
        Some(req.batch_id),
        crate::trajectory::llm_request_detail(req.model, messages.len(), 0),
    )?;

    let resp = req
        .client
        .chat(&messages, &[], &options, req.cancel)
        .await?;

    req.trajectory.log(
        EventKind::LlmResponse,
        Some(req.batch_id),
        crate::trajectory::llm_response_detail(
            resp.usage.prompt_tokens,
            resp.usage.completion_tokens,
            &[],
        ),
    )?;

    if let Err(e) = req.cost.add(&resp.usage) {
        req.trajectory.log(
            EventKind::BudgetExceeded,
            Some(req.batch_id),
            json!({ "error": e.to_string() }),
        )?;
        return Err(e.into());
    }

    // 解析 LLM 返回的 JSON 数组
    let classified = parse_llm_response(&resp.message.content);

    // 把 LLM 结果映射回文件条目
    let mut llm_moves = Vec::new();
    let mut uncategorized = Vec::new();
    let mut matched_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    for item in &classified {
        let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let category = item.get("category").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() || category.is_empty() {
            continue;
        }

        // 查找对应的 FileEntry
        if let Some(entry) = files.iter().find(|f| f.path.to_string_lossy() == path) {
            matched_paths.insert(path.to_string());

            if category == "未分类" || category.eq_ignore_ascii_case("uncategorized") {
                // LLM 明确表示无法分类 → 长尾桶
                uncategorized.push(entry.path.clone());
                continue;
            }
            if let Some(cat) = req.categories.iter().find(|c| c.name == category) {
                llm_moves.push((*entry, category.to_string(), cat.subfolder.clone()));
            } else {
                // LLM 返回了未知类别名 → 未分类
                uncategorized.push(entry.path.clone());
            }
        }
    }

    // LLM 未返回的文件 → 未分类
    for entry in files {
        if !matched_paths.contains(&*entry.path.to_string_lossy()) {
            uncategorized.push(entry.path.clone());
        }
    }

    Ok((llm_moves, uncategorized, resp.usage))
}

fn build_system_prompt(
    categories: &[CategoryDef],
    examples: &[FewShotExample],
    permissions: &crate::permission::PermissionConfig,
    scan_files: &[FileEntry],
) -> String {
    let mut cats = String::new();
    for (i, c) in categories.iter().enumerate() {
        let desc = c.description.as_deref().unwrap_or("");
        cats.push_str(&format!("{}. {}（子目录: {}）", i + 1, c.name, c.subfolder));
        if !desc.is_empty() {
            cats.push_str(&format!(" — {desc}"));
        }
        if let Some(note) = &c.note {
            cats.push_str(&format!("｜备注: {note}"));
        }
        cats.push('\n');
    }
    // 保证有"未分类"类别
    if !categories.iter().any(|c| c.name == "未分类") {
        cats.push_str(&format!(
            "{}. 未分类（子目录: 未分类）— 无法确定时使用\n",
            categories.len() + 1
        ));
    }

    // 统一：全局示例 + 分类绑定引用 → 全部走 resolve_anchor（权限感知 + 移动追踪）
    let mut ex = String::new();
    let mut seen = std::collections::HashSet::new();

    // ① 全局 few_shot 示例（被分类绑定 refs 覆盖的同名项跳过）
    for e in examples {
        if let Some(cat) = categories.iter().find(|c| c.name == e.category) {
            if !cat.few_shot_refs.is_empty() && !cat.few_shot_refs.iter().any(|r| r == &e.filename)
            {
                continue;
            }
        }
        let anchor = resolve_anchor(e, permissions, scan_files);
        seen.insert(anchor.filename.clone());
        match &anchor.content {
            Some(text) => ex.push_str(&format!(
                "  \"{}\" → {}（内容锚点: {}…）\n",
                anchor.filename,
                anchor.category,
                text.replace('\n', " ")
                    .chars()
                    .take(120)
                    .collect::<String>()
            )),
            None => ex.push_str(&format!(
                "  \"{}\" → {}（权限: 仅文件名）\n",
                anchor.filename, anchor.category
            )),
        }
    }

    // ② 分类绑定的 few_shot_refs（文件名/路径引用）→ resolve_anchor 重解析
    for c in categories {
        for r in &c.few_shot_refs {
            if seen.contains(r) {
                continue;
            }
            let ref_example = FewShotExample {
                filename: r.clone(),
                category: c.name.clone(),
                path: Some(r.clone()),
            };
            let anchor = resolve_anchor(&ref_example, permissions, scan_files);
            seen.insert(anchor.filename.clone());
            match &anchor.content {
                Some(text) => ex.push_str(&format!(
                    "  \"{}\" → {}（内容锚点: {}…）\n",
                    anchor.filename,
                    anchor.category,
                    text.replace('\n', " ")
                        .chars()
                        .take(120)
                        .collect::<String>()
                )),
                None => ex.push_str(&format!(
                    "  \"{}\" → {}（权限: 仅文件名）\n",
                    anchor.filename, anchor.category
                )),
            }
        }
    }

    format!(
        "你是文件分类助手。根据文件名和元数据，将文件分类到以下类别之一。\n\
         如果无法确定，归类为\"未分类\"。禁止强行归类——不确定的宁可归入\"未分类\"。\n\n\
         类别:\n{cats}\n\
         示例（重要，体现用户意图）:\n{ex}\n\
         请以JSON数组回复: [{{\"path\":\"绝对路径\",\"category\":\"类别名\"}}, ...]\n\
         每个文件都必须出现在结果中。只返回JSON，不要其他文字。"
    )
}

fn read_text_slice(path: &std::path::Path, limit: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; limit];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 常见图片扩展名（多模态候选）。
fn is_image_ext(ext: Option<&str>) -> bool {
    matches!(
        ext.unwrap_or("").to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "heic" | "avif"
    )
}

fn build_user_prompt(files: &[&FileEntry]) -> String {
    let mut lines = String::new();
    for (i, entry) in files.iter().enumerate() {
        lines.push_str(&format!(
            "{}. {} ({}MB, ext:{})\n",
            i + 1,
            entry.path.to_string_lossy(),
            entry.size / 1_000_000,
            entry.extension.as_deref().unwrap_or("(无)"),
        ));
    }
    format!("请分类以下文件:\n{lines}")
}

/// 从 LLM 响应文本中提取 JSON 数组。
fn parse_llm_response(content: &str) -> Vec<Value> {
    let start = content.find('[');
    let end = content.rfind(']');
    match (start, end) {
        (Some(s), Some(e)) if e > s => {
            let json_str = &content[s..=e];
            serde_json::from_str::<Vec<Value>>(json_str).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// 给 CLI 用的辅助：从 classify 结果构造 (src, dst) 移动对列表。
pub fn moves_from_classify_result(
    result: &ClassifyResult,
) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
    result.moves.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json_array() {
        let resp = r#"好的，这是分类结果：
[{"path":"/a/b.mp4","category":"电影"},{"path":"/c/d.mkv","category":"未分类"}]"#;
        let arr = parse_llm_response(resp);
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["category"], "电影");
        assert_eq!(arr[1]["category"], "未分类");
    }

    #[test]
    fn parse_no_json_returns_empty() {
        let resp = "我无法分类这些文件。";
        assert!(parse_llm_response(resp).is_empty());
    }

    #[test]
    fn system_prompt_includes_uncategorized() {
        let cats = vec![CategoryDef {
            name: "电影".into(),
            subfolder: "电影".into(),
            description: Some("长片".into()),
            rule_type: crate::tree::RuleType::Simple,
            is_first_level: true,
            note: None,
            few_shot_refs: vec![],
        }];
        let examples = vec![FewShotExample::new("a.mkv", "电影")];
        let prompt = build_system_prompt(
            &cats,
            &examples,
            &crate::permission::PermissionConfig::default(),
            &[],
        );
        assert!(prompt.contains("未分类"));
        assert!(prompt.contains("电影"));
        assert!(prompt.contains("a.mkv"));
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;
    use crate::permission::{AccessTier, PermissionConfig, PermissionRule};

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ds_anchor_{name}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn perms_with(ext: &str, tier: AccessTier) -> PermissionConfig {
        PermissionConfig {
            rules: vec![PermissionRule {
                category: None,
                extensions: vec![ext.into()],
                min_bytes: None,
                max_bytes: None,
                tier,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn anchor_with_content_when_permitted() {
        let dir = tmp("c");
        let f = dir.join("sample.txt");
        std::fs::write(&f, "这是内容锚点示例正文").unwrap();
        let ex = FewShotExample {
            filename: "sample.txt".into(),
            category: "文档".into(),
            path: Some(f.to_string_lossy().into_owned()),
        };
        let a = resolve_anchor(&ex, &perms_with("txt", AccessTier::ContentSlice), &[]);
        assert!(a.content.is_some(), "ContentSlice 权限下应读内容");
        assert!(a.content.unwrap().contains("内容锚点示例"));
        assert_eq!(a.resolved_path, Some(f));
    }

    #[test]
    fn anchor_denied_content_by_permission() {
        let dir = tmp("d");
        let f = dir.join("secrets.xlsx");
        std::fs::write(&f, "maybe sensitive").unwrap();
        let ex = FewShotExample {
            filename: "secrets.xlsx".into(),
            category: "表格".into(),
            path: Some(f.to_string_lossy().into_owned()),
        };
        let a = resolve_anchor(&ex, &perms_with("xlsx", AccessTier::None), &[]);
        assert!(a.content.is_none(), "None 权限下不得读内容");
        assert_eq!(a.resolved_path, Some(f)); // 路径仍被锚定（仅不读内容）
    }

    #[test]
    fn anchor_relocates_moved_file_by_basename() {
        let dir = tmp("m");
        let old = dir.join("old_sub");
        std::fs::create_dir_all(&old).unwrap();
        let f = old.join("report.txt");
        std::fs::write(&f, "summer report").unwrap();
        // 模拟移动：示例 path 指向已不存在的旧位置，扫描结果含新位置同名文件
        let ex = FewShotExample {
            filename: "report.txt".into(),
            category: "文档".into(),
            path: Some(old.join("gone/report.txt").to_string_lossy().into_owned()),
        };
        let moved = dir.join("new_sub/report.txt");
        std::fs::create_dir_all(dir.join("new_sub")).unwrap();
        std::fs::rename(&f, &moved).unwrap();
        let scan_file = FileEntry::from_path(moved.clone()).unwrap();
        let a = resolve_anchor(
            &ex,
            &perms_with("txt", AccessTier::ContentSlice),
            &[scan_file],
        );
        assert_eq!(a.resolved_path, Some(moved), "应按 basename 重定位到新位置");
        assert!(a.content.is_some(), "重定位后若权限允许应可读内容");
    }

    #[test]
    fn plain_filename_example_stays_filename_only() {
        let ex = FewShotExample::new("随手一记.txt", "笔记");
        let a = resolve_anchor(&ex, &PermissionConfig::default(), &[]);
        assert_eq!(a.filename, "随手一记.txt");
        assert!(a.resolved_path.is_none());
        assert!(a.content.is_none());
    }
}
