use ds_engine::{
    domain::DirClass,
    permission::{AccessTier, PermissionConfig, PermissionRule},
    safe_fs::{self, TaskStore},
    tree::RuleType,
    workflow::{Change, Node, Proposal, Task},
    workflow_ai,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn fixture() -> (Task, TaskStore) {
    let base = std::env::temp_dir().join(format!("ds-workflow-test-{}", uuid::Uuid::new_v4()));
    let root = base.join("Downloads");
    std::fs::create_dir_all(root.join("Portable/config")).unwrap();
    std::fs::create_dir_all(root.join("文档/工作资料")).unwrap();
    for (path, content) in [
        ("a.txt", "private content"),
        ("movie.mp4", "video"),
        ("Portable/editor.exe", "program"),
        ("Portable/config/options.json", "{}"),
        ("文档/工作资料/existing.pdf", "existing"),
    ] {
        std::fs::write(root.join(path), content).unwrap();
    }
    let mut task = Task::new(
        root,
        "organize",
        PermissionConfig {
            default: AccessTier::FilenameOnly,
            content_slice_bytes: 8,
            rules: vec![],
        },
    )
    .unwrap();
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    let store = TaskStore::new(base.join("state")).unwrap();
    store.save(&task).unwrap();
    (task, store)
}
fn planned(task: &mut Task) {
    task.advance().unwrap();
    task.advance().unwrap();
    task.advance().unwrap();
    task.generate_rules(&CancellationToken::new(), &|_, _, _| {})
        .unwrap();
    task.advance().unwrap();
}
fn approved(task: &mut Task) {
    planned(task);
    task.reviewed = true;
    task.advance().unwrap();
}

#[test]
fn review_edit_cancellation_is_atomic_and_does_not_consume_the_proposal() {
    let (mut task, _) = fixture();
    planned(&mut task);
    let node = task.nodes[0].clone();
    let mut after = node.clone();
    after.name = "审查调整".into();
    task.proposal = Some(Proposal { id:"review-cancel".into(), scene:"review".into(), revision:task.revision,
        message:"rename category".into(), changes:vec![Change {id:"change".into(),kind:"node".into(),target:node.id.clone(),label:node.name.clone(),before:json!(node),after:json!(after)}] });
    let before = serde_json::to_value(&task).unwrap();
    let cancel = CancellationToken::new();
    // Cancel after local work has started, not merely before dispatch.
    let result = task.apply_proposal_with_progress("review-cancel", &["change".into()], "review", &cancel, &|_, _, _| { cancel.cancel(); });
    assert!(result.is_err());
    assert_eq!(serde_json::to_value(&task).unwrap(), before);
    task.apply_proposal("review-cancel", &["change".into()], "review").unwrap();
    assert_eq!(task.phase, 4);
    assert!(!task.reviewed);
    assert!(task.proposal.is_none());
    assert!(task.root.join("movie.mp4").exists());
}
fn orphan(id: &str, parent: Option<&str>) -> Node {
    Node {
        id: id.into(),
        parent: parent.map(String::from),
        name: id.into(),
        rule_type: RuleType::Simple,
        extensions: vec![],
        note: String::new(),
        examples: vec![],
        mapping: None,
        position: None,
    }
}

#[test]
fn six_phases_require_scan_plan_and_review() {
    let (mut task, _) = fixture();
    assert_eq!(task.phase, 0);
    task.advance().unwrap();
    assert_eq!(task.phase, 1);
    task.advance().unwrap();
    assert_eq!(task.phase, 2);
    task.advance().unwrap();
    assert_eq!(task.phase, 3);
    assert!(task.advance().is_err());
    task.generate_rules(&CancellationToken::new(), &|_, _, _| {})
        .unwrap();
    assert_eq!(task.phase, 3);
    assert_eq!(task.status, "planned");
    task.advance().unwrap();
    assert_eq!(task.phase, 4);
    assert!(task.advance().is_err());
    task.reviewed = true;
    task.advance().unwrap();
    assert_eq!(task.phase, 5);
}

#[test]
fn returning_to_planning_keeps_the_draft_but_upstream_changes_invalidate_it() {
    let (mut task, _) = fixture();
    planned(&mut task);
    task.operations[0].selected = false;
    task.plan_source = Some("ai".into());
    let original = serde_json::to_value(&task.operations).unwrap();
    task.reviewed = true;
    task.advance().unwrap();
    task.go_back(4).unwrap();
    assert!(!task.reviewed);
    task.go_back(3).unwrap();
    assert_eq!(serde_json::to_value(&task.operations).unwrap(), original);
    assert_eq!(task.plan_source.as_deref(), Some("ai"));
    task.advance().unwrap();
    assert!(task.advance().is_err());
    task.go_back(2).unwrap();
    assert_eq!(task.status, "draft");
    assert!(task.operations.is_empty());
    assert!(task.plan_source.is_none());
}

#[test]
fn an_empty_rule_plan_can_still_be_reviewed() {
    let (fixture, _) = fixture();
    let root = fixture.root.parent().unwrap().join("Empty");
    std::fs::create_dir(&root).unwrap();
    let mut task = Task::new(root, "organize", fixture.permissions).unwrap();
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    task.advance().unwrap();
    task.advance().unwrap();
    task.advance().unwrap();
    task.generate_rules(&CancellationToken::new(), &|_, _, _| {})
        .unwrap();
    assert_eq!(task.phase, 3);
    assert!(task.operations.is_empty());
    task.advance().unwrap();
    assert_eq!(task.phase, 4);
    assert!(!task.reviewed);
}
#[test]
fn atomic_folder_is_whole_and_existing_container_is_reused() {
    let (mut task, _) = fixture();
    planned(&mut task);
    assert!(task
        .operations
        .iter()
        .any(|o| o.source == "Portable" && o.kind == "directory"));
    assert!(!task
        .operations
        .iter()
        .any(|o| o.source.starts_with("Portable/")));
    assert!(!task
        .operations
        .iter()
        .any(|o| o.source.contains("existing.pdf")));
    assert!(task
        .operations
        .iter()
        .any(|o| o.source == "movie.mp4" && o.destination.starts_with("视频/")));
}
#[test]
fn directory_override_never_rescans_and_invalidates_plan() {
    let (mut task, _) = fixture();
    planned(&mut task);
    let count = task.entries.len();
    std::fs::write(task.root.join("arrived-after-scan.txt"), "new").unwrap();
    task.set_directory("Portable", DirClass::Normal).unwrap();
    assert_eq!(task.entries.len(), count);
    assert!(task.operations.is_empty());
    assert_eq!(task.phase, 2);
}
#[test]
fn graph_rejects_cycles_complex_top_and_duplicate_extensions() {
    let (mut task, _) = fixture();
    task.nodes.push(orphan("orphan", Some("orphan")));
    assert!(task.validate_graph(false).is_err());
    task.nodes.pop();
    let i = task
        .nodes
        .iter()
        .position(|n| n.parent.as_deref() == Some("root"))
        .unwrap();
    let old = task.nodes[i].clone();
    task.nodes[i].rule_type = RuleType::Complex;
    assert!(task.validate_graph(true).is_err());
    task.nodes[i] = old.clone();
    let mut duplicate = old;
    duplicate.id = "duplicate".into();
    duplicate.name = "duplicate".into();
    task.nodes.push(duplicate);
    assert!(task.validate_graph(true).is_err());
}
#[test]
fn detached_subtrees_remain_editable_and_do_not_participate() {
    let (mut task, _) = fixture();
    task.nodes.push(orphan("loose", None));
    task.nodes.push(orphan("child", Some("loose")));
    task.validate_graph(false).unwrap();
    assert!(!task.active("loose"));
    assert!(!task.active("child"));
    let loose = task.nodes.iter_mut().find(|n| n.id == "loose").unwrap();
    loose.parent = Some("root".into());
    loose.extensions = vec!["xyz".into()];
    assert!(task.active("child"));
    assert_eq!(task.node_path("child").unwrap(), "loose/child");
}
#[test]
fn prompt_context_obeys_permission_and_contains_no_paths() {
    let (mut task, _) = fixture();
    let file = task
        .entries
        .iter()
        .find(|e| e.id == "a.txt")
        .unwrap()
        .clone();
    task.permissions.default = AccessTier::None;
    assert!(workflow_ai::file_context(&task, &file).unwrap().is_none());
    task.permissions.default = AccessTier::FilenameOnly;
    let v = workflow_ai::file_context(&task, &file).unwrap().unwrap();
    assert_eq!(v, json!({"name":"a.txt","extension":"txt"}));
    task.permissions.default = AccessTier::ContentSlice;
    let v = workflow_ai::file_context(&task, &file).unwrap().unwrap();
    assert_eq!(v["text_excerpt"], "private ");
    assert!(!v.to_string().contains("Downloads"));
    task.permissions.rules = vec![PermissionRule {
        category: None,
        extensions: vec!["txt".into()],
        tier: AccessTier::None,
        min_bytes: None,
        max_bytes: None,
    }];
    assert!(workflow_ai::file_context(&task, &file).unwrap().is_none());
}
#[test]
fn no_overwrite_even_if_destination_appears_after_planning() {
    let (mut task, store) = fixture();
    approved(&mut task);
    let op = task
        .operations
        .iter()
        .find(|o| o.source == "a.txt")
        .unwrap()
        .clone();
    let destination = task.root.join(&op.destination);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(&destination, "occupied").unwrap();
    assert!(safe_fs::execute(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).is_err());
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "occupied");
    assert!(task.root.join("a.txt").exists());
}
#[test]
fn execute_then_restore_preserves_content_and_writes_intent_first() {
    let (mut task, store) = fixture();
    approved(&mut task);
    let paths: Vec<_> = task
        .entries
        .iter()
        .filter(|e| !e.is_dir())
        .map(|e| (e.id.clone(), std::fs::read(task.root.join(&e.id)).unwrap()))
        .collect();
    safe_fs::execute(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).unwrap();
    assert_eq!(task.status, "completed");
    let events = store.events(task.id).unwrap();
    let intent = events
        .iter()
        .position(|e| e["kind"] == "move_intent")
        .unwrap();
    let done = events
        .iter()
        .position(|e| e["kind"] == "move_done")
        .unwrap();
    assert!(intent < done);
    assert!(task
        .operations
        .iter()
        .filter(|o| o.status == "done")
        .all(|o| o.fingerprint.as_deref().unwrap().starts_with("blake3")));
    for event in events.iter().filter(|e| e["kind"] == "move_intent") {
        let detail = &event["detail"];
        let algorithm = detail["fingerprint_algorithm"].as_str().unwrap();
        assert!(matches!(algorithm, "blake3" | "blake3-adaptive-v2"));
        assert!(detail["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{algorithm}:")));
    }
    assert!(events[intent]["detail"].get("sha256").is_none());
    safe_fs::rollback(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).unwrap();
    assert_eq!(task.status, "rolled_back");
    for (path, bytes) in paths {
        assert_eq!(std::fs::read(task.root.join(path)).unwrap(), bytes);
    }
}
#[test]
fn restore_conflict_or_modified_file_is_not_reported_as_success() {
    let (mut task, store) = fixture();
    approved(&mut task);
    safe_fs::execute(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).unwrap();
    let op = task
        .operations
        .iter()
        .find(|o| o.source == "a.txt")
        .unwrap()
        .clone();
    std::fs::write(task.root.join(&op.destination), "user edited").unwrap();
    safe_fs::rollback(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).unwrap();
    assert_eq!(task.status, "partial");
    assert_eq!(
        std::fs::read_to_string(task.root.join(op.destination)).unwrap(),
        "user edited"
    );
}
#[test]
fn changed_source_and_nested_atomic_contents_block_execution() {
    let (mut task, store) = fixture();
    approved(&mut task);
    std::fs::write(task.root.join("Portable/config/new.json"), "new").unwrap();
    assert!(safe_fs::execute(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).is_err());
    assert!(task.root.join("Portable/editor.exe").exists());
}
#[test]
fn recovered_intent_has_no_duplicate_move() {
    let (mut task, store) = fixture();
    approved(&mut task);
    let i = task
        .operations
        .iter()
        .position(|o| o.source == "a.txt")
        .unwrap();
    let op = task.operations[i].clone();
    task.operations[i].fingerprint =
        Some(safe_fs::fingerprint(&task.root.join(&op.source), &CancellationToken::new()).unwrap());
    task.operations[i].status = "moving".into();
    task.status = "executing".into();
    store.save(&task).unwrap();
    let dst = task.root.join(&op.destination);
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    safe_fs::move_noreplace(&task.root.join(&op.source), &dst).unwrap();
    safe_fs::recover(&mut task, &store).unwrap();
    assert_eq!(task.operations[i].status, "done");
    assert_eq!(task.status, "recovery_required");
    assert!(dst.exists());
}

