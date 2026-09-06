//! 文件名重生：LLM + 元数据 → 人类可读文件名。
//!
//! 将 `qB6f3a91EE.mp4` 这类哈希串/压制组命名改写为 `三体 (2023) S01E01.mp4`。
//! 改名操作以 Move 形式进入计划系统（src=旧路径, dst=旧目录/新名），可预览与回滚。
//! 内容切片仅在 tier=ContentSlice 时读入 prompt；否则仅用文件名+元数据。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::cost::Usage;
use crate::domain::FileEntry;
use crate::llm::{ChatOptions, LlmClient, Message};
use crate::metadata::{extract as extract_metadata, FileMetadata};
use crate::permission::PermissionConfig;
use crate::tools::organize::resolve_collision;
use crate::trajectory::{EventKind, TrajectoryLogger};
use crate::Result;

/// 一条重命名建议。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameSuggestion {
    pub path: PathBuf,
    pub old_name: String,
    pub new_name: String,
    pub new_path: PathBuf,
    pub reason: String,
}

/// 重命名管线结果。
pub struct RenameResult {
    pub renames: Vec<RenameSuggestion>,
    pub skipped: Vec<PathBuf>,
    pub stats: RenameStats,
}

#[derive(Debug, Clone, Default)]
pub struct RenameStats {
    pub total: usize,
    pub renamed: usize,
    pub skipped: usize,
    pub unchanged: usize,
    pub usage: Usage,
}

/// 重命名管线输入。
pub struct RenameRequest<'a> {
    pub files: &'a [FileEntry],
    pub permissions: &'a PermissionConfig,
    pub client: &'a dyn LlmClient,
    pub cost: &'a crate::cost::CostTracker,
    pub model: &'a str,
    pub temperature: f32,
    pub batch_id: uuid::Uuid,
    pub trajectory: &'a TrajectoryLogger,
    pub cancel: &'a CancellationToken,
    /// few-shot 示例：旧名 → 新名。
    pub examples: &'a [(String, String)],
}

/// 执行文件名重生：为每个文件请求 LLM 生成人类可读名，构造重命名对。
pub async fn rename_with_llm(req: &RenameRequest<'_>) -> Result<RenameResult> {
    if req.files.is_empty() {
        return Ok(RenameResult {
            renames: vec![],
            skipped: vec![],
            stats: RenameStats::default(),
        });
    }

    // 收集每个文件的可用信息（受权限控制）
    let mut file_infos: Vec<(&FileEntry, FileMetadata, Option<String>)> = Vec::new();
    let mut denied = Vec::new();
    for entry in req.files.iter() {
        let tier = req
            .permissions
            .tier_for(entry.extension.as_deref(), entry.size);
        if tier == crate::permission::AccessTier::None {
            denied.push(entry.path.clone());
            continue;
        }
        let meta = match extract_metadata(&entry.path) {
            Ok(m) => m,
            Err(_) => FileMetadata {
                file_type: "unknown".into(),
                image_dimensions: None,
                duration_secs: None,
            },
        };

        // 若权限允许且为文本类，读取内容切片辅助识别
        let tier = req
            .permissions
            .tier_for(entry.extension.as_deref(), entry.size);
        let content = if tier == crate::permission::AccessTier::ContentSlice {
            let limit = req.permissions.clamp_slice(2048);
            read_text_slice(&entry.path, limit)
        } else {
            None
        };

        file_infos.push((entry, meta, content));
    }
    if file_infos.is_empty() {
        return Ok(RenameResult {
            stats: RenameStats {
                total: req.files.len(),
                skipped: denied.len(),
                ..Default::default()
            },
            renames: vec![],
            skipped: denied,
        });
    }

    // 构造 LLM 请求
    let system = build_rename_prompt(req.examples);
    let user = build_user_msg(&file_infos);

    let options = ChatOptions {
        model: req.model.to_string(),
        temperature: req.temperature,
        max_tokens: None,
        thinking_mode: false,
    };
    let messages = vec![Message::system(&system), Message::user(&user)];

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

    // 解析 LLM 返回的重命名建议
    let suggestions = parse_rename_response(&resp.message.content);

    let mut renames = Vec::new();
    let mut skipped = denied;
    let mut unchanged = 0usize;

    for (entry, _meta, _content) in &file_infos {
        let path_str = entry.path.to_string_lossy().to_string();
        if let Some(s) = suggestions.iter().find(|s| s.path == path_str) {
            // 保留扩展名
            let ext = entry.extension.as_deref().unwrap_or("");
            let new_name = if ext.is_empty() || s.new_name.ends_with(&format!(".{ext}")) {
                s.new_name.clone()
            } else {
                format!("{}.{}", s.new_name, ext)
            };

            if new_name == entry.basename() {
                unchanged += 1;
                continue;
            }

            let parent = entry.path.parent().unwrap_or(std::path::Path::new("."));
            if crate::workflow::valid_name(&new_name).is_err() {
                skipped.push(entry.path.clone());
                continue;
            }
            let new_path_raw = parent.join(&new_name);
            let new_path = resolve_collision(&new_path_raw);

            renames.push(RenameSuggestion {
                path: entry.path.clone(),
                old_name: entry.basename(),
                new_name: new_path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or(new_name),
                new_path,
                reason: s.reason.clone(),
            });
        } else {
            skipped.push(entry.path.clone());
        }
    }

    let stats = RenameStats {
        total: req.files.len(),
        renamed: renames.len(),
        skipped: skipped.len(),
        unchanged,
        usage: resp.usage,
    };

    Ok(RenameResult {
        renames,
        skipped,
        stats,
    })
}

