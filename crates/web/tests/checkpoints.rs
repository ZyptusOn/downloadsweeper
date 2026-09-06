mod support;
use serde_json::{json, Value};
use std::time::Duration;
use support::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn process_death_durable_pause_resume_stale_gates_and_archive_jobs() {
    let mut s = Server::new("checkpoint-fixture").await;
    let root = s.root("Downloads");
    for i in 0..6000 {
        write(&root, &format!("file-{i:05}.txt"), i.to_string());
    }
    let before = hashes(&root);
    let mut t = s.post("create", &Value::Null, json!({"root":root})).await;
    let job = s.act("scan", &t).await;
    s.post("cancel", &Value::Null, json!({"job_id":job["job"]["id"]}))
        .await;
    t = s.wait(job.clone(), "paused").await;
    assert!(array(&t["entries"]).is_empty());
    assert_eq!(t["scanned"], false);
    assert_eq!(s.get("/api/jobs").await[0]["resumable"], true);
    t = s.act("dismiss_proposal", &t).await;
    let error = s
        .blocked(
            "resume_job",
            &Value::Null,
            json!({"job_id":job["job"]["id"]}),
        )
        .await;
    assert!(text(&error["error"]).contains("已改变"));
    assert_eq!(
        array(&s.get("/api/jobs").await)
            .iter()
            .find(|j| j["id"] == job["job"]["id"])
            .unwrap()["resumable"],
        false
    );
    let interrupted = s.act("scan", &t).await;
    s.restart().await;
    let jobs = s.get("/api/jobs").await;
    let recovered = array(&jobs)
        .iter()
        .find(|j| j["id"] == interrupted["job"]["id"])
        .unwrap();
    assert_eq!(recovered["status"], "interrupted");
    assert_eq!(recovered["resumable"], true);
    let continued = s
        .post(
            "resume_job",
            &Value::Null,
            json!({"job_id":recovered["id"]}),
        )
        .await;
    assert_eq!(continued["job"]["id"], interrupted["job"]["id"]);
    t = s.wait(continued, "completed").await;
    assert_eq!(array(&t["entries"]).len(), 6000);
    let job = s.act("test_connection", &t).await;
    tokio::time::sleep(Duration::from_millis(3200)).await;
    let running = s.get("/api/bootstrap").await["job"].clone();
    assert!(running.is_object());
    assert_ne!(running["saved_at"], job["job"]["saved_at"]);
    s.post("cancel", &Value::Null, json!({"job_id":job["job"]["id"]}))
        .await;
    t = s.wait(job.clone(), "paused").await;
    assert_eq!(
        s.get("/api/bootstrap").await["last_job"]["resumable"],
        false
    );
    assert_eq!(array(&t["pending_calls"]).len(), 1);
    let count = s.mock.bodies().len();
    s.restart().await;
    assert_eq!(
        array(&s.get("/api/jobs").await)
            .iter()
            .find(|j| j["id"] == job["job"]["id"])
            .unwrap()["resumable"],
        false
    );
    s.blocked(
        "resume_job",
        &Value::Null,
        json!({"job_id":job["job"]["id"]}),
    )
    .await;
    assert_eq!(s.mock.bodies().len(), count);
    t = s.current(&t).await;
    let export = s.act("archive_export", &t).await;
    s.wait(export.clone(), "completed").await;
    let url = format!("/api/jobs/{}/result", text(&export["job"]["id"]));
    let archive = s.get(&url).await;
    let payload: Value = serde_json::from_str(text(&archive["payload"])).unwrap();
    assert_eq!(payload["task"], t);
    let import = s
        .post("archive_import", &Value::Null, json!({"task":archive}))
        .await;
    s.wait(import.clone(), "completed").await;
    let result = s
        .get(&format!("/api/jobs/{}/result", text(&import["job"]["id"])))
        .await;
    assert_eq!(result["archive_id"], import["job"]["id"]);
    assert_eq!(
        s.get(&format!("/api/archives/{}", text(&result["archive_id"])))
            .await["payload"],
        archive["payload"]
    );
    assert_eq!(array(&s.get("/api/tasks").await).len(), 1);
    s.restart().await;
    assert_eq!(s.get(&url).await["checksum"], archive["checksum"]);
    assert_eq!(s.mock.bodies().len(), count);
    assert_eq!(hashes(&root), before);
}
