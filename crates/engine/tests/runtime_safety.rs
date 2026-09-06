use ds_engine::{
    permission::{AccessTier, PermissionConfig},
    safe_fs,
    tools::{Tool, ToolContext, ToolRegistry},
    trajectory::TrajectoryLogger,
};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn legacy_read_tools_cannot_escape_or_expose_denied_names() {
    let root = std::env::temp_dir().join(format!("ds-runtime-safety-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("allowed")).unwrap();
    std::fs::write(root.join("allowed/private.txt"), "private contents").unwrap();
    std::fs::write(root.join("outside.txt"), "outside content").unwrap();
    let mut context = ToolContext {
        scan_root: root.join("allowed"),
        permissions: PermissionConfig {
            default: AccessTier::None,
            rules: vec![],
            content_slice_bytes: 64,
        },
        trajectory: Arc::new(TrajectoryLogger::open(&root.join("events.jsonl")).unwrap()),
    };
    let scan = ds_engine::tools::fs_tools::ScanDownloadsTool
        .call(json!({}), &context, &CancellationToken::new())
        .await
        .unwrap();
    assert!(!scan.to_string().contains("private.txt"));
    context.permissions.default = AccessTier::ContentSlice;
    let read = ds_engine::tools::fs_tools::ReadContentSliceTool;
    for path in [
        root.join("outside.txt").to_string_lossy().to_string(),
        "../outside.txt".into(),
    ] {
        let result = read
            .call(json!({"path":path}), &context, &CancellationToken::new())
            .await
            .unwrap();
        assert!(result.get("error").is_some());
        assert!(!result.to_string().contains("outside content"));
    }
    assert!(!ToolRegistry::with_defaults()
        .names()
        .iter()
        .any(|n| n == "move_batch" || n == "trash_batch"));
}

#[test]
fn evidence_checks_snapshot_and_legacy_moves_never_overwrite() {
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
        ds_engine::tools::organize::perform_move(&root.join("a.txt"), &root.join("b.txt")).is_err()
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
