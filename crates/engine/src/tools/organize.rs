//! 整理类工具：规则分类（确定性、token-free）、批量移动、批量回收站删除。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use crate::domain::{deny, BatchId};
use crate::llm::ToolDef;
use crate::scan;
use crate::tools::{fresh_batch, Tool, ToolContext};
use crate::trajectory::{new_batch_id, EventKind};
use crate::Result;

/// 规则分类工具：按扩展名把扫描到的文件映射到目标子目录，输出“计划”而非执行。
pub struct ClassifyByRulesTool;

#[async_trait]
impl Tool for ClassifyByRulesTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "classify_by_rules".into(),
            description: "按用户给定的扩展名规则，把下载目录中的文件映射到目标子目录，返回拟移动计划(不执行)。这是零 token 成本的确定性整理，应优先于 LLM 推理使用；LLM 仅处理未命中的歧义文件。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "target_dir": { "type": "string", "description": "整理目标根目录" },
                    "rules": {
                        "type": "array",
                        "description": "扩展名→子目录映射规则",
                        "items": {
                            "type": "object",
                            "properties": {
                                "extensions": { "type": "array", "items": { "type": "string" } },
                                "subfolder": { "type": "string" }
                            },
                            "required": ["extensions", "subfolder"]
                        }
                    }
                },
                "required": ["target_dir", "rules"]
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: &ToolContext,
        _cancel: &CancellationToken,
    ) -> Result<Value> {
        let target_dir = match args.get("target_dir").and_then(|v| v.as_str()) {
            Some(s) => PathBuf::from(s),
            None => return Ok(deny("缺少 target_dir")),
        };
        let rules = match args.get("rules").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => return Ok(deny("缺少 rules")),
        };
        // 归一化规则：扩展名小写
        let rules_norm: Vec<(Vec<String>, String)> = rules
            .iter()
            .filter_map(|r| {
                let exts = r
                    .get("extensions")?
                    .as_array()?
                    .iter()
                    .filter_map(|e| e.as_str().map(|s| s.to_ascii_lowercase()))
                    .collect::<Vec<_>>();
                let sub = r.get("subfolder")?.as_str()?.to_string();
                Some((exts, sub))
            })
            .collect();

        let root = ctx.scan_root.clone();
        let entries = spawn_blocking(move || scan::scan(&root))
            .await
            .map_err(|e| anyhow::anyhow!("扫描失败: {e}"))?;

        let mut moves = Vec::new();
        let mut unmatched = Vec::new();
        for e in entries.iter().filter(|e| {
            ctx.permissions.tier_for(e.extension.as_deref(), e.size)
                != crate::permission::AccessTier::None
        }) {
            let ext = e.extension.as_deref().unwrap_or("");
            if let Some((_, sub)) = rules_norm
                .iter()
                .find(|(exts, _)| exts.iter().any(|x| x == ext))
            {
                let dst = target_dir.join(sub).join(e.basename());
                moves
                    .push(json!({ "src": e.path.to_string_lossy(), "dst": dst.to_string_lossy() }));
            } else {
                unmatched
                    .push(json!({ "path": e.path.to_string_lossy(), "extension": e.extension }));
            }
        }
        Ok(
            json!({ "matched": moves.len(), "unmatched": unmatched.len(), "moves": moves, "unmatched_files": unmatched }),
        )
    }
}

/// 批量移动工具：执行一组移动，逐条写轨迹（含批次号），支持 dry_run 预览与命名冲突自动避让。
/// 删除语义走“移动到目标目录”，回滚信息已记录在轨迹中（逆序按 dst->src 可恢复）。
pub struct MoveBatchTool;

