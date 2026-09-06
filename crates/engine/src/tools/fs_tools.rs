//! 文件系统读取类工具（扫描、内容切片读取）。
//!
//! 内容读取严格按 `AccessTier` 强制：仅 `ContentSlice` 层级允许读取，
//! 且字节数取用户请求值与 `content_slice_bytes` 硬上限的较小者。

use std::io::Read;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use crate::domain::{deny, FileEntry};
use crate::llm::ToolDef;
use crate::permission::AccessTier;
use crate::scan;
use crate::tools::{Tool, ToolContext};
use crate::Result;

pub struct ScanDownloadsTool;

#[async_trait]
impl Tool for ScanDownloadsTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "scan_downloads".into(),
            description: "扫描默认下载目录，返回所有文件及其元数据与该文件类型的访问层级(tier)。tier 决定后续可对该文件读取到何种程度：none=仅可按扩展名规则整理、filename_only=可读文件名、metadata=可读元数据、content_slice=可读内容切片。".into(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        _args: Value,
        ctx: &ToolContext,
        _cancel: &CancellationToken,
    ) -> Result<Value> {
        let root = ctx.scan_root.clone();
        let perms = ctx.permissions.clone();
        let entries = spawn_blocking(move || scan::scan(&root))
            .await
            .map_err(|e| anyhow::anyhow!("扫描任务失败: {e}"))?;

        let arr: Vec<Value> = entries
            .iter()
            .filter(|e| perms.tier_for(e.extension.as_deref(), e.size) != AccessTier::None)
            .map(|e| {
                let tier = perms.tier_for(e.extension.as_deref(), e.size);
                let modified = e
                    .modified
                    .duration_since(UNIX_EPOCH)
                    .map(|d| {
                        chrono::DateTime::<Utc>::from_timestamp(d.as_secs() as i64, 0)
                            .map(|t| t.to_rfc3339())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let mut value = json!({
                    "basename": e.basename(),
                    "extension": e.extension,
                    "size": e.size,
                    "modified": modified,
                    "tier": tier,
                });
                if tier < AccessTier::Metadata {
                    value.as_object_mut().unwrap().remove("size");
                    value.as_object_mut().unwrap().remove("modified");
                }
                value
            })
            .collect();
        Ok(json!({ "count": arr.len(), "files": arr }))
    }
}

pub struct ReadContentSliceTool;

#[async_trait]
impl Tool for ReadContentSliceTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "read_content_slice".into(),
            description: "读取一个文件的文本内容切片（前若干字节）。仅当该文件类型的 tier 为 content_slice 时允许；否则返回权限错误。用于对歧义文件做内容级分类。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "文件绝对路径" },
                    "max_bytes": { "type": "integer", "description": "希望读取的最大字节数，将受权限硬上限裁剪", "minimum": 1 }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: &ToolContext,
        _cancel: &CancellationToken,
    ) -> Result<Value> {
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Ok(deny("缺少参数 path")),
        };
        let requested = args
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(ctx.permissions.content_slice_bytes);
        let limit = ctx.permissions.clamp_slice(requested);

        let given = std::path::PathBuf::from(&path_str);
        let relative = if given.is_absolute() {
            match given.strip_prefix(&ctx.scan_root) {
                Ok(path) => path.to_path_buf(),
                Err(_) => return Ok(deny("路径不在授权根目录内")),
            }
        } else {
            given
        };
        let path = match crate::safe_fs::checked_path(
            &ctx.scan_root,
            &relative.to_string_lossy().replace('\\', "/"),
        ) {
            Ok(path) => path,
            Err(_) => return Ok(deny("路径越界或包含链接")),
        };
        let entry = match FileEntry::from_path(path.clone()) {
            Ok(e) => e,
            Err(e) => return Ok(deny(&format!("无法读取文件元数据: {e}"))),
        };

        let tier = ctx
            .permissions
            .tier_for(entry.extension.as_deref(), entry.size);
        if tier != AccessTier::ContentSlice {
            return Ok(deny(&format!(
                "权限不足：该扩展名({})层级为 {:?}，需 content_slice 才可读内容",
                entry.extension.as_deref().unwrap_or("(无)"),
                tier
            )));
        }

        let root = ctx.scan_root.clone();
        let rel = relative.to_string_lossy().replace('\\', "/");
        let size = entry.size;
        let modified = entry
            .modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let content = spawn_blocking(move || -> anyhow::Result<String> {
            let mut f = crate::safe_fs::open_evidence(&root, &rel, size, modified)?;
            let mut buf = vec![0u8; limit];
            let n = f.read(&mut buf)?;
            buf.truncate(n);
            crate::safe_fs::verify_evidence(&f, size, modified)?;
            // 丢失无效 UTF-8 字节，保证返回文本。
            Ok(String::from_utf8_lossy(&buf).into_owned())
        })
        .await
        .map_err(|e| anyhow::anyhow!("读取失败: {e}"))?;

        match content {
            Ok(text) => Ok(json!({
                "path": path_str,
                "basename": entry.basename(),
                "extension": entry.extension,
                "bytes_read": text.len(),
                "content": text,
            })),
            Err(e) => Ok(deny(&format!("读取文件失败: {e}"))),
        }
    }
}
