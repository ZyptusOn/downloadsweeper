mod support;
use serde_json::{json, Value};
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_answers_are_billed_without_publishing_changes() {
    let s = Server::new("local-test").await;
    s.budget(Value::Null).await;
    s.configure(json!({"max_output_tokens":16384,"context_length":65536,"thinking_mode":true}))
        .await;
    let root = s.root("files");
    write(&root, "readme.txt", "local fixture");
    let mut t = s.tree(&root, "organize", Value::Null).await;
    let before = t["nodes"].clone();
    for (model, error) in [
        ("truncated-reasoning-fixture", "回答被截断"),
        ("truncated-json-fixture", "回答被截断"),
        ("empty-answer-fixture", "没有返回最终回答"),
        ("invalid-json-fixture", "改动 JSON"),
    ] {
        s.configure(json!({"model":model})).await;
        let calls = array(&t["calls"]).len();
        t = s
            .wait(
                s.post(
                    "chat",
                    &t,
                    json!({"scene":"tree","message":"生成可合并的结构建议"}),
                )
                .await,
                "failed",
            )
            .await;
        assert!(text(&s.get("/api/bootstrap").await["last_job"]["error"]).contains(error));
        assert_eq!(t["nodes"], before);
        assert!(t["proposal"].is_null());
        assert!(!array(&t["messages"])
            .iter()
            .any(|m| m["role"] == "assistant"));
        assert_eq!(array(&t["calls"]).len(), calls + 1);
        let c = array(&t["calls"]).last().unwrap();
        assert_eq!(c["max_output_tokens"], 16384);
        assert_eq!(
            c["usage"]["completion_tokens"],
            if model.starts_with("truncated-") {
                16384
            } else {
                45
            }
        );
    }
    s.configure(json!({"model":"deepseek-fixture"})).await;
    t = s
        .run(
            "chat",
            &t,
            json!({"scene":"tree","message":"生成可合并的结构建议"}),
        )
        .await;
    assert!(text(&s.get("/api/bootstrap").await["last_job"]["message"]).contains("1 项建议"));
    assert_eq!(t["nodes"], before);
    assert_eq!(array(&t["calls"]).last().unwrap()["finish_reason"], "stop");
    t = s.merge(&t, "tree", None, "completed").await;
    assert_eq!(t["nodes"][0]["note"], "保留原有分类，减少不必要的移动。");
    assert!(t["proposal"].is_null() && array(&t["operations"]).is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("readme.txt")).unwrap(),
        "local fixture"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graph_partial_edits_dependency_gate_and_invalid_parent() {
    let s = Server::new("tree-structure-fixture").await;
    let root = s.root("files");
    write(&root, "school.mp4", "fixture video");
    write(&root, "reference.pdf", "fixture reference");
    let mut t = s.tree(&root, "organize", Value::Null).await;
    let mut nodes = t["nodes"].clone();
    let movie = nodes
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["name"] == "电影")
        .unwrap();
    movie["examples"] = json!(["school.mp4"]);
    movie["position"] = json!([812, 364]);
    let movie = movie.clone();
    t = s.post("tree", &t, json!({"nodes":nodes})).await;
    let original = t["nodes"].clone();
    t = s
        .run(
            "suggest_tree",
            &t,
            json!({"scene":"tree","message":"根据摘要新增子目录、调整节点描述"}),
        )
        .await;
    assert_eq!(t["nodes"], original);
    let p = &t["proposal"];
    assert_eq!(array(&p["changes"]).len(), 6);
    assert_eq!(
        array(&p["changes"])
            .iter()
            .filter(|c| c["before"].is_null())
            .count(),
        2
    );
    let edit = &array(&p["changes"])
        .iter()
        .find(|c| c["target"] == movie["id"])
        .unwrap()["after"];
    assert_eq!(edit["name"], "影视长片");
    for key in ["parent", "examples", "rule_type"] {
        assert_eq!(edit[key], movie[key]);
    }
    assert_eq!(edit["position"], json!([812.0, 364.0]));
    let child = array(&p["changes"])
        .iter()
        .find(|c| c["target"] == "fixture-campus")
        .unwrap();
    assert_eq!(child["after"]["name"], "校园记录／学业");
    assert!(text(&p["message"]).contains("全角字符"));
    let error = s
        .blocked(
            "proposal",
            &t,
            json!({"proposal_id":p["id"],"scene":"tree","ids":[child["id"]]}),
        )
        .await;
    assert!(text(&error["error"]).contains("父节点"));
    assert_eq!(s.current(&t).await["nodes"], original);
    t = s.merge(&t, "tree", None, "completed").await;
    let ns = array(&t["nodes"]);
    let find = |id: &Value| ns.iter().find(|n| n["id"] == *id).unwrap();
    assert_eq!(find(&json!("fixture-campus"))["parent"], "fixture-topics");
    assert_eq!(find(&json!("fixture-topics"))["parent"], movie["parent"]);
    assert_eq!(find(&movie["id"])["name"], "影视长片");
    assert!(!ns.iter().any(|n| n["name"] == "番剧"));
    assert_eq!(
        ns.iter().find(|n| n["name"] == "剪辑素材").unwrap()["parent"],
        "fixture-topics"
    );
    assert!(t["proposal"].is_null() && array(&t["operations"]).is_empty());
    assert_eq!(s.current(&t).await["nodes"], t["nodes"]);
    assert_eq!(
        std::fs::read(root.join("school.mp4")).unwrap(),
        b"fixture video"
    );
    t = s.post("tree", &t, json!({"nodes":original})).await;
    s.configure(json!({"model":"invalid-tree-fixture"})).await;
    t = s
        .wait(
            s.post(
                "chat",
                &t,
                json!({"scene":"tree","message":"生成无效父节点测试"}),
            )
            .await,
            "failed",
        )
        .await;
    assert_eq!(t["nodes"], original);
    assert!(t["proposal"].is_null());
    assert!(text(&s.get("/api/bootstrap").await["last_job"]["error"]).contains("有效目录结构"));
    s.configure(json!({"model":"tree-structure-fixture"})).await;
    s.run(
        "suggest_tree",
        &t,
        json!({"scene":"tree","message":"生成完整目录改动"}),
    )
    .await;
    let r = s.mock.bodies().pop().unwrap();
    assert!(text(&r["messages"][1]["content"]).contains("file_inspection"));
    assert!(array(&r["messages"])
        .iter()
        .any(|m| m["role"] == "system" && text(&m["content"]).contains("多层分类结构")));
    assert!(!array(&r["messages"])
        .iter()
        .any(|m| m["role"] == "assistant"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_top_level_and_duplicate_template_reconciliation_in_both_modes() {
    let s = Server::new("new-top-level-fixture").await;
    for mode in ["desktop", "organize"] {
        let root = s.root(&format!("audio-{mode}"));
        for i in 0..6 {
            write(&root, &format!("meeting-{i}.mp3"), "fixture");
        }
        let mut t = s.tree(&root, mode, Value::Null).await;
        t=s.post("tree",&t,json!({"nodes":array(&t["nodes"]).iter().filter(|n|n["id"]!="audio").collect::<Vec<_>>()})).await;
        let original = t["nodes"].clone();
        t = s
            .run(
                "suggest_tree",
                &t,
                json!({"message":"为大量音频补齐一级分类和录音子目录"}),
            )
            .await;
        assert_eq!(t["nodes"], original);
        let p = &t["proposal"];
        assert_eq!(array(&p["changes"]).len(), 3);
        let child = array(&p["changes"])
            .iter()
            .find(|c| c["target"] == "new-recordings")
            .unwrap();
        s.blocked(
            "proposal",
            &t,
            json!({"proposal_id":p["id"],"scene":"tree","ids":[child["id"]]}),
        )
        .await;
        assert_eq!(s.current(&t).await["nodes"], original);
        t = s.merge(&t, "tree", None, "completed").await;
        assert_eq!(
            array(&t["nodes"])
                .iter()
                .find(|n| n["id"] == "new-music")
                .unwrap()["parent"],
            "root"
        );
    }
    s.configure(json!({"model":"duplicate-template-fixture"}))
        .await;
    for mode in ["desktop", "organize"] {
        let root = s.root(&format!("photo-{mode}"));
        write(&root, "sample.jpg", "Synthetic name-only fixture");
        let mut t = s
            .tree(
                &root,
                mode,
                json!({"default":"filename_only","rules":[],"content_slice_bytes":0}),
            )
            .await;
        let mut nodes = t["nodes"].clone();
        let photo = nodes
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["name"] == "照片")
            .unwrap();
        photo["examples"] = json!(["sample.jpg"]);
        photo["position"] = json!([800.0, 240.0]);
        let photo = photo.clone();
        t = s.post("tree", &t, json!({"nodes":nodes})).await;
        t = s
            .run(
                "suggest_tree",
                &t,
                json!({"message":"按照默认模板细化照片分类"}),
            )
            .await;
        assert_eq!(t["nodes"], nodes);
        let p = &t["proposal"];
        assert!(text(&p["message"]).contains("复用同名节点"));
        assert_eq!(array(&p["changes"]).len(), 2);
        let edit = array(&p["changes"])
            .iter()
            .find(|c| c["target"] == photo["id"])
            .unwrap();
        assert_eq!(edit["before"], photo);
        assert_eq!(edit["after"]["examples"], photo["examples"]);
        assert_eq!(edit["after"]["position"], photo["position"]);
        assert_eq!(
            array(&p["changes"])
                .iter()
                .find(|c| c["target"] == "photo-travel")
                .unwrap()["after"]["parent"],
            photo["id"]
        );
        assert!(!array(&p["changes"])
            .iter()
            .any(|c| c["target"] == "duplicate-photo"));
        assert_eq!(array(&t["calls"]).len(), 3);
        assert!(array(&s.events(&t).await)
            .iter()
            .any(|e| e["kind"] == "ai_proposal_nodes_reused"));
        t = s.merge(&t, "tree", None, "completed").await;
        assert_eq!(
            array(&t["nodes"])
                .iter()
                .filter(|n| n["parent"] == photo["parent"] && n["name"] == "照片")
                .count(),
            1
        );
        assert_eq!(
            array(&t["nodes"])
                .iter()
                .find(|n| n["id"] == "photo-travel")
                .unwrap()["parent"],
            photo["id"]
        );
        assert_eq!(
            std::fs::read_to_string(root.join("sample.jpg")).unwrap(),
            "Synthetic name-only fixture"
        );
    }
}
