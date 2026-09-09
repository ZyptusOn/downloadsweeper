mod support;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_keeps_unchanged_names_extensions_and_denied_files() {
    let s = Server::new("media-fixture").await;
    let root = s.root("rename-boundaries");
    write(&root, "report.pdf", "synthetic document");
    write(&root, "private.xlsx", "denied contents");
    let before = hashes(&root);
    let mut task = s
        .tree(
            &root,
            "rename",
            json!({"default":"filename_only",
        "rules":[{"extensions":["xlsx"],"tier":"none"}],"content_slice_bytes":16}),
        )
        .await;
    task = s
        .post(
            "rename_scope",
            &task,
            json!({"extensions":["pdf","xlsx"],"web_search":false}),
        )
        .await;
    task = s.step(&task).await;
    task = s.run("rename", &task, json!({})).await;
    assert!(
        array(&task["operations"]).is_empty(),
        "unchanged names must not generate moves"
    );
    assert_eq!(array(&task["calls"]).len(), 1);
    assert!(!serde_json::to_string(&s.mock.bodies())
        .unwrap()
        .contains("private.xlsx"));
    assert!(!serde_json::to_string(&s.mock.bodies())
        .unwrap()
        .contains("denied contents"));
    s.configure(json!({"model":"local-test"})).await;
    task = s.post("back", &task, json!({"phase":3})).await;
    task = s.run("rename", &task, json!({})).await;
    assert_eq!(array(&task["operations"]).len(), 1);
    assert_eq!(task["operations"][0]["source"], "report.pdf");
    assert_eq!(task["operations"][0]["destination"], "可读_report.pdf");
    assert_eq!(hashes(&root), before, "proposals never move files");
    task = s.approve(&task).await;
    task = s.run("execute", &task, json!({})).await;
    s.run("rollback", &task, json!({})).await;
    assert_eq!(hashes(&root), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_workflow_security_search_rollback_and_billing() {
    let s = Server::new("local-test").await;
    let root = s.root("Downloads");
    for (name, data) in [
        ("readme.txt", "permitted text"),
        ("private.xlsx", "denied table"),
        ("clip.mp4", "fixture video"),
        ("cache.log", "fixture temporary log"),
        ("Portable/editor.exe", "fixture software"),
        ("Portable/config.json", "{}"),
        ("文档/工作资料/existing.pdf", "existing"),
    ] {
        write(&root, name, data);
    }
    let before = hashes(&root);
    let mut t = s
        .post(
            "create",
            &Value::Null,
            json!({"root":root,"mode":"organize"}),
        )
        .await;
    s.blocked("advance", &t, json!({})).await;
    t = s.run("scan", &t, json!({})).await;
    assert_eq!(t["phase"], 0);
    assert_eq!(t["scanned"], true);
    t = s.step(&t).await;
    let mut p = t["permissions"].clone();
    p["rules"] = json!([{"extensions":["xlsx"],"tier":"none"},{"extensions":["txt"],"tier":"content_slice"}]);
    t = s.post("permissions", &t, json!({"permissions":p})).await;
    let entries = t["entries"].clone();
    t = s
        .post("directory", &t, json!({"id":"Portable","class":"atomic"}))
        .await;
    assert_eq!(t["entries"], entries);
    t = s.step(&t).await;
    t = s
        .run(
            "chat",
            &t,
            json!({"scene":"tree","message":"给当前目录一个可合并的建议"}),
        )
        .await;
    s.blocked("proposal",&t,json!({"proposal_id":t["proposal"]["id"],"scene":"permissions","ids":ids(&t["proposal"]["changes"])})).await;
    t = s.merge(&t, "tree", None, "completed").await;
    assert_eq!(t["nodes"][0]["note"], "保留原有分类，减少不必要的移动。");
    assert_eq!(
        t["calls"][0]["usage"],
        json!({"prompt_tokens":321,"completion_tokens":45,"cached_input_tokens":0,"cache_write_tokens":0,"cache_write_1h_tokens":0,"cache_details_known":false})
    );
    t = s.step(&t).await;
    assert_eq!(t["plan_source"], "rules");
    t = s.run("plan_ai", &t, json!({})).await;
    assert_eq!(t["plan_source"], "ai");
    t = s.step(&t).await;
    assert!(!array(&t["operations"]).is_empty());
    for o in array(&t["operations"]) {
        let source = text(&o["source"]);
        assert!(!source.starts_with("Portable/"));
        assert!(!source.contains("existing.pdf"));
        if source.ends_with(".xlsx") {
            assert!(!text(&o["destination"]).starts_with("视频"));
        }
    }
    s.blocked("execute", &t, json!({})).await;
    t = s.approve(&t).await;
    t = s.run("execute", &t, json!({})).await;
    assert_eq!(t["status"], "completed");
    t = s.run("cleanup_ai", &t, json!({})).await;
    assert!(array(&t["cleanup"]).iter().any(|c| c["source"] == "ai"));
    t = s.run("rollback", &t, json!({})).await;
    assert_eq!(t["status"], "rolled_back");
    assert_eq!(hashes(&root), before);
    let ev = s.events(&t).await;
    for kind in ["move_intent", "restore_done"] {
        assert!(array(&ev).iter().any(|e| e["kind"] == kind));
    }
    let mut imported = s.post("import", &Value::Null, json!({"task":t})).await;
    assert_ne!(imported["id"], t["id"]);
    assert_eq!(imported["scanned"], false);
    assert_eq!(imported["messages"], t["messages"]);
    assert!(array(&imported["operations"]).is_empty());
    imported = s.run("scan", &imported, json!({})).await;
    assert_eq!(imported["nodes"][0]["note"], t["nodes"][0]["note"]);
    for (key, value) in [
        ("Origin", "https://unrelated.example"),
        ("Host", "unrelated.example"),
    ] {
        assert_eq!(
            s.client
                .get(format!("{}/api/bootstrap", s.base))
                .header(key, value)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    s.request("/api/action", json!({"action":"create"}), 403, false)
        .await;
    assert_eq!(
        s.client
            .get(format!("{}/config.toml", s.base))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let mut cfg = s.config().await;
    cfg["llm"]["pricing"] =
        json!({"mode":"manual","input_per_1k_usd":0.001,"output_per_1k_usd":0.002});
    cfg["search"] = json!({"enabled":true,"endpoint":format!("{}/search",s.mock.url),"api_key":"fixture-search-key"});
    let saved = s.post("config", &Value::Null, json!({"config":cfg})).await;
    assert!(saved["llm"].get("api_key").is_none() && saved["search"].get("api_key").is_none());
    let mut r=s.tree(&root,"rename",json!({"default":"filename_only","rules":[{"extensions":["xlsx"],"tier":"none"}],"content_slice_bytes":64})).await;
    r = s
        .post(
            "rename_scope",
            &r,
            json!({"extensions":["txt","xlsx"],"web_search":true}),
        )
        .await;
    r = s.step(&r).await;
    r = s.run("rename", &r, json!({})).await;
    assert_eq!(array(&r["operations"]).len(), 1);
    assert_eq!(r["operations"][0]["source"], "readme.txt");
    assert_eq!(array(&r["search_calls"]).len(), 1);
    assert_eq!(r["search_calls"][0]["query"], "readme.txt");
    assert!((r["calls"][0]["cost_usd"].as_f64().unwrap() - 0.000411).abs() < 1e-12);
    assert!(text(&r["operations"][0]["reason"]).contains("https://example.com/fixture"));
    r = s.approve(&r).await;
    r = s.run("execute", &r, json!({})).await;
    s.run("rollback", &r, json!({})).await;
    assert_eq!(hashes(&root), before);
    let mut stream = s
        .client
        .get(format!("{}/api/events", s.base))
        .send()
        .await
        .unwrap();
    let pending = s
        .post(
            "chat",
            &imported,
            json!({"scene":"scan","message":"slow-fixture"}),
        )
        .await;
    let saw = tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        loop {
            bytes.extend(stream.chunk().await.unwrap().unwrap());
            if String::from_utf8_lossy(&bytes).contains("\"type\":\"progress\"") {
                break;
            }
        }
    })
    .await;
    assert!(saw.is_ok());
    let start = Instant::now();
    s.post(
        "cancel",
        &Value::Null,
        json!({"job_id":pending["job"]["id"]}),
    )
    .await;
    imported = s.wait(pending, "paused").await;
    assert!(start.elapsed() < Duration::from_secs(3));
    drop(stream);
    let calls = array(&imported["calls"]).len();
    s.budget(json!(1)).await;
    imported = s
        .wait(s.act("test_connection", &imported).await, "failed")
        .await;
    assert!(text(&s.get("/api/bootstrap").await["last_job"]["error"]).contains("预算"));
    assert_eq!(array(&imported["calls"]).len(), calls);
    s.budget(json!(30000)).await;
    s.configure(json!({"model":"missing-usage-fixture"})).await;
    imported = s
        .wait(s.act("test_connection", &imported).await, "failed")
        .await;
    assert!(text(&s.get("/api/bootstrap").await["last_job"]["error"]).contains("token"));
    assert_eq!(array(&imported["calls"]).len(), calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planning_preserves_draft_selection_and_ids_on_failure_cancel_and_back() {
    let s = Server::new("local-test").await;
    let root = s.root("files");
    for n in ["movie.mp4", "lecture.mp4", "notes.txt", "Portable/app.exe"] {
        write(&root, n, format!("fixture {n}"));
    }
    let before = hashes(&root);
    let mut t = s.planning(&root, "organize", Value::Null).await;
    assert_eq!(t["phase"], 3);
    assert_eq!(t["plan_source"], "rules");
    assert!(array(&t["calls"]).is_empty());
    let op_ids = ids(&t["operations"]);
    assert!(array(&t["operations"])
        .iter()
        .any(|o| o["source"] == "Portable"));
    s.blocked("execute", &t, json!({})).await;
    t = s.run("plan_ai", &t, json!({})).await;
    assert_eq!(ids(&t["operations"]), op_ids);
    assert!(array(&t["operations"])
        .iter()
        .any(|o| text(&o["destination"]).starts_with("视频/电影/")));
    t = s.step(&t).await;
    s.blocked("advance", &t, json!({})).await;
    let selected = json!(array(&op_ids)[1..]);
    t = s
        .post("review", &t, json!({"selected":selected,"reviewed":true}))
        .await;
    t = s.run("plan_ai", &t, json!({})).await;
    assert_eq!(t["phase"], 4);
    assert_eq!(t["reviewed"], false);
    assert_eq!(
        json!(array(&t["operations"])
            .iter()
            .filter(|o| o["selected"] == true)
            .map(|o| o["id"].clone())
            .collect::<Vec<_>>()),
        selected
    );
    let draft = t["operations"].clone();
    t = s.post("back", &t, json!({"phase":3})).await;
    assert_eq!(t["operations"], draft);
    assert_eq!(t["plan_source"], "ai");
    t = s.step(&t).await;
    assert_eq!(t["operations"], draft);
    let count = array(&t["calls"]).len();
    s.configure(json!({"model":"invalid-json-fixture"})).await;
    t = s.wait(s.act("plan_ai", &t).await, "failed").await;
    assert_eq!(t["operations"], draft);
    assert_eq!(t["phase"], 4);
    assert_eq!(array(&t["calls"]).len(), count + 2);
    s.configure(json!({"model":"planning-pause-fixture"})).await;
    let pending = s.act("plan_ai", &t).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let b = s.get("/api/bootstrap").await;
            if s.mock
                .bodies()
                .iter()
                .any(|r| r["model"] == "planning-pause-fixture")
                && b["job"].is_object()
            {
                assert!(number(&b["job"]["total"]) > 0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    s.post(
        "cancel",
        &Value::Null,
        json!({"job_id":pending["job"]["id"]}),
    )
    .await;
    t = s.wait(pending, "paused").await;
    assert_eq!(t["operations"], draft);
    s.blocked("execute", &t, json!({})).await;
    assert_eq!(hashes(&root), before);
    t = s.post("back", &t, json!({"phase":2})).await;
    assert!(array(&t["operations"]).is_empty());
    assert!(t["plan_source"].is_null());
    s.configure(json!({"model":"local-test"})).await;
    t = s.step(&t).await;
    assert_eq!(t["plan_source"], "rules");
    let empty = s.planning(&s.root("empty"), "organize", Value::Null).await;
    assert!(array(&empty["operations"]).is_empty());
    let empty = s.step(&empty).await;
    assert_eq!(empty["phase"], 4);
    assert_eq!(empty["reviewed"], false);
}