#[test]
fn legacy_sha256_and_new_blake3_operations_can_restore_in_one_task() {
    use sha2::{Digest, Sha256};
    let (mut task, store) = fixture();
    // Large files in the same task exercise the sampled policy alongside both older formats.
    for path in ["large.mp4", "Portable/large.bin"] {
        std::fs::File::create(task.root.join(path))
            .unwrap()
            .set_len(2 * 1024 * 1024)
            .unwrap();
    }
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    approved(&mut task);
    let legacy = format!(
        "{:x}",
        Sha256::digest(std::fs::read(task.root.join("a.txt")).unwrap())
    );
    safe_fs::execute(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).unwrap();
    task.operations
        .iter_mut()
        .find(|o| o.source == "a.txt")
        .unwrap()
        .fingerprint = Some(legacy);
    store.save(&task).unwrap();
    let mut loaded = store.load(task.id).unwrap();
    assert!(loaded.operations.iter().any(|o| o
        .fingerprint
        .as_deref()
        .unwrap_or("")
        .starts_with("blake3-adaptive-v2:")));
    safe_fs::rollback(
        &mut loaded,
        &store,
        &CancellationToken::new(),
        &|_, _, _| {},
    )
    .unwrap();
    assert_eq!(loaded.status, "rolled_back");
    assert_eq!(
        std::fs::read(task.root.join("a.txt")).unwrap(),
        b"private content"
    );
    assert!(task.root.join("Portable/config/options.json").is_file());
    assert_eq!(
        task.root.join("large.mp4").metadata().unwrap().len(),
        2 * 1024 * 1024
    );
    assert_eq!(
        task.root
            .join("Portable/large.bin")
            .metadata()
            .unwrap()
            .len(),
        2 * 1024 * 1024
    );
}

