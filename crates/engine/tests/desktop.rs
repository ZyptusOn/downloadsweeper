use ds_engine::{
    domain::DirClass,
    permission::PermissionConfig,
    safe_fs::{self, TaskStore},
    workflow::Task,
};
use tokio_util::sync::CancellationToken;

struct Desktop {
    task: Task,
    store: TaskStore,
    base: std::path::PathBuf,
}
impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
fn fixture() -> Desktop {
    let base = std::env::temp_dir().join(format!("ds-desktop-{}", uuid::Uuid::new_v4()));
    let root = base.join("Desktop");
    std::fs::create_dir_all(root.join("项目/内部")).unwrap();
    std::fs::create_dir_all(root.join("文本")).unwrap();
    for (name, data) in [
        ("note.txt", "note"),
        ("report.docx", "report"),
        ("paper.pdf", "pdf"),
        ("photo.png", "image"),
        ("app.lnk", "shortcut"),
        ("desktop.ini", "system"),
        ("~$report.docx", "lock"),
        (".hidden", "hidden"),
        ("项目/内部/private.txt", "private"),
        ("文本/old.txt", "existing"),
    ] {
        std::fs::write(root.join(name), data).unwrap();
    }
    let task = Task::new(root, "desktop", PermissionConfig::default()).unwrap();
    let store = TaskStore::new(base.join("state")).unwrap();
    Desktop { task, store, base }
}

#[test]
fn desktop_shallow_plan_review_execute_restore_preserves_folders() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    f.task.scan(&c, &p).unwrap();
    assert!(f.task.entries.iter().all(|e| e.parent.is_empty()));
    assert!(f
        .task
        .entries
        .iter()
        .filter(|e| e.is_dir())
        .all(|e| e.class == DirClass::Atomic));
    assert!(f.task.set_directory("项目", DirClass::Normal).is_err());
    f.task.prepare_desktop(&c, &p).unwrap();
    assert_eq!(f.task.phase, 4);
    assert!(f.task.calls.is_empty());
    assert_eq!(f.task.operations.len(), 4);
    assert_eq!(f.task.retained.len(), 6);
    assert!(f
        .task
        .operations
        .iter()
        .any(|o| o.destination == "文本 (2)/note.txt"));
    assert!(safe_fs::execute(&mut f.task, &f.store, &c, &p).is_err());
    f.task.reviewed = true;
    f.task.advance().unwrap();
    safe_fs::execute(&mut f.task, &f.store, &c, &p).unwrap();
    assert_eq!(f.task.status, "completed");
    for name in [
        "项目/内部/private.txt",
        "文本/old.txt",
        "app.lnk",
        "desktop.ini",
        "~$report.docx",
        ".hidden",
    ] {
        assert!(f.task.root.join(name).is_file());
    }
    safe_fs::rollback(&mut f.task, &f.store, &c, &p).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.task.root.join("note.txt")).unwrap(),
        "note"
    );
}

#[test]
fn desktop_cancel_does_not_commit_partial_plan_and_tampered_folder_moves_fail() {
    let mut f = fixture();
    f.task
        .scan(&CancellationToken::new(), &|_, _, _| {})
        .unwrap();
    let before = serde_json::to_value(&f.task).unwrap();
    let c = CancellationToken::new();
    c.cancel();
    assert!(f.task.prepare_desktop(&c, &|_, _, _| {}).is_err());
    assert_eq!(serde_json::to_value(&f.task).unwrap(), before);
    f.task
        .prepare_desktop(&CancellationToken::new(), &|_, _, _| {})
        .unwrap();
    let op = &mut f.task.operations[0];
    op.source = "项目".into();
    op.kind = "directory".into();
    assert!(safe_fs::validate_operations(&f.task).is_err());
}

#[test]
fn desktop_six_stages_preserve_custom_tree_and_legacy_shortcut() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    f.task.scan(&c, &p).unwrap();
    assert!(f.task.is_organizing());
    f.task.advance().unwrap();
    assert_eq!(f.task.phase, 1);
    f.task.advance().unwrap();
    assert_eq!(f.task.phase, 2);
    let text = f
        .task
        .nodes
        .iter_mut()
        .find(|n| n.extensions.contains(&"txt".into()))
        .unwrap();
    text.name = "自定义文本".into();
    text.note = "根据项目主题进一步分类".into();
    let edited = serde_json::to_value(&f.task.nodes).unwrap();
    f.task.advance().unwrap();
    f.task.generate_rules(&c, &p).unwrap();
    assert_eq!(f.task.phase, 3);
    assert!(f
        .task
        .operations
        .iter()
        .any(|o| o.destination == "自定义文本/note.txt"));
    f.task.advance().unwrap();
    f.task.go_back(2).unwrap();
    f.task.prepare_desktop(&c, &p).unwrap();
    assert_eq!(serde_json::to_value(&f.task.nodes).unwrap(), edited);
}

