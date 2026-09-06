mod support;
use futures::FutureExt;
use serde_json::json;
use support::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg(any(windows, target_os = "macos"))]
#[ignore = "explicit native recycle roundtrip; only self-created fixture files, never empties the bin"]
async fn recycle_confirmation_restart_restore_then_organization_undo() {
    let mut s = Server::new("local-test").await;
    let root = s.root("files");
    for name in ["临时缓存.log", "keep.txt"] {
        write(&root, name, format!("synthetic recycle roundtrip {name}"));
    }
    let before = hashes(&root);
    assert_eq!(s.config().await["recycle_supported"], true);
    let mut t = s.scan(&root, "desktop").await;
    // Catch failures so the harness restores only this test's recycle batches before unwinding.
    let result = std::panic::AssertUnwindSafe(async {
        t = s.run("desktop_plan", &t, json!({})).await;
        t = s.approve(&t).await;
        t = s.run("execute", &t, json!({})).await;
        let candidate = array(&t["cleanup"])
            .iter()
            .find(|c| c["original_id"] == "临时缓存.log")
            .unwrap()
            .clone();
        s.blocked("cleanup_trash", &t, json!({"selected":["临时缓存.log"]}))
            .await;
        t = s
            .run(
                "cleanup_trash",
                &t,
                json!({"selected":["临时缓存.log"],"confirmed":true}),
            )
            .await;
        let recycled = array(&t["recycled"]).last().unwrap();
        assert_eq!(recycled["status"], "trashed");
        let batch = recycled["batch"].clone();
        assert!(!root.join(text(&candidate["path"])).exists());
        t = s.wait(s.act("rollback", &t).await, "failed").await;
        assert_eq!(t["status"], "completed");
        s.restart().await;
        t = s.current(&t).await;
        assert_eq!(array(&t["recycled"]).last().unwrap()["batch"], batch);
        t = s.run("cleanup_restore", &t, json!({"batch":batch})).await;
        assert!(array(&t["recycled"])
            .iter()
            .all(|r| r["status"] == "restored"));
        assert!(root.join(text(&candidate["path"])).is_file());
        t = s.run("rollback", &t, json!({})).await;
        assert_eq!(t["status"], "rolled_back");
        assert!(array(&t["calls"]).is_empty());
        assert_eq!(hashes(&root), before);
    })
    .catch_unwind()
    .await;
    if let Err(error) = result {
        let recovery = std::panic::AssertUnwindSafe(async {
            let mut current = s.current(&t).await;
            let batches: std::collections::BTreeSet<_> = array(&current["recycled"])
                .iter()
                .filter(|r| r["status"] != "restored")
                .map(|r| text(&r["batch"]).to_owned())
                .collect();
            for batch in batches {
                current = s
                    .run("cleanup_restore", &current, json!({"batch":batch}))
                    .await;
            }
        })
        .catch_unwind()
        .await;
        if recovery.is_err() {
            let retained = std::mem::replace(&mut s.dir, tempfile::tempdir().unwrap()).keep();
            panic!(
                "Fixture recovery still pending; preserved test data at {}",
                retained.display()
            );
        }
        std::panic::resume_unwind(error);
    }
}