#[test]
fn sampled_move_intent_recovers_and_modified_sample_blocks_rollback() {
    use std::io::{Seek, SeekFrom, Write};
    let (mut task, store) = fixture();
    let path = task.root.join("large.mp4");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(2 * 1024 * 1024)
        .unwrap();
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    approved(&mut task);
    let i = task
        .operations
        .iter()
        .position(|o| o.source == "large.mp4")
        .unwrap();
    let op = task.operations[i].clone();
    task.operations[i].fingerprint =
        Some(safe_fs::fingerprint(&path, &CancellationToken::new()).unwrap());
    task.operations[i].status = "moving".into();
    task.status = "executing".into();
    store.save(&task).unwrap();
    let dst = task.root.join(&op.destination);
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    safe_fs::move_noreplace(&path, &dst).unwrap();
    let mut task = store.load(task.id).unwrap();
    safe_fs::recover(&mut task, &store).unwrap();
    assert_eq!(task.operations[i].status, "done");
    let mut file = std::fs::OpenOptions::new().write(true).open(&dst).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    file.seek(SeekFrom::Start(31)).unwrap();
    file.write_all(&[99]).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    drop(file);
    safe_fs::rollback(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).unwrap();
    assert_eq!(task.status, "partial");
    assert!(!path.exists());
    assert!(dst.exists());
    assert!(task.operations[i]
        .error
        .as_deref()
        .unwrap()
        .contains("修改"));
}

