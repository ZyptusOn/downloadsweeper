//! 目录树模板：分层目录结构，分类任务为"文件 → 树节点"。
//!
//! 用户可在 config.toml 中定义树模板，或用内置默认树。
//! 树节点最终展平为 CategoryDef（subfolder 为完整路径，如 "媒体/电影/国产"）。
//!
//! 规则类型设计：
//! - 根的第一级子节点（一级目录）**必须** Simple（简单规则：按扩展名归类），
//!   从初期就区分开不同扩展名类型——见 `validate()` 硬校验。
//! - 一级之下的节点允许 Simple（按扩展名）或 Complex（文件名/内容/备注/few-shot 决定）。
//! - 一级节点的 Complex 违规会在保存/加载时提示以指导用户，但不阻止程序跑通旧数据。

use serde::{Deserialize, Serialize};

use crate::classify::CategoryDef;

/// 目录节点的规则类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleType {
    /// 简单规则：仅按扩展名命中即归入（一级目录强制）。
    #[default]
    Simple,
    /// 复杂规则：文件名/文件内容/备注/few-shot 相关。
    Complex,
}

impl RuleType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RuleType::Simple => "simple",
            RuleType::Complex => "complex",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            RuleType::Simple => "简单规则（扩展名）",
            RuleType::Complex => "复杂规则（文件名/内容/备注）",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TreeNode {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 规则类型：Simple（按扩展名）或 Complex（AI/备注/few-shot 参与）。
    /// 默认 Simple。
    #[serde(default)]
    pub rule_type: RuleType,
    /// 简单规则命中的扩展名列表（rule_type=Simple 时生效）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// 用户文本备注（复杂规则节点用于描述"里面该装什么"；不进文件夹名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 该节点的 few-shot 引用（文件名或路径）；运行时按权限解析为 文件名 或 内容锚点。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub few_shot_refs: Vec<String>,
    #[serde(default)]
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    /// 递归展平为 CategoryDef 列表（含自身和所有子节点）。
    /// `parent_path` 是父节点的完整路径（不含自身名）。
    /// `is_first_level` 标记当前节点是否处于一级（根的直属子节点）。
    pub fn to_categories(&self, parent_path: &str, is_first_level: bool) -> Vec<CategoryDef> {
        let full_path = if parent_path.is_empty() {
            self.name.clone()
        } else {
            format!("{parent_path}/{}", self.name)
        };
        let mut cats = vec![CategoryDef {
            name: self.name.clone(),
            subfolder: full_path.clone(),
            description: self.description.clone(),
            rule_type: self.rule_type,
            is_first_level,
            note: self.note.clone(),
            few_shot_refs: self.few_shot_refs.clone(),
        }];
        for child in &self.children {
            cats.extend(child.to_categories(&full_path, false));
        }
        cats
    }
}

/// 一份完整的目录树模板。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TreeTemplate {
    pub roots: Vec<TreeNode>,
    /// 孤儿节点：已从有效树断开、暂未连接的节点。
    /// 仍可被拖回连接；在连接回有效节点之前不参与分类与保存的整理。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<TreeNode>,
}

impl TreeTemplate {
    /// 展平为 CategoryDef 列表（自动追加"未分类"桶；仅有效 roots，孤儿不参与）。
    pub fn to_categories(&self) -> Vec<CategoryDef> {
        let mut cats = Vec::new();
        for root in &self.roots {
            cats.extend(root.to_categories("", false));
        }
        // 强制追加未分类桶（若用户未定义）
        if !cats.iter().any(|c| c.name == "未分类") {
            cats.push(CategoryDef {
                name: "未分类".into(),
                subfolder: "未分类".into(),
                description: Some("无法确定类型".into()),
                rule_type: RuleType::Simple,
                is_first_level: true,
                note: None,
                few_shot_refs: vec![],
            });
        }
        cats
    }

    /// 根据节点名启发式建议规则类型（AI 初步调整，用户可采纳）。
    /// 返回 (节点完整路径, 当前类型, 建议类型)。
    /// 规则：节点名含 素材/剪辑/混剪/绿幕/特效/合集/系列/精选/合集 等 → Complex；
    /// 其余默认 Simple 不给出建议。一级节点不被改动（强制 Simple）。
    pub fn suggest_rule_types(&self) -> Vec<(String, RuleType, RuleType)> {
        let complex_keywords = [
            "素材", "剪辑", "混剪", "绿幕", "特效", "合集", "系列", "精选", "综艺", "预告", "花絮",
            "原画", "草稿", "临时", "备份", "模板", "字体", "音效",
        ];
        let mut out = Vec::new();
        fn walk(
            node: &TreeNode,
            path: &str,
            depth: usize,
            keywords: &[&str],
            out: &mut Vec<(String, RuleType, RuleType)>,
        ) {
            let full_path = if path.is_empty() {
                node.name.clone()
            } else {
                format!("{path}/{}", node.name)
            };
            // 只有非一级节点才建议规则类型（一级强制 Simple）
            if depth > 0 {
                let any_keyword = keywords.iter().any(|k| node.name.contains(k));
                let suggested = if any_keyword {
                    RuleType::Complex
                } else {
                    RuleType::Simple
                };
                if suggested == RuleType::Complex && node.rule_type != RuleType::Complex {
                    out.push((full_path.clone(), node.rule_type, suggested));
                } else if suggested == RuleType::Simple && node.rule_type != RuleType::Simple {
                    out.push((full_path.clone(), node.rule_type, suggested));
                }
            }
            for child in &node.children {
                walk(child, &full_path, depth + 1, keywords, out);
            }
        }
        for root in &self.roots {
            walk(root, "", 0, &complex_keywords, &mut out);
        }
        out
    }

