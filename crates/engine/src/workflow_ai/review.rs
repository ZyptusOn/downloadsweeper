//! Review edits are an atomic plan transaction; evidence reads obey existing permissions.
use super::*;
use std::collections::BTreeMap;

/// Apply the approved graph and finish only affected semantic branches before publication.
pub async fn apply_review_proposal(
    task: &mut Task, cfg: &AppConfig, store: &TaskStore, proposal_id: &str, ids: &[String],
    selected: Option<Vec<String>>, cancel: &CancellationToken, progress: &Progress<'_>,
    parallel: &(dyn Fn(crate::jobs::ParallelProgress) + Send + Sync),
) -> Result<()> {
    let proposal = task.proposal.clone().context("没有待处理建议")?;
    let changes: Vec<_> = proposal.changes.iter().filter(|c| ids.contains(&c.id)).cloned().collect();
    let mut draft = task.clone();
    if let Some(selected) = selected {
        ensure!(selected.iter().all(|id| draft.operations.iter().any(|o| &o.id == id)), "勾选的操作已过期");
        for op in &mut draft.operations { op.selected = selected.contains(&op.id); }
    }
    let id = proposal_id.to_owned();
    let ids = ids.to_vec();
    let local_cancel = cancel.clone();
    progress(0, 0, "正在应用建议并规划具体文件位置；完成后更新整理后预览");
    let mut draft = tokio::task::spawn_blocking(move || -> Result<Task> {
        draft.apply_proposal_with_progress(&id, &ids, "review", &local_cancel, &|_, _, _| {})?;
        Ok(draft)
    }).await??;
    let changed: HashSet<_> = changes.iter().filter(|c| c.kind == "node"
        && (c.before.is_null() || c.after.is_null() || ["parent","rule_type","extensions","note","examples"].iter().any(|key| c.before[*key] != c.after[*key])
            || (c.after["rule_type"] == "complex" && c.before["name"] != c.after["name"])))
        .map(|c| c.target.as_str()).collect();
    let explicit: HashSet<_> = changes.iter().filter(|c| c.kind == "placement").map(|c| c.target.as_str()).collect();
    let scope: HashSet<_> = draft.candidates().into_iter().filter(|entry| !explicit.contains(entry.id.as_str())
        && !draft.semantic_options(entry).is_empty()
        && (draft.semantic_options(entry).iter().any(|node| draft.ancestry(&node.id).is_ok_and(|path| path.iter().any(|n| changed.contains(n.id.as_str()))))
            || current_node(task, &entry.id).is_some_and(|node| task.ancestry(&node.id).is_ok_and(|path| path.iter().any(|n| changed.contains(n.id.as_str()))))))
        .map(|entry| entry.id.clone()).collect();
    if !scope.is_empty() {
        super::classification::finish_review(task, &mut draft, cfg, store, &scope, cancel, progress, parallel).await?;
    }
    ensure!(!cancel.is_cancelled(), "修改已暂停，原计划保留");
    // Runtime usage and checkpoints belong to the original task throughout the transaction.
    task.nodes = draft.nodes;
    task.operations = draft.operations;
    task.retained = draft.retained;
    task.classification = None;
    if scope.is_empty() { task.review_classification = None; }
    task.proposal = None;
    task.reviewed = false;
    task.plan_source = Some("ai_review".into());
    task.touch();
    store.save(task)?;
    store.event(task, "review_plan_applied", json!({"proposal_id":proposal_id,"changes":changes,"semantic_files":scope.len(),"operations":task.operations.len(),"files_moved":0}))?;
    progress(task.operations.len(), task.operations.len(), "具体位置规划已完成，整理后预览已更新");
    Ok(())
}

pub(super) const INSTRUCTIONS: &str = "review 是当前可见的整理审查页，允许 kind=node 调整目标结构，允许 kind=placement,target=review_files 中的 id,after={\"node_id\":目标节点ID或null} 调整某个文件/完整文件夹的归属，null 表示保留原位。禁止输出磁盘路径、重命名、删除文件或权限改动。一级目录仍必须按扩展名分类；例如把视频移出其它，应新增视频一级简单规则节点并从旧一级节点释放这些扩展名，Rust 会自动重算受影响文件，无需逐个列出。未受影响的现有分类会保留。只对给出的文件ID提出 placement；省略的文件不是不存在。节点与文件归属改动必须满足依赖关系，仍由用户选择采纳，合并后须重新审查。";

pub(super) fn current_node<'a>(task: &'a Task, source: &str) -> Option<&'a Node> {
    let op = task.operations.iter().find(|o| o.source == source)?;
    let parent = op.destination.rsplit_once('/')?.0;
    task.nodes.iter().find(|n| task.node_path(&n.id).is_ok_and(|p| p == parent))
}

fn allowed(task: &Task, entry: &Entry, node: &str) -> bool {
    task.active(node) && (task.rule_node(entry).is_some_and(|n| n.id == node)
        || task.semantic_options(entry).iter().any(|n| n.id == node))
}

