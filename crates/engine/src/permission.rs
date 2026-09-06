//! 分级隐私授权。
//!
//! 权限由用户配置决定，**独立于 LLM**：工具在构造返回给 LLM 的内容前，
//! 先依据文件扩展名 + 文件大小查到该类型的 `AccessTier`，再据此裁剪/拒绝内容。
//! LLM 无法通过 prompt 绕过——它根本没有拿到被裁掉的数据。
//!
//! 形态：**预设规则集（default_presets）+ 用户自定义规则**。
//! 每条规则按扩展名（可为空作兜底）和文件大小区间（可选）划定适用范围，
//! 自上而下匹配第一条命中。

use serde::{Deserialize, Serialize};

/// Category metadata shared by clients. Choosing a preset does not grant content access.
#[derive(Serialize)]
pub struct PermissionPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub color: &'static str,
    pub extensions: Vec<&'static str>,
}
pub fn category_presets() -> Vec<PermissionPreset> {
    [
        ("video", "视频", "#7560ad", "mp4 mkv avi mov webm m4v wmv flv mpg mpeg ts m2ts 3gp"),
        ("text", "文本", "#278477", "txt md markdown rst log text"),
        ("document", "文档", "#467dc2", "pdf doc docx docm dot dotx odt rtf pages wps"),
        ("image", "图片", "#be7840", "jpg jpeg png gif webp bmp tif tiff heic heif avif svg ico raw cr2 nef arw"),
        ("music", "音乐", "#ba5783", "mp3 flac wav m4a aac ogg opus wma aiff ape alac mid midi"),
        ("spreadsheet", "表格", "#51863d", "xls xlsx xlsm xlsb xltx ods csv tsv numbers et"),
        ("presentation", "演示文稿", "#c26449", "ppt pptx pptm pps ppsx odp key dps"),
        ("archive", "压缩包", "#a18a31", "zip rar 7z tar gz bz2 xz zst tgz cab"),
        ("software", "软件安装包", "#617587", "exe msi msix appx dmg pkg deb rpm apk aab"),
        ("ebook", "电子书", "#886348", "epub mobi azw azw3 fb2 djvu cbz cbr"),
        ("code", "代码与配置", "#388396", "rs py js ts jsx tsx html css c h cpp hpp java go swift sh bat ps1 json yaml yml toml xml ini conf sql ipynb"),
        ("design", "设计与模型", "#826b9f", "psd ai sketch fig xd blend obj fbx stl glb gltf dwg dxf"),
        ("font", "字体", "#887a68", "ttf otf woff woff2 ttc"),
        ("folder", "文件夹", "#628b69", "@folder"),
        ("disk", "磁盘镜像", "#5e719b", "iso img vhd vhdx vmdk qcow2"),
    ].into_iter().map(|(id, name, color, extensions)| PermissionPreset {
        id, name, color, extensions: extensions.split_whitespace().collect(),
    }).collect()
}

/// 对单类文件允许大模型访问的层级，由弱到强。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AccessTier {
    /// 不可访问：仅可按扩展名参与规则整理，LLM 看不到文件名与内容。
    None,
    /// 仅文件名：可读 basename 与扩展名。
    FilenameOnly,
    /// 元数据：文件名 + 大小 + 修改时间 + 扩展元数据（EXIF/标签等）。
    Metadata,
    /// 图像模态：在 Metadata 基础上，可读取图片缩略图（多层视觉内容，需多模态模型）。
    Image,
    /// 内容切片：在 Metadata 基础上，可读最多 `content_slice_bytes` 字节文本。
    ContentSlice,
}

impl Default for AccessTier {
    fn default() -> Self {
        AccessTier::FilenameOnly
    }
}

/// 一条规则：匹配扩展名 + 大小区间 → 授予某个层级。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionRule {
    /// Display category only; never participates in permission matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// 小写扩展名列表，如 `["txt", "md"]`。空表示匹配所有（兜底规则）。
    #[serde(default)]
    pub extensions: Vec<String>,
    /// 适用文件大小下限（字节），None 表示不限。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_bytes: Option<u64>,
    /// 适用文件大小上限（字节），None 表示不限。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    pub tier: AccessTier,
}

