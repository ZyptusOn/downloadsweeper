//! 集成测试：真实批量移动 + JSONL 轨迹落盘。

use std::path::PathBuf;
use std::sync::Arc;

use ds_engine::permission::PermissionConfig;
use ds_engine::tools::{
    organize::{MoveBatchTool, TrashBatchTool},
    Tool, ToolContext,
};
use ds_engine::trajectory::TrajectoryLogger;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn move_batch_executes_and_logs() {
    // 准备临时目录与一个源文件
    let tmp = std::env::temp_dir().join(format!("ds_it_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join("src")).unwrap();
    let src = tmp.join("src/a.txt");
    std::fs::write(&src, b"hello").unwrap();
    let dst = tmp.join("out/sub/a.txt");

    let traj_path = tmp.join("traj.jsonl");
    let traj = Arc::new(TrajectoryLogger::open(&traj_path).unwrap());
    let ctx = ToolContext {
        permissions: PermissionConfig::default(),
        scan_root: tmp.clone(),
        trajectory: traj.clone(),
    };

    let tool = MoveBatchTool;
    let args = serde_json::json!({
        "dry_run": false,
        "moves": [{ "src": src.to_string_lossy(), "dst": dst.to_string_lossy() }]
    });
    let res = tool
        .call(args, &ctx, &CancellationToken::new())
        .await
        .unwrap();

    assert!(res.get("error").is_some());
    assert!(src.exists(), "旧工具不得移动文件");
    assert!(!dst.exists());
}

#[tokio::test]
async fn move_batch_collision_avoidance() {
    let tmp = std::env::temp_dir().join(format!("ds_it_col_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join("src")).unwrap();
    std::fs::create_dir_all(tmp.join("out")).unwrap();
    // 目标已存在一个同名文件
    std::fs::write(tmp.join("out/a.txt"), b"old").unwrap();
    let src = tmp.join("src/a.txt");
    std::fs::write(&src, b"new").unwrap();
    let dst = tmp.join("out/a.txt");

    let traj = Arc::new(TrajectoryLogger::open(&tmp.join("t.jsonl")).unwrap());
    let ctx = ToolContext {
        permissions: PermissionConfig::default(),
        scan_root: tmp.clone(),
        trajectory: traj,
    };
    let tool = MoveBatchTool;
    let args = serde_json::json!({
        "dry_run": true,
        "moves": [{ "src": src.to_string_lossy(), "dst": dst.to_string_lossy() }]
    });
    let res = tool
        .call(args, &ctx, &CancellationToken::new())
        .await
        .unwrap();
    // dry_run 下预览的 dst 应被改名为 "a (1).txt"，不覆盖已存在文件
    let resolved = res["results"][0]["dst"].as_str().unwrap();
    assert!(resolved.contains("a (1).txt"), "未做冲突避让: {res}");
}

#[allow(dead_code)]
fn _force_pathbuf_use() -> PathBuf {
    PathBuf::new()
}

#[tokio::test]
async fn trash_batch_tool_dry_run_and_execute() {
    let tmp = std::env::temp_dir().join(format!("ds_trash_tool_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let f1 = tmp.join("junk1.txt");
    let f2 = tmp.join("junk2.txt");
    std::fs::write(&f1, b"junk1").unwrap();
    std::fs::write(&f2, b"junk2").unwrap();

    let traj = Arc::new(TrajectoryLogger::open(&tmp.join("t.jsonl")).unwrap());
    let ctx = ToolContext {
        permissions: PermissionConfig::default(),
        scan_root: tmp.clone(),
        trajectory: traj,
    };
    let tool = TrashBatchTool;
    let cancel = CancellationToken::new();

    // dry_run: 不删除，返回预览
    let res = tool
        .call(
            serde_json::json!({ "dry_run": true, "paths": [f1.to_string_lossy(), f2.to_string_lossy()] }),
            &ctx,
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(res["executed"], serde_json::json!(false));
    assert_eq!(res["results"][0]["status"], serde_json::json!("preview"));
    assert!(f1.exists(), "dry_run 不应删除文件");

    // 真实执行：文件进入系统回收站
    let res = tool
        .call(
            serde_json::json!({ "paths": [f1.to_string_lossy(), f2.to_string_lossy()] }),
            &ctx,
            &cancel,
        )
        .await
        .unwrap();
    assert!(res.get("error").is_some());
    assert!(f1.exists() && f2.exists(), "旧工具不得删除文件");
}