#[async_trait]
impl Tool for MoveBatchTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "move_batch".into(),
            description: "批量移动文件。逐条记录到操作轨迹(含批次号与回滚信息)。dry_run=true 时只返回将要发生的结果而不真正移动。目标已存在时自动追加数字后缀避免覆盖。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "dry_run": { "type": "boolean", "default": false },
                    "moves": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "src": { "type": "string" },
                                "dst": { "type": "string" }
                            },
                            "required": ["src", "dst"]
                        }
                    }
                },
                "required": ["moves"]
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: &ToolContext,
        _cancel: &CancellationToken,
    ) -> Result<Value> {
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !dry_run {
            return Ok(deny(
                "旧工具不允许修改文件；请通过审查后的 safe_fs 执行计划",
            ));
        }
        let moves = match args.get("moves").and_then(|v| v.as_array()) {
            Some(a) => a.clone(),
            None => return Ok(deny("缺少 moves")),
        };

        let batch_id = new_batch_id();
        let traj = ctx.trajectory.clone();
        let results = if dry_run {
            // 仅预演：构造冲突避让后的目标路径并返回
            moves
                .iter()
                .filter_map(|m| {
                    let src = m.get("src")?.as_str()?.to_string();
                    let dst = m.get("dst")?.as_str()?.to_string();
                    let resolved = resolve_collision(Path::new(&dst));
                    Some(json!({ "src": src, "dst": resolved.to_string_lossy(), "status": "preview", "mode": "preview" }))
                })
                .collect::<Vec<_>>()
        } else {
            let traj2 = traj.clone();
            let batch_id2 = batch_id;
            spawn_blocking(move || -> Vec<Value> {
                let mut out = Vec::new();
                for m in &moves {
                    let src = match m.get("src").and_then(|v| v.as_str()) {
                        Some(s) => PathBuf::from(s),
                        None => {
                            out.push(json!({ "status": "skipped", "error": "缺少 src" }));
                            continue;
                        }
                    };
                    let dst_raw = match m.get("dst").and_then(|v| v.as_str()) {
                        Some(s) => PathBuf::from(s),
                        None => {
                            out.push(json!({ "src": src.to_string_lossy(), "status": "skipped", "error": "缺少 dst" }));
                            continue;
                        }
                    };
                    let dst = resolve_collision(&dst_raw);
                    match perform_move(&src, &dst) {
                        Ok(mode) => {
                            let _ = traj2.log(
                                EventKind::FileOperation,
                                Some(batch_id2),
                                json!({
                                    "op": "move",
                                    "mode": mode,
                                    "src": src.to_string_lossy(),
                                    "dst": dst.to_string_lossy(),
                                    "size": std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0),
                                }),
                            );
                            out.push(json!({ "src": src.to_string_lossy(), "dst": dst.to_string_lossy(), "status": "ok", "mode": mode }));
                        }
                        Err(e) => {
                            let _ = traj2.log(
                                EventKind::Error,
                                Some(batch_id2),
                                json!({ "op": "move", "src": src.to_string_lossy(), "dst": dst.to_string_lossy(), "error": e.to_string() }),
                            );
                            out.push(json!({ "src": src.to_string_lossy(), "dst": dst.to_string_lossy(), "status": "failed", "error": e.to_string() }));
                        }
                    }
                }
                out
            })
            .await
            .map_err(|e| anyhow::anyhow!("批量移动任务失败: {e}"))?
        };

        Ok(json!({ "batch_id": batch_id.to_string(), "executed": !dry_run, "results": results }))
    }
}