impl PermissionRule {
    /// 通配规则（后置兜底）。
    pub fn fallback(tier: AccessTier) -> Self {
        Self {
            category: None,
            extensions: vec![],
            min_bytes: None,
            max_bytes: None,
            tier,
        }
    }

    pub fn matches(&self, extension: Option<&str>, size: u64) -> bool {
        let ext_ok = if self.extensions.is_empty() {
            true
        } else {
            extension
                .map(|e| self.extensions.iter().any(|r| r.eq_ignore_ascii_case(e)))
                .unwrap_or(false)
        };
        let min_ok = self.min_bytes.map(|m| size >= m).unwrap_or(true);
        let max_ok = self.max_bytes.map(|m| size <= m).unwrap_or(true);
        ext_ok && min_ok && max_ok
    }
}

/// 完整权限配置。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionConfig {
    /// 未命中任何规则的扩展名/大小所适用的默认层级。
    #[serde(default)]
    pub default: AccessTier,
    /// 显式规则，自上而下匹配第一条命中。
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    /// 内容切片的硬上限（字节）。工具读取内容时强制裁剪到此值。
    #[serde(default = "default_slice_bytes")]
    pub content_slice_bytes: usize,
}

fn default_slice_bytes() -> usize {
    4096
}

impl PermissionConfig {
    /// 依据扩展名 + 大小决定访问层级。
    pub fn tier_for(&self, extension: Option<&str>, size: u64) -> AccessTier {
        for rule in &self.rules {
            if rule.matches(extension, size) {
                return rule.tier;
            }
        }
        self.default
    }

    /// 实际可读内容字节数：取用户请求值与硬上限的较小者。
    pub fn clamp_slice(&self, requested: usize) -> usize {
        requested.min(self.content_slice_bytes)
    }