fn read_text_slice(path: &std::path::Path, limit: usize) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; limit];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn build_rename_prompt(examples: &[(String, String)]) -> String {
    let mut ex = String::new();
    for (old, new) in examples {
        ex.push_str(&format!("  \"{old}\" → \"{new}\"\n"));
    }
    format!(
        "你是文件重命名助手。根据文件名、元数据和（如有）内容切片，为文件生成人类可读的文件名。\n\
         规则:\n\
         1. 电影/番剧格式: \"名称 (年份) S01E01\" 或 \"名称 (年份)\"\n\
         2. 文档格式: \"描述_日期\" 或 \"描述\"\n\
         3. 若无法识别内容，返回与原名相同\n\
         4. 不要包含文件扩展名（系统会自动追加）\n\
         5. 禁止使用特殊字符 / \\ : * ? \" < > |\n\n\
         示例:\n{ex}\n\
         请以JSON数组回复: [{{\"path\":\"绝对路径\",\"new_name\":\"新名\",\"reason\":\"简述原因\"}}, ...]\n\
         每个文件都必须出现。只返回JSON。"
    )
}

fn build_user_msg(file_infos: &[(&FileEntry, FileMetadata, Option<String>)]) -> String {
    let mut lines = String::new();
    for (i, (entry, meta, content)) in file_infos.iter().enumerate() {
        let dims = meta
            .image_dimensions
            .map(|(w, h)| format!("{w}x{h}"))
            .unwrap_or_default();
        let content_preview = content
            .as_deref()
            .map(|c| format!("内容片段: {}", c.chars().take(100).collect::<String>()))
            .unwrap_or_default();
        lines.push_str(&format!(
            "{}. {} ({}MB, type:{}, {}, ext:{}) {}\n",
            i + 1,
            entry.path.to_string_lossy(),
            entry.size / 1_000_000,
            meta.file_type,
            dims,
            entry.extension.as_deref().unwrap_or("(无)"),
            content_preview,
        ));
    }
    format!("请重命名以下文件:\n{lines}")
}

#[derive(Debug, Deserialize)]
struct RawRename {
    path: String,
    new_name: String,
    #[serde(default)]
    reason: String,
}

fn parse_rename_response(content: &str) -> Vec<RawRename> {
    let start = content.find('[');
    let end = content.rfind(']');
    match (start, end) {
        (Some(s), Some(e)) if e > s => {
            let json_str = &content[s..=e];
            serde_json::from_str::<Vec<RawRename>>(json_str).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// 从 RenameResult 提取 (old_path, new_path) 移动对，供 PlanOrchestrator 使用。
pub fn moves_from_rename_result(result: &RenameResult) -> Vec<(PathBuf, PathBuf)> {
    result
        .renames
        .iter()
        .map(|r| (r.path.clone(), r.new_path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rename_json() {
        let resp = r#"结果如下：
[{"path":"/a/b.mp4","new_name":"三体 (2023) S01E01","reason":"文件名识别为三体动画"}]"#;
        let arr = parse_rename_response(resp);
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].new_name, "三体 (2023) S01E01");
        assert_eq!(arr[0].reason, "文件名识别为三体动画");
    }

    #[test]
    fn parse_empty_response() {
        assert!(parse_rename_response("无法识别").is_empty());
    }

    #[test]
    fn prompt_has_examples() {
        let prompt = build_rename_prompt(&[("qB6f3a91EE.mp4".into(), "三体 (2023) S01E01".into())]);
        assert!(prompt.contains("qB6f3a91EE.mp4"));
        assert!(prompt.contains("三体 (2023) S01E01"));
    }
}
