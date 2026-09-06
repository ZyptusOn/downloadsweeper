mod support;
use serde_json::{json, Value};
use std::time::Instant;
use support::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_parallelism_large_context_rename_checkpoints_and_reservations() {
    let s = Server::new("runtime-fixture").await;
    s.configure(json!({"context_length":1000000,"max_output_tokens":64000,"parallel_requests":3}))
        .await;
    let root = s.root("files");
    for i in 0..120 {
        write(&root, &format!("{i:03}.txt"), "test evidence");
    }
    let before = hashes(&root);
    let mut t = s.planning(&root, "organize", Value::Null).await;
    s.mock.reset_maximum();
    let start = Instant::now();
    t = s.run("plan_ai", &t, json!({"batch_size":20})).await;
    let parallel = start.elapsed();
    assert!(s.mock.maximum() > 1 && s.mock.maximum() <= 3);
    assert_eq!(t["classification"]["completed"], 120);
    assert!(array(&t["pending_calls"]).is_empty());
    let ids: std::collections::HashSet<_> =
        array(&t["calls"]).iter().map(|c| text(&c["id"])).collect();
    assert_eq!(ids.len(), array(&t["calls"]).len());
    s.configure(json!({"parallel_requests":1})).await;
    let start = Instant::now();
    s.run("plan_ai", &t, json!({"batch_size":20})).await;
    let serial = start.elapsed();
    assert!(
        parallel.as_secs_f64() < serial.as_secs_f64() * 0.85,
        "parallel={parallel:?}, serial={serial:?}"
    );
    let long = s.root("long");
    for i in 0..240 {
        write(
            &long,
            &format!("{i}{}.txt", "文件描述".repeat(14)),
            "fixture",
        );
    }
    s.configure(json!({"parallel_requests":3})).await;
    let large = s
        .run(
            "plan_ai",
            &s.planning(&long, "organize", Value::Null).await,
            json!({"batch_size":512}),
        )
        .await;
    assert_eq!(array(&large["classification"]["batches"]).len(), 1);
    assert!(array(&s.events(&large).await).iter().any(
        |e| e["kind"] == "llm_request" && number(&e["detail"]["input_text_bytes"]) > 24 * 1024
    ));
    let mixed = s.root("mixed");
    for ext in ["txt", "mp4", "png"] {
        for i in 0..12 {
            write(&mixed, &format!("{i:03}.{ext}"), "fixture");
        }
    }
    let inspection = s.tree(&mixed, "organize", Value::Null).await;
    s.mock.reset_maximum();
    let inspection = s
        .run(
            "suggest_tree",
            &inspection,
            json!({"message":"检查并设计目录"}),
        )
        .await;
    assert_eq!(s.mock.maximum(), 3);
    assert_eq!(inspection["inspection"]["status"], "complete");
    assert_eq!(
        array(&inspection["inspection"]["groups"])
            .iter()
            .map(|g| number(&g["inspected"]))
            .sum::<u64>(),
        36
    );
    let small = s.root("rename");
    for i in 0..6 {
        write(&small, &format!("{i:03}.txt"), "fixture");
    }
    let mut r = s.tree(&small, "rename", Value::Null).await;
    r = s
        .post(
            "rename_scope",
            &r,
            json!({"extensions":["txt"],"web_search":false}),
        )
        .await;
    r = s.step(&r).await;
    let original = r["operations"].clone();
    s.configure(json!({"model":"runtime-rename-fail"})).await;
    r = s.wait(s.act("rename", &r).await, "failed").await;
    let saved = r["rename_checkpoint"]["results"]
        .as_object()
        .unwrap()
        .clone();
    assert!(!saved.is_empty() && saved.len() < 6);
    assert_eq!(r["operations"], original);
    let count = s.mock.bodies().len();
    r = s.run("rename", &r, json!({})).await;
    assert_eq!(s.mock.bodies().len() - count, 6 - saved.len());
    for (k, v) in saved {
        assert_eq!(r["rename_checkpoint"]["results"][k], v);
    }
    assert_eq!(r["rename_checkpoint"]["status"], "complete");
    assert_eq!(array(&r["operations"]).len(), 6);
    s.configure(json!({"model":"runtime-fixture","max_output_tokens":16000}))
        .await;
    s.budget(json!(20000)).await;
    let b = s
        .run(
            "plan_ai",
            &s.planning(&root, "organize", Value::Null).await,
            json!({"batch_size":20}),
        )
        .await;
    let ev = s.events(&b).await;
    let mut reserved = 0i64;
    for e in array(&ev) {
        if e["kind"] == "llm_request" {
            reserved += number(&e["detail"]["reserved_tokens"]) as i64;
        } else if e["kind"] == "llm_response" {
            let request = array(&ev)
                .iter()
                .find(|r| r["kind"] == "llm_request" && r["detail"]["id"] == e["detail"]["id"])
                .unwrap();
            reserved -= number(&request["detail"]["reserved_tokens"]) as i64;
            reserved += (number(&e["detail"]["usage"]["prompt_tokens"])
                + number(&e["detail"]["usage"]["completion_tokens"]))
                as i64;
        }
        assert!(reserved <= 20000);
    }
    assert!(array(&b["pending_calls"]).is_empty());
    s.configure(json!({"model":"runtime-timeout","request_timeout_seconds":1}))
        .await;
    let u = s
        .post(
            "create",
            &Value::Null,
            json!({"root":small,"mode":"organize"}),
        )
        .await;
    let u = s.wait(s.act("test_connection", &u).await, "failed").await;
    assert_eq!(array(&u["pending_calls"]).len(), 1);
    assert!(array(&u["calls"]).is_empty());
    let stored: Value = serde_json::from_slice(
        &std::fs::read(
            s.dir
                .path()
                .join("data/tasks")
                .join(text(&u["id"]))
                .join("task.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored["pending_calls"], u["pending_calls"]);
    s.configure(json!({"model":"runtime-denied"})).await;
    let rejected = s
        .post(
            "create",
            &Value::Null,
            json!({"root":small,"mode":"organize"}),
        )
        .await;
    let rejected = s
        .wait(s.act("test_connection", &rejected).await, "failed")
        .await;
    assert!(array(&rejected["pending_calls"]).is_empty() && array(&rejected["calls"]).is_empty());
    assert_eq!(hashes(&root), before);
    let imported = s.post("import", &Value::Null, json!({"task":r})).await;
    assert!(
        imported["rename_checkpoint"].is_null() && array(&imported["pending_calls"]).is_empty()
    );
    assert_eq!(imported["scanned"], false);
}