#[tokio::test]
async fn desktop_ai_readiness_and_empty_run_preserve_the_rule_plan() {
    use ds_engine::{config::AppConfig, permission::AccessTier, tree::RuleType, workflow_ai};
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    f.task.scan(&c, &p).unwrap();
    f.task.permissions.default = AccessTier::FilenameOnly;
    f.task.permissions.rules.clear();
    let report = workflow_ai::classification_readiness(&f.task);
    assert_eq!(report.eligible_files, 6);
    assert_eq!(report.protected_entries, 4);
    assert!(f.task.nodes.iter().filter(|n| n.parent.as_deref() == Some("root"))
        .all(|n| n.rule_type == RuleType::Simple));

    f.task.permissions.rules.push(ds_engine::permission::PermissionRule { extensions: vec!["@folder".into()], tier: AccessTier::None, ..Default::default() });
    // Reproduce an existing desktop task created with the old, simple-only template.
    f.task.nodes.retain(|n| n.rule_type == RuleType::Simple);
    f.task.prepare_desktop(&c, &p).unwrap();
    let before = serde_json::to_value(&f.task).unwrap();
    let report = workflow_ai::classification_readiness(&f.task);
    assert_eq!(report.eligible_files, 0);
    assert_eq!(report.no_semantic_rule, 4);
    let error = workflow_ai::refine(&mut f.task, &AppConfig::default(), &f.store, &c, &p)
        .await.unwrap_err();
    assert!(error.to_string().contains("未调用 AI"));
    assert_eq!(serde_json::to_value(&f.task).unwrap(), before);
    assert!(f.task.calls.is_empty());
    assert!(f.task.classification.is_none());
    assert_eq!(f.task.plan_source.as_deref(), Some("rules"));

    f.task.permissions.default = AccessTier::None;
    let report = workflow_ai::classification_readiness(&f.task);
    assert_eq!(report.permission_denied, 6);
    assert_eq!(report.no_semantic_rule, 0);
}

#[test]
fn desktop_container_mapping_preserves_contents_rescan_and_undo() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    std::fs::write(f.task.root.join("文本/note.txt"), "existing collision").unwrap();
    f.task.scan(&c, &p).unwrap();
    f.task.advance().unwrap();
    let before_entries = f.task.entries.len();
    f.task
        .set_directory("文本", DirClass::Container)
        .unwrap();
    assert_eq!(
        f.task.entries.len(),
        before_entries,
        "Changing type must not rescan"
    );
    assert!(f.task.set_directory("文本", DirClass::Normal).is_err());
    f.task.advance().unwrap();
    f.task
        .nodes
        .iter_mut()
        .find(|n| n.extensions.contains(&"txt".into()))
        .unwrap()
        .mapping = Some("文本".into());
    f.task.validate_graph(true).unwrap();
    f.task.scan(&c, &p).unwrap();
    assert_eq!(
        f.task
            .entries
            .iter()
            .find(|e| e.id == "文本")
            .unwrap()
            .class,
        DirClass::Container
    );
    assert!(f.task.entries.iter().all(|e| e.parent.is_empty()));
    assert!(f
        .task
        .nodes
        .iter()
        .any(|n| n.mapping.as_deref() == Some("文本")));
    f.task.advance().unwrap();
    f.task.advance().unwrap();
    f.task.advance().unwrap();
    f.task.generate_rules(&c, &p).unwrap();
    let destination = f
        .task
        .operations
        .iter()
        .find(|o| o.source == "note.txt")
        .unwrap()
        .destination
        .clone();
    assert!(destination.starts_with("文本/") && destination != "文本/note.txt");
    f.task.advance().unwrap();
    f.task.reviewed = true;
    f.task.advance().unwrap();
    safe_fs::execute(&mut f.task, &f.store, &c, &p).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.task.root.join(&destination)).unwrap(),
        "note"
    );
    assert_eq!(
        std::fs::read_to_string(f.task.root.join("文本/note.txt")).unwrap(),
        "existing collision"
    );
    safe_fs::rollback(&mut f.task, &f.store, &c, &p).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.task.root.join("note.txt")).unwrap(),
        "note"
    );
    assert_eq!(
        std::fs::read_to_string(f.task.root.join("文本/old.txt")).unwrap(),
        "existing"
    );
}

#[test]
fn switching_desktop_container_back_to_atomic_clears_mapping() {
    let mut f = fixture();
    f.task
        .scan(&CancellationToken::new(), &|_, _, _| {})
        .unwrap();
    f.task
        .set_directory("文本", DirClass::Container)
        .unwrap();
    f.task.nodes[0].mapping = Some("文本".into());
    f.task.set_directory("文本", DirClass::Atomic).unwrap();
    assert!(f.task.nodes.iter().all(|n| n.mapping.is_none()));
}