    /// 硬校验：根的第一级子节点必须为 Simple 规则。
    /// 返回违规节点名列表（含简要原因）。
    pub fn validate(&self) -> Vec<String> {
        let mut violations = Vec::new();
        for root in &self.roots {
            for child in &root.children {
                if child.rule_type != RuleType::Simple {
                    violations.push(format!(
                        "一级目录「{}」必须是简单规则（按扩展名），请先指定扩展名或改为简单规则",
                        child.name
                    ));
                } else if child.extensions.is_empty() {
                    violations.push(format!(
                        "一级目录「{}」为简单规则但未指定扩展名，无法命中任何文件",
                        child.name
                    ));
                }
            }
        }
        violations
    }

    /// 从 JSON 字符串解析。
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }

    /// 序列化为 JSON 字符串。
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// 从文件加载自定义树；文件不存在则返回默认树。
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if path.exists() {
            let s = std::fs::read_to_string(path)?;
            Self::from_json(&s)
        } else {
            Ok(Self::default_tree())
        }
    }

    /// 保存自定义树到文件。
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_json()?)?;
        Ok(())
    }

    /// 重置为默认树模板。
    pub fn reset(&self) -> Self {
        Self::default_tree()
    }

    /// 内置默认树模板。
    pub fn default_tree() -> Self {
        fn tn(
            name: &str,
            rule: RuleType,
            exts: &[&str],
            note: Option<&str>,
            children: Vec<TreeNode>,
        ) -> TreeNode {
            TreeNode {
                name: name.into(),
                description: None,
                rule_type: rule,
                extensions: exts.iter().map(|s| s.to_string()).collect(),
                note: note.map(|s| s.into()),
                few_shot_refs: vec![],
                children,
            }
        }
        // 需要 description 的节点用完整 struct
        fn tdn(
            name: &str,
            desc: &str,
            rule: RuleType,
            exts: &[&str],
            children: Vec<TreeNode>,
        ) -> TreeNode {
            TreeNode {
                name: name.into(),
                description: Some(desc.into()),
                rule_type: rule,
                extensions: exts.iter().map(|s| s.to_string()).collect(),
                note: None,
                few_shot_refs: vec![],
                children,
            }
        }
        Self {
            roots: vec![
                tdn(
                    "媒体",
                    "视频、音频",
                    RuleType::Simple,
                    &[],
                    vec![
                        tdn(
                            "电影",
                            "完整长片",
                            RuleType::Simple,
                            &["mp4", "mkv", "avi", "mov"],
                            vec![
                                tn("国产", RuleType::Complex, &[], Some("中文电影"), vec![]),
                                tn("欧美", RuleType::Complex, &[], Some("欧美电影"), vec![]),
                                tn("日韩", RuleType::Complex, &[], Some("日韩电影"), vec![]),
                            ],
                        ),
                        tdn(
                            "番剧",
                            "日本动画",
                            RuleType::Simple,
                            &["mkv", "mp4"],
                            vec![
                                tn("TV", RuleType::Complex, &[], Some("电视动画"), vec![]),
                                tn("剧场版", RuleType::Complex, &[], Some("动画电影"), vec![]),
                            ],
                        ),
                        tn(
                            "剪辑素材",
                            RuleType::Simple,
                            &["mp4", "mov"],
                            Some("视频编辑素材"),
                            vec![],
                        ),
                        tn(
                            "音乐",
                            RuleType::Simple,
                            &["mp3", "flac", "wav"],
                            None,
                            vec![],
                        ),
                    ],
                ),
                tn(
                    "文档",
                    RuleType::Simple,
                    &[],
                    None,
                    vec![
                        tn(
                            "工作",
                            RuleType::Simple,
                            &["doc", "docx", "xlsx"],
                            None,
                            vec![],
                        ),
                        tn("个人", RuleType::Simple, &["pdf"], None, vec![]),
                        tn("课件", RuleType::Simple, &["pptx", "pdf"], None, vec![]),
                    ],
                ),
                tn(
                    "图片",
                    RuleType::Simple,
                    &[],
                    None,
                    vec![
                        tn(
                            "照片",
                            RuleType::Simple,
                            &["jpg", "jpeg", "heic"],
                            None,
                            vec![],
                        ),
                        tn("截图", RuleType::Simple, &["png"], None, vec![]),
                    ],
                ),
                tn(
                    "软件",
                    RuleType::Simple,
                    &[],
                    None,
                    vec![
                        tn(
                            "安装包",
                            RuleType::Simple,
                            &["exe", "dmg", "pkg", "msi"],
                            None,
                            vec![],
                        ),
                        tn(
                            "压缩包",
                            RuleType::Simple,
                            &["zip", "rar", "7z"],
                            None,
                            vec![],
                        ),
                    ],
                ),
                tdn("未分类", "无法确定类型", RuleType::Simple, &[], vec![]),
            ],
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_tree_with_paths() {
        let tree = TreeTemplate::default_tree();
        // 默认树应通过自身校验（一级全部 Simple 且有扩展名）
        assert!(
            tree.validate().is_empty(),
            "默认树校验应通过: {:?}",
            tree.validate()
        );
        let cats = tree.to_categories();
        // 应有未分类桶
        assert!(cats.iter().any(|c| c.name == "未分类"));
        // 路径应为分层
        assert!(cats.iter().any(|c| c.subfolder == "媒体/电影/国产"));
        assert!(cats.iter().any(|c| c.subfolder == "媒体/番剧/剧场版"));
        assert!(cats.iter().any(|c| c.subfolder == "文档/工作"));
        // 一级应为 Simple（CategoryDef 不携带 extensions，它们只存在树节点）
        let yinyue = cats.iter().find(|c| c.name == "音乐").unwrap();
        assert_eq!(yinyue.rule_type, RuleType::Simple);
        // 复杂规则节点带 note
        let guochan = cats.iter().find(|c| c.name == "国产").unwrap();
        assert_eq!(guochan.rule_type, RuleType::Complex);
        assert_eq!(guochan.note.as_deref(), Some("中文电影"));
    }

    #[test]
    fn validate_rejects_complex_first_level() {
        let tree = TreeTemplate {
            orphans: vec![],
            roots: vec![TreeNode {
                name: "根".into(),
                children: vec![TreeNode {
                    name: "一级复杂".into(),
                    rule_type: RuleType::Complex,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let violations = tree.validate();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("必须"));

        // 一级 Simple 但未指定扩展名也应提示
        let tree2 = TreeTemplate {
            orphans: vec![],
            roots: vec![TreeNode {
                name: "根".into(),
                children: vec![TreeNode {
                    name: "一级无扩展名".into(),
                    rule_type: RuleType::Simple,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let violations = tree2.validate();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("扩展名"));
    }

    #[test]
    fn old_json_compatibility() {
        // 旧格式 tree.json：节点只有 name/description/children
        let json = r#"{
            "roots": [
                {"name": "媒体", "description": "影音", "children": [
                    {"name": "电影", "children": []}
                ]},
                {"name": "未分类", "children": []}
            ]
        }"#;
        let tree = TreeTemplate::from_json(json).unwrap();
        let cats = tree.to_categories();
        // 旧节点默认 Simple
        let dianying = cats.iter().find(|c| c.name == "电影").unwrap();
        assert_eq!(dianying.rule_type, RuleType::Simple);
        // 未分类不重复
        assert_eq!(cats.iter().filter(|c| c.name == "未分类").count(), 1);
    }

    #[test]
    fn suggest_rule_types_basic() {
        let tree = TreeTemplate {
            orphans: vec![],
            roots: vec![TreeNode {
                name: "媒体".into(),
                children: vec![TreeNode {
                    name: "视频专区".into(), // 一级：不应被建议改动
                    rule_type: RuleType::Simple,
                    extensions: vec!["mp4".into()],
                    children: vec![
                        TreeNode {
                            name: "剪辑素材".into(),
                            rule_type: RuleType::Simple,
                            ..Default::default()
                        }, // 命中关键词 → 建议 Complex
                        TreeNode {
                            name: "普通片子".into(),
                            rule_type: RuleType::Complex,
                            ..Default::default()
                        }, // 不命中 → 建议 Simple
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let sugg = tree.suggest_rule_types();
        // "普通片子" 为 Complex→建议 Simple；"剪辑素材" Simple→建议 Complex
        assert!(sugg
            .iter()
            .any(|(p, _, s)| p == "媒体/视频专区/剪辑素材" && *s == RuleType::Complex));
        assert!(sugg
            .iter()
            .any(|(p, _, s)| p == "媒体/视频专区/普通片子" && *s == RuleType::Simple));
        // 一级不被建议
        assert!(!sugg.iter().any(|(p, _, _)| p == "媒体/视频专区"));
    }
}