pub(crate) fn revise_plan(original: &Task, draft: &mut Task, changes: &[Change], cancel: &CancellationToken, progress: &Progress<'_>) -> Result<()> {
    ensure!(original.is_organizing() && original.phase == 4 && original.status == "planned", "只能在整理计划审查阶段修改归属");
    draft.validate_graph(true)?;
    // Recalculate format rules locally, then retain still-valid paid semantic decisions.
    draft.generate_rules(cancel, progress)?;
    let mut placements: BTreeMap<String, Option<String>> = BTreeMap::new();
    for e in original.candidates() {
        let same_base = original.rule_node(e).map(|n| &n.id) == draft.rule_node(e).map(|n| &n.id);
        if let Some(node) = current_node(original, &e.id) {
            if (same_base || e.is_dir()) && allowed(draft, e, &node.id) {
                placements.insert(e.id.clone(), Some(node.id.clone()));
            }
        } else if same_base && original.retained.iter().any(|v| v["source"] == e.id) {
            placements.insert(e.id.clone(), None);
        }
    }
    for c in changes.iter().filter(|c| c.kind == "placement") {
        let entry = draft.candidates().into_iter().find(|e| e.id == c.target).context("文件不在可整理快照中，不能拆散整体目录")?;
        ensure!(entry_tier(draft, entry) != AccessTier::None, "没有读取此文件的权限");
        ensure!(!(draft.mode == "desktop" && crate::workflow::desktop_retained(entry))
            && !["crdownload", "part", "download", "tmp"].contains(&entry.extension.as_str()), "受保护条目不可调整");
        ensure!(!draft.nodes.iter().any(|n| n.mapping.as_ref().is_some_and(|m| under(&entry.id, m))), "复用容器内部文件保持原位");
        let value = c.after.get("node_id").context("归属建议缺少 node_id")?;
        let node = if value.is_null() { None } else {
            let id = value.as_str().context("归属节点 ID 无效")?;
            ensure!(allowed(draft, entry, id), "归属节点不存在或不符合扩展名规则；请同时采纳关联节点改动");
            Some(id.to_string())
        };
        placements.insert(c.target.clone(), node);
    }
    let old_ops: HashMap<_, _> = original.operations.iter().map(|o| (o.source.as_str(), o)).collect();
    draft.operations.retain(|o| !placements.contains_key(&o.source));
    draft.retained.retain(|v| !v["source"].as_str().is_some_and(|id| placements.contains_key(id)));
    let mut reserved: HashSet<_> = draft.operations.iter().map(|o| o.destination.to_lowercase()).collect();
    let total = placements.len();
    for (i, (source, node)) in placements.into_iter().enumerate() {
        ensure!(!cancel.is_cancelled(), "修改已暂停，原计划保留");
        progress(i, total, "正在更新审查计划，保留未受影响的分类");
        let entry = draft.entries.iter().find(|e| e.id == source).context("源条目不存在")?;
        let Some(node) = node else {
            let retained = original.retained.iter().find(|v| v["source"] == source).cloned()
                .unwrap_or_else(|| json!({"source":source,"reason":"审查调整：保留原位"}));
            draft.retained.push(retained);
            continue;
        };
        let target = format!("{}/{}", draft.node_path(&node)?, entry.name);
        if target == source {
            draft.retained.push(json!({"source":source,"reason":"已在目标位置"}));
            continue;
        }
        ensure!(!entry.is_dir() || !under(&target, &source), "目标不能位于源目录内部");
        let destination = collision_path(&draft.root, &target, &mut reserved)?;
        let old = old_ops.get(source.as_str()).copied();
        let manifest = if draft.mode == "desktop" && entry.is_dir() {
            if let Some(value) = old.and_then(|o| o.directory_manifest.clone()) { Some(value) }
            else { Some(crate::safe_fs::directory_manifest_with_progress(&checked_path(&draft.root, &source)?, cancel, progress)?) }
        } else { None };
        draft.operations.push(Operation {
            id: old.map(|o| o.id.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            source, destination: destination.clone(), kind: entry.kind.clone(), size: entry.size, modified_ms: entry.modified_ms,
            reason: old.filter(|o| o.destination == destination).map(|o| o.reason.clone()).unwrap_or_else(|| "AI 审查调整（尚未移动）".into()),
            selected: old.map_or(true, |o| o.selected), status: "pending".into(), error: None, fingerprint: None, directory_manifest: manifest,
        });
    }
    // Preserve the user's deselections even when a format rule changed.
    for op in &mut draft.operations { if let Some(old) = old_ops.get(op.source.as_str()) { op.selected = old.selected; op.id = old.id.clone(); } }
    ensure!(!cancel.is_cancelled(), "修改已暂停，原计划保留");
    draft.reviewed = false;
    draft.proposal = None;
    draft.classification = None; // Old batches refer to the old graph; never resume them.
    draft.plan_source = Some("ai_review".into());
    draft.touch();
    progress(total, total, "审查计划已更新，请检查新目标位置并重新确认");
    Ok(())
}