#[test]
fn startup_recovers_legacy_move_and_restore_intents() {
    use sha2::{Digest, Sha256};
    for restoring in [false, true] {
        let (mut task, store) = fixture();
        approved(&mut task);
        let i = task
            .operations
            .iter()
            .position(|o| o.source == "a.txt")
            .unwrap();
        let op = task.operations[i].clone();
        task.operations[i].fingerprint = Some(format!("{:x}", Sha256::digest(b"private content")));
        task.operations[i].status = if restoring { "restoring" } else { "moving" }.into();
        task.status = "executing".into();
        if !restoring {
            let destination = task.root.join(&op.destination);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            safe_fs::move_noreplace(&task.root.join(&op.source), &destination).unwrap();
        }
        store.save(&task).unwrap();
        let mut loaded = store.load(task.id).unwrap();
        safe_fs::recover(&mut loaded, &store).unwrap();
        assert_eq!(
            loaded.operations[i].status,
            if restoring { "restored" } else { "done" }
        );
        assert_eq!(loaded.status, "recovery_required");
    }
}
#[test]
fn paths_and_atomic_descendants_cannot_be_injected() {
    let (mut task, _) = fixture();
    planned(&mut task);
    for bad in [
        "../outside",
        "/absolute",
        "C:/outside",
        "a/../x",
        "a\\b",
        "CON",
    ] {
        assert!(safe_fs::checked_path(&task.root, bad).is_err());
    }
    task.operations[0].source = "Portable/editor.exe".into();
    assert!(safe_fs::validate_operations(&task).is_err());
}
#[test]
fn stale_or_hidden_scene_proposals_cannot_merge() {
    let (mut task, _) = fixture();
    task.phase = 2;
    let node = task.nodes[0].clone();
    let mut after = node.clone();
    after.note = "AI advice".into();
    let p = Proposal {
        id: "p".into(),
        scene: "tree".into(),
        revision: task.revision,
        message: "proposal".into(),
        changes: vec![Change {
            id: "change".into(),
            kind: "node".into(),
            target: node.id.clone(),
            label: node.name.clone(),
            before: json!(node),
            after: json!(after),
        }],
    };
    task.proposal = Some(p.clone());
    assert!(task
        .apply_proposal("p", &["change".into()], "permissions")
        .is_err());
    task.revision += 1;
    assert!(task
        .apply_proposal("p", &["change".into()], "tree")
        .is_err());
    task.revision -= 1;
    task.apply_proposal("p", &["change".into()], "tree")
        .unwrap();
    assert_eq!(task.nodes[0].note, "AI advice");
}

