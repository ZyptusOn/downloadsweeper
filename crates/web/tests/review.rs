mod support;
use serde_json::{json, Value};
use support::*;
async fn planning(s: &Server, root: &std::path::Path, mode: &str) -> Value {
    let mut t=s.tree(root,mode,json!({"default":"filename_only","content_slice_bytes":64,"rules":[{"extensions":["xlsx"],"tier":"none"}]})).await;
    t=s.post("tree",&t,json!({"nodes":[node("other",Some("root"),"其它",&["*"]),node("docs",Some("root"),"文档",&["txt"]),node("work",Some("docs"),"工作",&[])]})).await;
    s.step(&t).await
}
fn op<'a>(t: &'a Value, source: &str) -> &'a Value {
    array(&t["operations"])
        .iter()
        .find(|o| o["source"] == source)
        .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_splits_placements_retention_and_atomic_rejection() {
    let s = Server::new("local-test").await;
    s.budget(Value::Null).await;
    s.configure(json!({"parallel_requests":3})).await;
    for mode in ["desktop", "organize"] {
        let root = s.root(mode);
        for n in ["clip.mp4", "notes.txt", "private.xlsx"] {
            write(&root, n, format!("fixture-{n}"));
        }
        let before_files = hashes(&root);
        s.configure(json!({"model":"local-test"})).await;
        let mut t = planning(&s, &root, mode).await;
        t = s.run("plan_ai", &t, json!({"batch_size":2})).await;
        t = s.step(&t).await;
        assert_eq!(op(&t, "notes.txt")["destination"], "文档/工作/notes.txt");
        t=s.post("review",&t,json!({"selected":array(&t["operations"]).iter().filter(|o|o["source"]!="notes.txt").map(|o|o["id"].clone()).collect::<Vec<_>>(),"reviewed":true})).await;
        let before = t["operations"].clone();
        s.configure(json!({"model":"review-split-fixture"})).await;
        t = s
            .run(
                "chat",
                &t,
                json!({"scene":"review","message":"把视频从其它移出来放到单独一级目录"}),
            )
            .await;
        assert_eq!(t["operations"], before);
        assert_eq!(t["reviewed"], true);
        let calls = array(&t["calls"]).len();
        t = s.merge(&t, "review", None, "completed").await;
        assert_eq!(t["phase"], 4);
        assert_eq!(t["status"], "planned");
        assert_eq!(t["reviewed"], false);
        assert_eq!(op(&t, "clip.mp4")["destination"], "视频/clip.mp4");
        assert_eq!(
            op(&t, "notes.txt"),
            array(&before)
                .iter()
                .find(|o| o["source"] == "notes.txt")
                .unwrap()
        );
        assert_eq!(array(&t["calls"]).len(), calls);
        assert!(t["classification"].is_null());
        s.configure(json!({"model":"review-placement-fixture"}))
            .await;
        t = s
            .run(
                "chat",
                &t,
                json!({"scene":"review","message":"把notes放到新建笔记子目录"}),
            )
            .await;
        let before = t["operations"].clone();
        let nodes = t["nodes"].clone();
        let placement = array(&t["proposal"]["changes"])
            .iter()
            .find(|c| c["kind"] == "placement")
            .unwrap()["id"]
            .clone();
        t = s
            .merge(&t, "review", Some(json!([placement])), "failed")
            .await;
        assert_eq!(t["operations"], before);
        assert_eq!(t["nodes"], nodes);
        assert!(t["proposal"].is_object());
        t = s.merge(&t, "review", None, "completed").await;
        assert_eq!(op(&t, "notes.txt")["destination"], "文档/笔记/notes.txt");
        assert_eq!(op(&t, "notes.txt")["selected"], false);
        s.configure(json!({"model":"review-keep-fixture"})).await;
        t = s
            .run(
                "chat",
                &t,
                json!({"scene":"review","message":"clip保持原位"}),
            )
            .await;
        t = s.merge(&t, "review", None, "completed").await;
        assert!(!array(&t["operations"])
            .iter()
            .any(|o| o["source"] == "clip.mp4"));
        assert!(array(&t["retained"])
            .iter()
            .any(|o| o["source"] == "clip.mp4"));
        s.configure(json!({"model":"review-invalid-fixture"})).await;
        let before = t["operations"].clone();
        t = s
            .wait(
                s.post("chat", &t, json!({"scene":"review","message":"invalid"}))
                    .await,
                "failed",
            )
            .await;
        assert_eq!(t["operations"], before);
        assert_eq!(hashes(&root), before_files);
    }
    let wire = s.mock.bodies();
    let split: Vec<_> = wire
        .iter()
        .filter(|r| r["model"] == "review-split-fixture")
        .collect();
    assert!(!split.is_empty() && !json!(split).to_string().contains("private.xlsx"));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn semantic_review_resumes_saved_batches_without_duplicate_charges() {
    let s = Server::new("review-semantic-resume-fixture").await;
    s.budget(Value::Null).await;
    s.configure(json!({"parallel_requests":3})).await;
    let root = s.root("Desktop");
    for i in 0..260 {
        write(&root, &format!("note{i:03}.txt"), "lesson fixture");
    }
    write(&root, "clip.mp4", "video fixture");
    write(&root, "private.xlsx", "private fixture");
    let hashes_before = hashes(&root);
    let mut t = planning(&s, &root, "desktop").await;
    t = s.step(&t).await;
    t=s.post("review",&t,json!({"selected":array(&t["operations"]).iter().filter(|o|o["source"]!="note000.txt").map(|o|o["id"].clone()).collect::<Vec<_>>(),"reviewed":true})).await;
    t = s
        .run(
            "chat",
            &t,
            json!({"scene":"review","message":"在文档下新建学习笔记，按内容归类"}),
        )
        .await;
    let before = t["operations"].clone();
    let nodes = t["nodes"].clone();
    let p = &t["proposal"];
    let pending = s
        .post(
            "proposal",
            &t,
            json!({"proposal_id":p["id"],"scene":"review","ids":ids(&p["changes"])}),
        )
        .await;
    let live = s.current(&t).await;
    assert_eq!(live["operations"], before);
    assert_eq!(live["nodes"], nodes);
    t = s.wait(pending, "failed").await;
    assert_eq!(t["operations"], before);
    assert_eq!(t["nodes"], nodes);
    assert!(t["proposal"].is_object());
    let run = t["review_classification"].clone();
    assert_eq!(run["total"], 260);
    assert!(number(&run["completed"]) > 0 && number(&run["completed"]) < 260);
    assert_eq!(run["status"], "failed");
    let finished = array(&run["batches"])
        .iter()
        .filter(|b| b["status"] == "complete")
        .count();
    let calls = array(&t["calls"]).len();
    let stopped = s.get("/api/bootstrap").await["last_job"].clone();
    assert_eq!(stopped["resumable"], true);
    t = s
        .wait(
            s.post("resume_job", &Value::Null, json!({"job_id":stopped["id"]}))
                .await,
            "failed",
        )
        .await;
    assert_eq!(array(&t["calls"]).len(), calls);
    assert_eq!(t["operations"], before);
    t = s.merge(&t, "review", None, "completed").await;
    assert!(t["proposal"].is_null());
    assert_eq!(t["review_classification"]["status"], "complete");
    assert_eq!(
        array(&t["calls"]).len() - calls,
        array(&run["batches"]).len() - finished
    );
    for o in array(&t["operations"])
        .iter()
        .filter(|o| text(&o["source"]).ends_with(".txt"))
    {
        assert_eq!(
            o["destination"],
            format!("文档/学习笔记/{}", text(&o["source"]))
        );
    }
    assert_eq!(op(&t, "note000.txt")["selected"], false);
    assert_eq!(
        op(&t, "clip.mp4"),
        array(&before)
            .iter()
            .find(|o| o["source"] == "clip.mp4")
            .unwrap()
    );
    assert_eq!(hashes(&root), hashes_before);
    let calls: Vec<_> = s
        .mock
        .bodies()
        .into_iter()
        .filter(|r| r["tools"].is_array())
        .collect();
    assert!(!calls.is_empty());
    let wire = json!(calls).to_string();
    assert!(!wire.contains("clip.mp4") && !wire.contains("private.xlsx"));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_parallel_progress_is_monotonic_and_counts_finished_files_only() {
    let s = Server::new("parallel-progress-fixture").await;
    s.configure(json!({"parallel_requests":3})).await;
    s.budget(Value::Null).await;
    let root = s.root("files");
    for i in 0..10 {
        write(&root, &format!("clip{i}.mp4"), "fixture");
    }
    let t = s.planning(&root, "organize", Value::Null).await;
    let mut samples = Vec::new();
    let t = s
        .wait_observed(
            s.post("plan_ai", &t, json!({"batch_size":2})).await,
            "completed",
            &mut samples,
        )
        .await;
    assert!(samples.iter().any(|s| array(&s["batches"])
        .iter()
        .filter(|b| b["status"] == "running")
        .count()
        == 3
        && array(&s["batches"])
            .iter()
            .any(|b| b["status"] == "pending")));
    let mut previous = 0;
    for s in samples {
        let count = number(&s["completed_files"]);
        assert!(count >= previous);
        previous = count;
        assert_eq!(
            count,
            array(&s["batches"])
                .iter()
                .filter(|b| b["status"] == "complete")
                .map(|b| number(&b["files"]))
                .sum::<u64>()
        );
    }
    assert_eq!(t["classification"]["completed"], 10);
    assert_eq!(array(&t["calls"]).len(), 5);
}
