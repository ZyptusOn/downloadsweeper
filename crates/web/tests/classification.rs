mod support;
use serde_json::{json, Value};
use std::time::Duration;
use support::*;
fn requests(ev: &Value) -> Vec<Value> {
    array(ev)
        .iter()
        .filter(|e| e["kind"] == "llm_request")
        .map(|e| e["detail"].clone())
        .collect()
}
fn evidence(ev: &Value) -> Vec<Value> {
    array(ev)
        .iter()
        .filter(|e| {
            e["kind"] == "classification_tool_result" && e["detail"]["tool"] == "read_file_evidence"
        })
        .map(|e| e["detail"]["result"].clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batches_checkpoint_truncation_and_adaptive_context() {
    let s = Server::new("local-test").await;
    s.configure(json!({"thinking_mode":true})).await;
    let root = s.root("files");
    for i in 0..130 {
        write(&root, &format!("clip_{i:03}.mp4"), "dummy video");
    }
    write(&root, "private.xlsx", "DO_NOT_SEND_PRIVATE_TABLE");
    write(&root, "Portable/app.exe", "DO_NOT_READ_ATOMIC_DIRECTORY");
    let before = hashes(&root);
    let mut t=s.planning(&root,"organize",json!({"default":"filename_only","content_slice_bytes":64,"rules":[{"extensions":["xlsx","@folder"],"tier":"none"}]})).await;
    let original = t["operations"].clone();
    let tree = t["nodes"].clone();
    t = s.run("plan_ai", &t, json!({"batch_size":64})).await;
    assert_eq!(t["classification"]["status"], "complete");
    assert_eq!(t["classification"]["completed"], 130);
    assert_eq!(
        json!(array(&t["classification"]["batches"])
            .iter()
            .map(|b| b["files"].clone())
            .collect::<Vec<_>>()),
        json!([64, 64, 2])
    );
    assert_eq!(array(&t["calls"]).len(), 3);
    assert!(array(&t["calls"])
        .iter()
        .all(|c| c["finish_reason"] == "tool_calls"));
    assert_eq!(t["nodes"], tree);
    assert_eq!(t["phase"], 3);
    assert_eq!(ids(&t["operations"]), ids(&original));
    for r in requests(&s.events(&t).await) {
        assert_eq!(r["thinking"], false);
        assert!(number(&r["input_text_bytes"]) <= 24 * 1024);
        let mut names: Vec<_> = array(&r["tools"]).iter().map(text).collect();
        names.sort();
        assert_eq!(names, ["read_file_evidence", "submit_classifications"]);
        assert_eq!(r["max_output_tokens"], 32768);
        assert_eq!(r["configured_max_output_tokens"], 32768);
        assert!(array(&r["output_limit_reasons"]).is_empty());
    }
    assert!(s.mock.bodies().iter().all(|r| r["max_tokens"] == 32768));
    assert!(!array(&t["operations"])
        .iter()
        .any(|o| text(&o["source"]).starts_with("Portable/")));
    s.configure(json!({"model":"classification-pause-fixture"}))
        .await;
    let draft = t["operations"].clone();
    let pending = s.post("plan_ai", &t, json!({"batch_size":64})).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current = s.current(&t).await;
            let b = s.get("/api/bootstrap").await;
            if current["classification"]["completed"] == 64
                && b["job"]["parallel"]["batches"]
                    .as_array()
                    .is_some_and(|bs| {
                        bs.iter()
                            .any(|b| b["id"] == "b2" && b["status"] == "running")
                    })
            {
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
    assert_eq!(t["classification"]["completed"], 64);
    assert_eq!(t["classification"]["status"], "paused");
    assert_eq!(t["operations"], draft);
    let calls = array(&t["calls"]).len();
    t = s.run("plan_ai", &t, json!({"batch_size":64})).await;
    assert_eq!(array(&t["calls"]).len(), calls + 2);
    let truncated = s.root("truncation");
    for i in 0..122 {
        write(&truncated, &format!("{i:03}.mp4"), "fixture");
    }
    s.configure(json!({"model":"classification-truncated-once-fixture"}))
        .await;
    let mut t = s.planning(&truncated, "organize", Value::Null).await;
    let original = t["operations"].clone();
    t = s
        .wait(
            s.post("plan_ai", &t, json!({"batch_size":61})).await,
            "failed",
        )
        .await;
    assert_eq!(t["classification"]["completed"], 61);
    assert_eq!(t["classification"]["batches"][0]["status"], "complete");
    assert_eq!(t["classification"]["batches"][1]["status"], "pending");
    assert_eq!(t["operations"], original);
    assert_eq!(array(&t["calls"]).len(), 2);
    assert_eq!(t["calls"][1]["usage"]["completion_tokens"], 32768);
    let error = text(&t["classification"]["error"]);
    assert!(
        error.contains("请求输出上限 32768")
            && error.contains("设置值 32768")
            && !error.contains("关闭")
    );
    let checkpoint = t["classification"]["batches"][0].clone();
    t = s.run("plan_ai", &t, json!({"batch_size":61})).await;
    assert_eq!(t["classification"]["status"], "complete");
    assert_eq!(array(&t["calls"]).len(), 3);
    assert_eq!(t["classification"]["batches"][0], checkpoint);
    let long = s.root("long");
    for i in 0..90 {
        write(
            &long,
            &format!("{i}{}.mp4", "很长的文件描述".repeat(9)),
            "fixture",
        );
    }
    s.configure(json!({"model":"local-test","context_length":32000}))
        .await;
    let t = s
        .run(
            "plan_ai",
            &s.planning(&long, "organize", Value::Null).await,
            json!({"batch_size":128}),
        )
        .await;
    let sizes: Vec<_> = array(&t["classification"]["batches"])
        .iter()
        .map(|b| number(&b["files"]))
        .collect();
    assert_eq!(sizes.iter().sum::<u64>(), 90);
    assert!(sizes.len() > 1 && *sizes.iter().max().unwrap() < 90);
    assert!(requests(&s.events(&t).await)
        .iter()
        .all(|r| number(&r["input_text_bytes"]) <= 24 * 1024));
    assert_eq!(hashes(&root), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn evidence_permissions_output_limits_and_invalid_tools() {
    let s = Server::new("deepseek-evidence-fixture").await;
    let root = s.root("evidence");
    write(
        &root,
        "notes.txt",
        format!("PERMITTED_EVIDENCE_{}", "x".repeat(4000)),
    );
    write(&root, "name_only.txt", "NAME_ONLY_CONTENT_MUST_NOT_LEAK");
    write(&root, "secret.xlsx", "DENIED_FILENAME_AND_CONTENT");
    let before = hashes(&root);
    let permissions = json!({"default":"filename_only","content_slice_bytes":64,"rules":[{"extensions":["xlsx"],"tier":"none"},{"extensions":["txt"],"min_bytes":1000,"tier":"content_slice"}]});
    let mut t = s.planning(&root, "organize", permissions.clone()).await;
    t = s
        .run("plan_ai", &t, json!({"thinking":true,"batch_size":64}))
        .await;
    let results = evidence(&s.events(&t).await);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["files"][0]["bytes"], 64);
    assert_eq!(results[0]["files"][0]["truncated"], true);
    assert_eq!(array(&t["calls"]).len(), 2);
    assert!(!json!(results)
        .to_string()
        .contains("NAME_ONLY_CONTENT_MUST_NOT_LEAK"));
    assert!(s
        .mock
        .bodies()
        .iter()
        .all(|r| r["thinking"]["type"] == "enabled" && r["max_tokens"] == 32768));
    s.configure(json!({"model":"classification-truncated-fixture","context_length":32000}))
        .await;
    s.budget(Value::Null).await;
    let mut limited = s.planning(&root, "organize", permissions).await;
    let draft = limited["operations"].clone();
    limited = s.wait(s.act("plan_ai", &limited).await, "failed").await;
    let r = requests(&s.events(&limited).await).pop().unwrap();
    assert_eq!(
        number(&r["max_output_tokens"]),
        32000 - number(&r["estimated_input_tokens"])
    );
    assert!(number(&r["max_output_tokens"]) < 32768);
    assert_eq!(r["output_limit_reasons"], json!(["上下文剩余空间"]));
    assert!(text(&limited["classification"]["error"]).contains("上下文剩余空间"));
    assert_eq!(limited["operations"], draft);
    s.configure(json!({"context_length":256000})).await;
    let used: u64 = array(&limited["calls"])
        .iter()
        .map(|c| number(&c["usage"]["prompt_tokens"]) + number(&c["usage"]["completion_tokens"]))
        .sum();
    s.budget(json!(used + number(&r["estimated_input_tokens"]) + 2000))
        .await;
    limited = s.wait(s.act("plan_ai", &limited).await, "failed").await;
    let r = requests(&s.events(&limited).await).pop().unwrap();
    assert_eq!(r["max_output_tokens"], 2000);
    assert_eq!(
        r["output_limit_reasons"],
        json!(["剩余任务 token 预算（含并行预留）"])
    );
    assert!(text(&limited["classification"]["error"]).contains("设置值 32768"));
    assert_eq!(limited["operations"], draft);
    s.budget(json!(100000)).await;
    for model in [
        "classification-forbidden-read-fixture",
        "classification-permission-fixture",
    ] {
        s.configure(json!({"model":model})).await;
        t = s.run("plan_ai", &t, json!({"batch_size":64})).await;
        let latest = evidence(&s.events(&t).await).pop().unwrap();
        if model.contains("forbidden") {
            assert!(latest.get("error").is_some());
        } else {
            assert!(array(&latest["files"])
                .iter()
                .any(|f| f["status"] == "unavailable"));
        }
    }
    let original = t["operations"].clone();
    for model in [
        "classification-invalid-node-fixture",
        "classification-incomplete-fixture",
        "classification-duplicate-fixture",
        "classification-path-fixture",
        "classification-missing-node-fixture",
    ] {
        s.configure(json!({"model":model})).await;
        t = s.wait(s.act("plan_ai", &t).await, "failed").await;
        assert_eq!(t["operations"], original, "{model}");
        assert_eq!(t["classification"]["completed"], 0, "{model}");
    }
    s.configure(json!({"model":"classification-null-fixture"}))
        .await;
    t = s.run("plan_ai", &t, json!({})).await;
    assert_eq!(t["operations"], original);
    let imported = s.post("import", &Value::Null, json!({"task":t})).await;
    assert!(imported["classification"].is_null());
    assert_eq!(imported["scanned"], false);
    assert_eq!(hashes(&root), before);
    let images = s.root("images");
    for i in 0..8 {
        write(&images, &format!("picture_{i}.png"), media::png());
    }
    s.configure(json!({"model":"classification-evidence-fixture","multimodal":true}))
        .await;
    let t = s
        .planning(
            &images,
            "organize",
            json!({"default":"image","content_slice_bytes":64,"rules":[]}),
        )
        .await;
    let t = s.run("plan_ai", &t, json!({"batch_size":128})).await;
    assert_eq!(
        json!(array(&t["classification"]["batches"])
            .iter()
            .map(|b| b["files"].clone())
            .collect::<Vec<_>>()),
        json!([4, 4])
    );
    assert_eq!(
        json!(requests(&s.events(&t).await)
            .iter()
            .map(|r| r["image_count"].clone())
            .collect::<Vec<_>>()),
        json!([0, 4, 0, 4])
    );
}