#[test]
fn graph_proposal_reparenting_survives_parent_deletion_in_any_order() {
    for reverse in [false, true] {
        let (mut task, _) = fixture();
        task.phase = 2;
        let mut parent = orphan("old-parent", Some("video-0"));
        parent.rule_type = RuleType::Complex;
        let mut child = orphan("kept-child", Some("old-parent"));
        child.rule_type = RuleType::Complex;
        task.nodes.extend([parent.clone(), child.clone()]);
        let mut moved = child.clone();
        moved.parent = Some("video-0".into());
        let mut changes = vec![
            Change {
                id: "move".into(),
                kind: "node".into(),
                target: child.id.clone(),
                label: child.name.clone(),
                before: json!(child),
                after: json!(moved),
            },
            Change {
                id: "delete".into(),
                kind: "node".into(),
                target: parent.id.clone(),
                label: parent.name.clone(),
                before: json!(parent),
                after: serde_json::Value::Null,
            },
        ];
        if reverse {
            changes.reverse();
        }
        task.proposal = Some(Proposal {
            id: "reparent".into(),
            scene: "tree".into(),
            revision: task.revision,
            message: "reparent then delete".into(),
            changes,
        });
        task.apply_proposal("reparent", &["move".into(), "delete".into()], "tree")
            .unwrap();
        assert_eq!(
            task.nodes
                .iter()
                .find(|n| n.id == "kept-child")
                .unwrap()
                .parent
                .as_deref(),
            Some("video-0")
        );
        assert!(!task.nodes.iter().any(|n| n.id == "old-parent"));
    }
}
#[test]
fn cancelled_scan_does_not_replace_saved_snapshot() {
    let (mut task, _) = fixture();
    let snapshot = task.entries.len();
    let token = CancellationToken::new();
    token.cancel();
    assert!(task.scan(&token, &|_, _, _| {}).is_err());
    assert_eq!(task.entries.len(), snapshot);
}

