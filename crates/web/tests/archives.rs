mod support;
use serde_json::{json, Value};
use std::{
    fs::File,
    time::{Duration, SystemTime},
};
use support::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn complete_history_archive_integrity_and_private_cleanup_batches() {
    let s = Server::new("fixture-model").await;
    s.configure(json!({"context_length":128000,"max_output_tokens":4096}))
        .await;
    let root = s.root("Downloads");
    let names: Vec<_> = (0..65)
        .map(|i| format!("installer-{i:02}.msi"))
        .chain([
            "private.zip".into(),
            "Portable/app.exe".into(),
            "Portable/cache.log".into(),
        ])
        .collect();
    for n in names {
        let p = write(&root, &n, "BODY_MUST_STAY_LOCAL");
        File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(400 * 86400))
            .unwrap();
    }
    let before = hashes(&root);
    let mut t = s.step(&s.scan(&root, "organize").await).await;
    t=s.post("permissions",&t,json!({"permissions":{"default":"none","content_slice_bytes":0,"rules":[{"extensions":["msi"],"tier":"filename_only"}]}})).await;
    t = s
        .post("directory", &t, json!({"id":"Portable","class":"atomic"}))
        .await;
    t["search_calls"] =
        json!([{"roundtrip":{"large_integer":u64::MAX,"whole_float":1.0,"negative_zero":-0.0}}]);
    t["messages"]=json!((0..40).map(|i|json!({"role":if i%2==0{"user"}else{"assistant"},"scene":"permissions","content":format!("history-marker-{i:02}")})).collect::<Vec<_>>());
    t = s.post("import", &Value::Null, json!({"task":t})).await;
    t = s.run("scan", &t, json!({})).await;
    t = s.step(&t).await;
    t = s
        .run(
            "chat",
            &t,
            json!({"scene":"permissions","message":"请继续"}),
        )
        .await;
    let sent = s.mock.bodies().pop().unwrap().to_string();
    assert!(sent.contains("history-marker-00") && sent.contains("history-marker-39"));
    assert_eq!(t["chat_context"]["included"], 40);
    let slow = s
        .post(
            "chat",
            &t,
            json!({"scene":"permissions","message":"slow-archive-check"}),
        )
        .await;
    assert_eq!(
        s.client
            .get(format!("{}/api/tasks/{}/archive", s.base, text(&t["id"])))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    t = s.wait(slow, "completed").await;
    t = s.step(&t).await;
    t = s.step(&t).await;
    t = s.approve(&t).await;
    t = s.run("execute", &t, json!({})).await;
    assert_eq!(t["status"], "completed");
    assert!(!array(&t["cleanup"])
        .iter()
        .any(|c| text(&c["original_id"]).starts_with("Portable/")));
    let count = s.mock.bodies().len();
    t = s.run("cleanup_ai", &t, json!({})).await;
    let all = s.mock.bodies();
    let batches = &all[count..];
    assert_eq!(batches.len(), 3);
    for r in batches {
        let items = mock::context(&array(&r["messages"]).last().unwrap()["content"]);
        assert!(array(&items).len() <= 32);
        for c in array(&items) {
            assert_eq!(c.as_object().unwrap().len(), 2);
            assert!(c.get("index").is_some());
            assert_eq!(
                c["file"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["extension", "name"]
            );
        }
    }
    let wire = json!(batches).to_string();
    for private in ["private.zip", "BODY_MUST_STAY_LOCAL", "cache.log"] {
        assert!(!wire.contains(private));
    }
    assert_eq!(
        array(&t["cleanup"])
            .iter()
            .filter(|c| c["source"] == "ai")
            .count(),
        65
    );
    let archive = s
        .get(&format!("/api/tasks/{}/archive", text(&t["id"])))
        .await;
    let payload: Value = serde_json::from_str(text(&archive["payload"])).unwrap();
    assert_eq!(payload["task"], t);
    assert_eq!(payload["trajectory"], s.events(&t).await);
    assert_eq!(
        payload["task"]["search_calls"][0]["roundtrip"]["large_integer"],
        u64::MAX
    );
    let saved = s
        .post("import", &Value::Null, json!({"task":archive}))
        .await;
    let id = text(&saved["archive_id"]);
    let url = format!("/api/archives/{id}");
    assert_eq!(s.get(&url).await, archive);
    assert!(!array(&s.get("/api/tasks").await)
        .iter()
        .any(|t| t["id"] == id));
    let mut bad = archive.clone();
    bad["payload"] = json!(text(&archive["payload"]).replace("history-marker-00", "tampered"));
    s.blocked("import", &Value::Null, json!({"task":bad})).await;
    let resumed = s
        .post(
            "resume_archive",
            &Value::Null,
            json!({"archive_id":id,"root":root}),
        )
        .await;
    assert_eq!(resumed["scanned"], false);
    assert!(array(&resumed["operations"]).is_empty());
    assert_eq!(resumed["messages"], t["messages"]);
    assert_eq!(s.get(&url).await, archive);
    let mut after: Vec<_> = hashes(&root).into_values().collect();
    let mut before: Vec<_> = before.into_values().collect();
    after.sort();
    before.sort();
    assert_eq!(after, before);
}