    /// 预设规则集：按扩展名 + 大小给出合理默认的隐私分层，可在此基础上增删改。
    pub fn default_presets() -> Vec<PermissionRule> {
        vec![
            // 文本类：允许读内容切片（小文本），大文件仅文件名
            PermissionRule {
                category: None,
                extensions: vec![
                    "txt".into(),
                    "md".into(),
                    "csv".into(),
                    "json".into(),
                    "log".into(),
                    "conf".into(),
                ],
                min_bytes: None,
                max_bytes: Some(1_048_576),
                tier: AccessTier::ContentSlice,
            },
            // Office 文档：仅文件名（内容解析为真实工作量）
            PermissionRule {
                category: None,
                extensions: vec!["doc".into(), "docx".into(), "odt".into()],
                min_bytes: None,
                max_bytes: None,
                tier: AccessTier::FilenameOnly,
            },
            // 表格：彻底不可访问，仅按扩展名整理
            PermissionRule {
                category: None,
                extensions: vec!["xls".into(), "xlsx".into(), "xlsm".into(), "ods".into()],
                min_bytes: None,
                max_bytes: None,
                tier: AccessTier::None,
            },
            // 媒体：元数据层（EXIF/标签提取，不读内容切片）
            PermissionRule {
                category: None,
                extensions: vec![
                    "mp4".into(),
                    "mkv".into(),
                    "avi".into(),
                    "mov".into(),
                    "mp3".into(),
                    "flac".into(),
                    "jpg".into(),
                    "jpeg".into(),
                    "png".into(),
                    "png".into(),
                ],
                min_bytes: None,
                max_bytes: None,
                tier: AccessTier::Metadata,
            },
            // 图片外其余可疑类型：仅文件名
            PermissionRule {
                category: None,
                extensions: vec![
                    "zip".into(),
                    "rar".into(),
                    "7z".into(),
                    "exe".into(),
                    "dmg".into(),
                    "pdf".into(),
                ],
                min_bytes: None,
                max_bytes: None,
                tier: AccessTier::FilenameOnly,
            },
            // 兜底：仅文件名
            PermissionRule::fallback(AccessTier::FilenameOnly),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_presets_are_unique_and_custom_extensions_control_permissions() {
        let presets = category_presets();
        assert!(presets.len() >= 14);
        let mut ids = std::collections::HashSet::new();
        for p in presets {
            assert!(ids.insert(p.id));
            // Some formats are ambiguous (video TS / TypeScript). Rule order decides.
            let mut extensions = std::collections::HashSet::new();
            for ext in p.extensions {
                assert!(extensions.insert(ext), "duplicate extension: {ext}");
            }
        }
        let rule: PermissionRule = serde_json::from_value(serde_json::json!({
            "category":"video", "extensions":["custommovie"], "tier":"image"
        }))
        .unwrap();
        let saved = serde_json::to_value(&rule).unwrap();
        assert_eq!(saved["category"], "video");
        let restored: PermissionRule = serde_json::from_value(saved).unwrap();
        let p = PermissionConfig {
            default: AccessTier::None,
            rules: vec![restored],
            content_slice_bytes: 0,
        };
        assert_eq!(p.tier_for(Some("CUSTOMMOVIE"), 10), AccessTier::Image);
        assert_eq!(
            p.tier_for(Some("mp4"), 10),
            AccessTier::None,
            "The label must not expand the edited extension list"
        );
    }

    fn cfg() -> PermissionConfig {
        serde_json::from_str(
            r#"{
                "default": "filename_only",
                "content_slice_bytes": 2048,
                "rules": [
                    {"extensions": ["txt","md"], "max_bytes": 1024, "tier": "content_slice"},
                    {"extensions": ["docx"], "tier": "filename_only"},
                    {"extensions": ["xlsx"], "tier": "none"},
                    {"extensions": [], "tier": "filename_only"}
                ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn tier_resolution() {
        let c = cfg();
        assert_eq!(c.tier_for(Some("txt"), 100), AccessTier::ContentSlice);
        assert_eq!(
            c.tier_for(Some("TXT"), 10_000),
            AccessTier::FilenameOnly,
            "超过 max_bytes 应跳过并命中后置兜底规则"
        );
        assert_eq!(c.tier_for(Some("docx"), 0), AccessTier::FilenameOnly);
        assert_eq!(c.tier_for(Some("xlsx"), 0), AccessTier::None);
        assert_eq!(c.tier_for(Some("zip"), 0), AccessTier::FilenameOnly);
        assert_eq!(c.tier_for(None, 0), AccessTier::FilenameOnly);
    }

    #[test]
    fn tier_by_size_window() {
        // 大文件（> 1GB）才允许 Metadata 的测试
        let c = PermissionConfig {
            rules: vec![PermissionRule {
                category: None,
                extensions: vec!["bin".into()],
                min_bytes: Some(1_000_000_000),
                max_bytes: None,
                tier: AccessTier::Metadata,
            }],
            ..Default::default()
        };
        assert_eq!(c.tier_for(Some("bin"), 1_500_000_000), AccessTier::Metadata);
        assert_eq!(c.tier_for(Some("bin"), 5_000), AccessTier::FilenameOnly);
    }

    #[test]
    fn presets_sane() {
        let presets = PermissionConfig::default_presets();
        let c = PermissionConfig {
            rules: presets,
            ..Default::default()
        };
        assert_eq!(c.tier_for(Some("txt"), 10), AccessTier::ContentSlice);
        assert_eq!(c.tier_for(Some("txt"), 5_000_000), AccessTier::FilenameOnly);
        assert_eq!(c.tier_for(Some("xlsx"), 0), AccessTier::None);
        assert_eq!(c.tier_for(Some("mp4"), 10), AccessTier::Metadata);
        assert_eq!(
            c.tier_for(Some("unknown_ext"), 10),
            AccessTier::FilenameOnly
        );
    }

    #[test]
    fn clamp() {
        let c = cfg();
        assert_eq!(c.clamp_slice(10000), 2048);
        assert_eq!(c.clamp_slice(100), 100);
    }
}