#[test]
fn rescan_preserves_user_tree_and_directory_overrides() {
    let (mut task, _) = fixture();
    task.nodes[0].note = "my custom rule".into();
    task.nodes[0].examples = vec!["a.txt".into()];
    task.set_directory("Portable", DirClass::Normal).unwrap();
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    assert_eq!(task.nodes[0].note, "my custom rule");
    assert_eq!(task.nodes[0].examples, vec!["a.txt"]);
    assert_eq!(
        task.entries
            .iter()
            .find(|e| e.id == "Portable")
            .unwrap()
            .class,
        DirClass::Normal
    );
}
#[test]
fn changed_file_cannot_be_sent_using_stale_privacy_size() {
    let (mut task, _) = fixture();
    task.permissions.default = AccessTier::ContentSlice;
    let entry = task
        .entries
        .iter()
        .find(|e| e.id == "a.txt")
        .unwrap()
        .clone();
    std::fs::write(
        task.root.join("a.txt"),
        "new private content beyond the original threshold",
    )
    .unwrap();
    assert!(workflow_ai::file_context(&task, &entry).is_err());
}
#[test]
fn second_process_cannot_open_active_task_store() {
    let (_, store) = fixture();
    assert!(TaskStore::new(store.directory.clone()).is_err());
    let directory = store.directory.clone();
    let clone = store.clone();
    drop(store);
    assert!(TaskStore::new(directory.clone()).is_err());
    drop(clone);
    assert!(TaskStore::new(directory).is_ok());
}
#[test]
fn cancelling_execution_retains_completed_moves_for_rollback() {
    let (mut task, store) = fixture();
    approved(&mut task);
    let token = CancellationToken::new();
    safe_fs::execute(&mut task, &store, &token, &|done, _, _| {
        if done == 1 {
            token.cancel();
        }
    })
    .unwrap();
    assert_eq!(task.status, "partial");
    assert_eq!(
        task.operations
            .iter()
            .filter(|o| o.status == "done")
            .count(),
        1
    );
    safe_fs::rollback(&mut task, &store, &CancellationToken::new(), &|_, _, _| {}).unwrap();
    assert_eq!(task.status, "rolled_back");
    assert!(task.root.join("a.txt").exists());
    assert!(task.root.join("Portable/editor.exe").exists());
}

#[test]
fn deep_semantic_branches_cannot_bypass_intermediate_extension_rules() {
    let (mut task, _) = fixture();
    let mut formats = orphan("only-mkv", Some("video-0"));
    formats.extensions = vec!["mkv".into()];
    let mut semantic = orphan("foreign", Some("only-mkv"));
    semantic.rule_type = RuleType::Complex;
    task.nodes.extend([formats, semantic]);
    task.validate_graph(true).unwrap();
    let movie = task.entries.iter().find(|e| e.id == "movie.mp4").unwrap();
    assert!(!task
        .semantic_options(movie)
        .iter()
        .any(|n| n.id == "foreign"));
    assert!(task
        .semantic_options(movie)
        .iter()
        .any(|n| n.id == "video-0"));
    assert_eq!(
        task.descend_simple("video-0", "mkv").unwrap().id,
        "only-mkv"
    );
    assert_eq!(task.descend_simple("video-0", "mp4").unwrap().id, "video-0");
}

#[test]
fn actual_directory_scene_cannot_modify_hidden_target_nodes() {
    let (mut task, _) = fixture();
    task.phase = 2;
    let node = task.nodes[0].clone();
    let mut after = node.clone();
    after.note = "hidden change".into();
    task.proposal = Some(Proposal {
        id: "scene-boundary".into(),
        scene: "directories".into(),
        revision: task.revision,
        message: "bad change".into(),
        changes: vec![Change {
            id: "x".into(),
            kind: "node".into(),
            target: node.id.clone(),
            label: node.name.clone(),
            before: json!(node),
            after: json!(after),
        }],
    });
    assert!(task
        .apply_proposal("scene-boundary", &["x".into()], "directories")
        .is_err());
    assert_eq!(task.nodes[0].note, node.note);
}