/// 若 dst 已存在则追加 " (n)" 后缀（保留扩展名）。
pub fn resolve_collision(dst: &Path) -> PathBuf {
    if !dst.exists() {
        return dst.to_path_buf();
    }
    let parent = dst.parent().unwrap_or_else(|| Path::new(""));
    let stem = dst
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = dst.extension().map(|s| s.to_string_lossy().into_owned());
    for n in 1..10_000 {
        let new_stem = format!("{stem} ({n})");
        let candidate = match &ext {
            Some(e) => parent.join(format!("{new_stem}.{e}")),
            None => parent.join(&new_stem),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    dst.to_path_buf()
}

/// Legacy plan compatibility: same safe no-overwrite move primitive, never copy-and-delete.
pub fn perform_move(src: &Path, dst: &Path) -> std::io::Result<&'static str> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::safe_fs::move_noreplace(src, dst).map_err(std::io::Error::other)?;
    Ok("rename")
}

/// 批量回收站删除工具：将文件移入系统回收站/废纸篓（可恢复）。
/// 逐条写轨迹。dry_run=true 时只预览不执行。回收站删除无法由本系统自动回滚，
/// 需用户通过 Finder/资源管理器手动恢复。
pub struct TrashBatchTool;

#[async_trait]
impl Tool for TrashBatchTool {
    fn def(&self) -> ToolDef {
        ToolDef {
            name: "trash_batch".into(),
            description: "将文件移入系统回收站/废纸篓（可恢复）。逐条记录到操作轨迹(含批次号)。dry_run=true 时只返回将要被删除的文件清单而不执行。注意：回收站操作无法由本系统自动回滚，需用户手动恢复。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "dry_run": { "type": "boolean", "default": false },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "要删除的文件绝对路径列表"
                    }
                },
                "required": ["paths"]
            }),
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: &ToolContext,
        _cancel: &CancellationToken,
    ) -> Result<Value> {
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !dry_run {
            return Ok(deny(
                "旧工具不允许修改文件；请通过审查后的 safe_fs 执行计划",
            ));
        }
        let paths = match args.get("paths").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => return Ok(deny("缺少 paths")),
        };

        let batch_id = new_batch_id();
        let traj = ctx.trajectory.clone();
        let results = if dry_run {
            paths
                .iter()
                .filter_map(|p| {
                    let p = p.as_str()?;
                    let exists = Path::new(p).exists();
                    Some(json!({ "path": p, "status": "preview", "exists": exists }))
                })
                .collect::<Vec<_>>()
        } else {
            let traj2 = traj.clone();
            let batch_id2 = batch_id;
            let paths_owned: Vec<Value> = paths.clone();
            spawn_blocking(move || -> Vec<Value> {
                let mut out = Vec::new();
                for p in &paths_owned {
                    let path = match p.as_str() {
                        Some(s) => PathBuf::from(s),
                        None => {
                            out.push(json!({ "status": "skipped", "error": "非字符串路径" }));
                            continue;
                        }
                    };
                    if !path.exists() {
                        let _ = traj2.log(
                            EventKind::Error,
                            Some(batch_id2),
                            json!({ "op": "trash", "src": path.to_string_lossy(), "error": "文件不存在" }),
                        );
                        out.push(json!({ "path": path.to_string_lossy(), "status": "failed", "error": "文件不存在" }));
                        continue;
                    }
                    match trash::delete(&path) {
                        Ok(()) => {
                            let _ = traj2.log(
                                EventKind::FileOperation,
                                Some(batch_id2),
                                json!({ "op": "trash", "src": path.to_string_lossy() }),
                            );
                            out.push(json!({ "path": path.to_string_lossy(), "status": "ok", "mode": "trash" }));
                        }
                        Err(e) => {
                            let _ = traj2.log(
                                EventKind::Error,
                                Some(batch_id2),
                                json!({ "op": "trash", "src": path.to_string_lossy(), "error": e.to_string() }),
                            );
                            out.push(json!({ "path": path.to_string_lossy(), "status": "failed", "error": e.to_string() }));
                        }
                    }
                }
                out
            })
            .await
            .map_err(|e| anyhow::anyhow!("批量回收站任务失败: {e}"))?
        };

        Ok(json!({ "batch_id": batch_id.to_string(), "executed": !dry_run, "results": results }))
    }
}

/// 供测试与 agent 复用：生成批次号。
#[allow(dead_code)]
pub fn batch() -> BatchId {
    fresh_batch()
}
