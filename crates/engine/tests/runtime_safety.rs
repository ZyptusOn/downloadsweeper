use ds_engine::safe_fs;
use serde_json::json;

#[test]
fn workflow_evidence_hides_denied_names_and_enforces_path_and_slice_limits() {
    use ds_engine::{
        permission::{AccessTier, PermissionConfig},
        workflow::Task,
        workflow_ai,
    };
    use tokio_util::sync::CancellationToken;
    let base = std::env::temp_dir().join(format!("ds-evidence-boundary-{}", uuid::Uuid::new_v4()));
    let root = base.join("allowed");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("private.txt"), "private contents").unwrap();
    std::fs::write(base.join("outside.txt"), "outside contents").unwrap();
    let mut task = Task::new(
        root,
        "organize",
        PermissionConfig {
            default: AccessTier::None,
            rules: vec![],
            content_slice_bytes: 7,
        },
    )
    .unwrap();
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    let entry = task
        .entries
        .iter()
        .find(|e| e.name == "private.txt")
        .unwrap()
        .clone();
    assert!(workflow_ai::file_context_capped(&task, &entry, 65536)
        .unwrap()
        .is_none());
    task.permissions.default = AccessTier::FilenameOnly;
    let names = workflow_ai::file_context_capped(&task, &entry, 65536)
        .unwrap()
        .unwrap();
    assert_eq!(names["name"], "private.txt");
    assert!(names.get("text_excerpt").is_none());
    task.permissions.default = AccessTier::ContentSlice;
    assert_eq!(
        workflow_ai::file_context_capped(&task, &entry, 65536)
            .unwrap()
            .unwrap()["text_excerpt"],
        "private"
    );
    assert_eq!(
        workflow_ai::file_context_capped(&task, &entry, 3)
            .unwrap()
            .unwrap()["text_excerpt"],
        "pri"
    );
    for id in [
        "../outside.txt".to_owned(),
        base.join("outside.txt").to_string_lossy().into_owned(),
    ] {
        let mut injected = entry.clone();
        injected.id = id;
        assert!(workflow_ai::file_context_capped(&task, &injected, 65536).is_err());
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn evidence_checks_snapshot_and_moves_never_overwrite() {
    let root = std::env::temp_dir().join(format!("ds-evidence-safety-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"original").unwrap();
    std::fs::write(root.join("b.txt"), b"preserved").unwrap();
    let metadata = std::fs::metadata(root.join("a.txt")).unwrap();
    let modified = ds_engine::workflow::modified_ms(&metadata);
    assert!(safe_fs::open_evidence(&root, "a.txt", metadata.len(), modified).is_ok());
    assert!(safe_fs::open_evidence(&root, "a.txt", 99, modified).is_err());
    assert!(safe_fs::open_evidence(&root, "../a.txt", metadata.len(), modified).is_err());
    assert!(
        safe_fs::move_noreplace(&root.join("a.txt"), &root.join("b.txt")).is_err()
    );
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"original");
    assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"preserved");
}

#[test]
fn tool_policy_rejects_ungranted_names_duplicate_ids_and_non_object_args() {
    use ds_engine::{
        ai_runtime::validate_tool_turn,
        llm::{ToolCall, ToolDef},
    };
    let definitions = vec![ToolDef {
        name: "read_file_evidence".into(),
        description: "fixture".into(),
        parameters: json!({}),
    }];
    let valid = ToolCall {
        id: "call-1".into(),
        name: "read_file_evidence".into(),
        args: json!({"file_ids":["f0"]}),
    };
    assert!(validate_tool_turn(&[valid.clone()], &definitions).is_ok());
    assert!(validate_tool_turn(&[valid.clone(), valid.clone()], &definitions).is_err());
    assert!(validate_tool_turn(
        &[ToolCall {
            name: "execute".into(),
            ..valid.clone()
        }],
        &definitions
    )
    .is_err());
    assert!(validate_tool_turn(
        &[ToolCall {
            args: json!([]),
            ..valid
        }],
        &definitions
    )
    .is_err());
}
