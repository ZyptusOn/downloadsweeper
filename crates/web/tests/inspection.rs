mod support;
use serde_json::{json, Value};
use std::time::Duration;
use support::*;
fn inspected(t: &Value) -> u64 {
    array(&t["inspection"]["groups"])
        .iter()
        .map(|g| number(&g["inspected"]))
        .sum()
}
fn contexts(s: &Server) -> Vec<Value> {
    s.mock
        .bodies()
        .iter()
        .map(|r| mock::context(&array(&r["messages"]).last().unwrap()["content"]))
        .collect()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overview_bounded_type_batches_resume_and_permission_invalidation() {
    let s = Server::new("inspection-pause-fixture").await;
    s.budget(Value::Null).await;
    s.configure(json!({"context_length":24000})).await;
    let root = s.root("files");
    for i in 0..55 {
        write(
            &root,
            &format!("observe_{i:02}.txt"),
            if i == 0 {
                "允许的文本切片".repeat(800)
            } else {
                "允许读取的内容".into()
            },
        );
    }
    for i in 0..25 {
        write(&root, &format!("video_{i:02}.mp4"), "dummy video");
    }
    for (n, v) in [
        ("private_NEVER_SEND.xlsx", "NEVER_SEND_TABLE_CONTENT"),
        ("name_only.docx", "NEVER_SEND_DOCX_CONTENT"),
        ("Portable/editor.exe", "program"),
        ("Portable/NEVER_SEND_PROTECTED.txt", "protected"),
    ] {
        write(&root, n, v);
    }
    let before = hashes(&root);
    let permissions = json!({"default":"filename_only","content_slice_bytes":4096,"rules":[{"extensions":["xlsx","@folder"],"tier":"none"},{"extensions":["txt"],"tier":"content_slice"},{"extensions":["mp4"],"tier":"metadata"}]});
    let mut t = s.tree(&root, "organize", permissions).await;
    let nodes = t["nodes"].clone();
    let job = s
        .post(
            "suggest_tree",
            &t,
            json!({"scene":"tree","message":"先查看总览，再分类型检查并建议结构"}),
        )
        .await;
    let completed = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let current = s.current(&t).await;
            let b = s.get("/api/bootstrap").await;
            if current["inspection"].is_object()
                && inspected(&current) > 0
                && b["job"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("第 2 批")
            {
                break inspected(&current);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    s.post("cancel", &Value::Null, json!({"job_id":job["job"]["id"]}))
        .await;
    t = s.wait(job, "paused").await;
    assert_eq!(t["inspection"]["status"], "paused");
    assert_eq!(inspected(&t), completed);
    assert_eq!(t["nodes"], nodes);
    assert!(t["proposal"].is_null());
    assert!(!array(&t["calls"]).is_empty());
    assert!(array(&t["calls"])
        .iter()
        .all(|c| c["usage"]["prompt_tokens"] == 321));
    let count = s.mock.bodies().len();
    let used: u64 = array(&t["calls"])
        .iter()
        .map(|c| number(&c["usage"]["prompt_tokens"]) + number(&c["usage"]["completion_tokens"]))
        .sum();
    s.budget(json!(used + 1)).await;
    t = s
        .wait(
            s.post(
                "suggest_tree",
                &t,
                json!({"scene":"tree","message":"预算不足时不得继续发送"}),
            )
            .await,
            "failed",
        )
        .await;
    assert_eq!(s.mock.bodies().len(), count);
    assert_eq!(inspected(&t), completed);
    s.budget(Value::Null).await;
    t = s
        .run(
            "suggest_tree",
            &t,
            json!({"scene":"tree","message":"继续分析并建议结构"}),
        )
        .await;
    assert_eq!(t["inspection"]["status"], "complete");
    assert_eq!(inspected(&t), 81);
    assert_eq!(
        array(&t["inspection"]["groups"])
            .iter()
            .map(|g| number(&g["withheld"]))
            .sum::<u64>(),
        2
    );
    assert_eq!(t["inspection"]["protected_files"], 2);
    assert!(t["proposal"].is_object());
    assert_eq!(t["nodes"], nodes);
    let contexts = contexts(&s);
    let overviews: Vec<_> = contexts
        .iter()
        .filter(|c| c["stage"] == "overview")
        .collect();
    let batches: Vec<_> = contexts
        .iter()
        .filter(|c| c["stage"] == "inspect_batch")
        .collect();
    assert_eq!(overviews.len(), 1);
    assert_eq!(
        batches
            .iter()
            .filter(|c| c["type_id"] == "text" && c["batch"] == 1)
            .count(),
        1
    );
    for c in batches {
        let files = array(&c["files"]);
        assert!(!files.is_empty() && files.len() <= 24);
        assert!(c["files"].to_string().len() <= 8192);
        assert!(files
            .iter()
            .all(|f| f["text_excerpt"].as_str().unwrap_or("").len() <= 1024));
    }
    assert!(!json!(overviews).to_string().contains("observe_00.txt"));
    let wire = json!(s.mock.bodies()).to_string();
    for denied in [
        "private_NEVER_SEND.xlsx",
        "NEVER_SEND_TABLE_CONTENT",
        "NEVER_SEND_DOCX_CONTENT",
        "NEVER_SEND_PROTECTED.txt",
    ] {
        assert!(!wire.contains(denied), "{denied}");
    }
    let last = s.mock.bodies().pop().unwrap().to_string();
    assert!(last.contains("file_inspection") && !last.contains("observe_00.txt"));
    let calls = array(&t["calls"]).len();
    t["nodes"][0]["note"] = json!("编辑树后仍可使用文件观察结果");
    t = s.post("tree", &t, json!({"nodes":t["nodes"]})).await;
    t = s
        .run(
            "suggest_tree",
            &t,
            json!({"scene":"tree","message":"根据已完成分析再给建议"}),
        )
        .await;
    assert_eq!(array(&t["calls"]).len(), calls + 1);
    assert_eq!(
        self::contexts(&s)
            .iter()
            .filter(|c| c["stage"] == "overview")
            .count(),
        1
    );
    t = s.post("back", &t, json!({"phase":1})).await;
    assert!(t["inspection"].is_null());
    let mut p = t["permissions"].clone();
    p["rules"][1]["tier"] = json!("none");
    t = s.post("permissions", &t, json!({"permissions":p})).await;
    t = s.step(&t).await;
    let offset = s.mock.bodies().len();
    t = s
        .run(
            "suggest_tree",
            &t,
            json!({"scene":"tree","message":"按更新后的权限重新检查"}),
        )
        .await;
    assert_eq!(inspected(&t), 26);
    assert!(!json!(&s.mock.bodies()[offset..])
        .to_string()
        .contains("observe_00.txt"));
    assert_eq!(hashes(&root), before);
}
