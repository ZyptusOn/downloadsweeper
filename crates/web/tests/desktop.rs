mod support;
use serde_json::{json, Value};
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn desktop_six_stages_selected_moves_and_lossless_restore() {
    let s = Server::new("local-test").await;
    assert!(s.config().await.get("default_desktop_root").is_some());
    let root = s.root("Desktop");
    for n in [
        "待办.txt",
        "周报.docx",
        "资料.pdf",
        "截图.png",
        "网站.url",
        "~$周报.docx",
        "项目/内部/private.txt",
    ] {
        write(&root, n, format!("synthetic fixture {n}"));
    }
    let before = hashes(&root);
    let mut t = s
        .post(
            "create",
            &Value::Null,
            json!({"root":root,"mode":"desktop"}),
        )
        .await;
    s.blocked("desktop_plan", &t, json!({})).await;
    t = s.run("scan", &t, json!({})).await;
    assert!(array(&t["entries"])
        .iter()
        .all(|e| e["parent"].as_str().unwrap_or("").is_empty()));
    s.blocked("directory", &t, json!({"id":"项目","class":"normal"}))
        .await;
    for phase in 1..=4 {
        t = s.step(&t).await;
        assert_eq!(t["phase"], phase);
    }
    assert_eq!(array(&t["operations"]).len(), 4);
    assert!(array(&t["calls"]).is_empty());
    assert_eq!(array(&t["retained"]).len(), 3);
    assert_eq!(hashes(&root), before);
    for action in ["execute", "advance"] {
        s.blocked(action, &t, json!({})).await;
    }
    t = s.post("back", &t, json!({"phase":0})).await;
    assert_eq!(t["scanned"], true);
    assert!(array(&t["operations"]).is_empty());
    t = s.run("desktop_plan", &t, json!({})).await;
    let selected: Vec<_> = array(&t["operations"])
        .iter()
        .filter(|o| o["source"] != "周报.docx")
        .map(|o| o["id"].clone())
        .collect();
    t = s
        .post("review", &t, json!({"selected":selected,"reviewed":true}))
        .await;
    t = s.step(&t).await;
    t = s.run("execute", &t, json!({})).await;
    assert_eq!(t["status"], "completed");
    for n in ["周报.docx", "项目/内部/private.txt", "网站.url"] {
        assert!(root.join(n).exists());
    }
    assert_eq!(
        array(&t["operations"])
            .iter()
            .filter(|o| o["status"] == "done")
            .count(),
        3
    );
    t = s.run("rollback", &t, json!({})).await;
    assert_eq!(t["status"], "rolled_back");
    assert_eq!(hashes(&root), before);
    assert!(array(&t["calls"]).is_empty() && array(&t["pending_calls"]).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn desktop_complete_agent_custom_tree_reused_container_and_examples() {
    let s = Server::new("classification-evidence-fixture").await;
    let root = s.root("Desktop");
    for n in [
        "brief.txt",
        "reference.txt",
        "shortcut.url",
        ".hidden.txt",
        "~$locked.txt",
        "Protected/private.txt",
    ] {
        write(&root, n, format!("Synthetic desktop project notes {n}"));
    }
    let before = hashes(&root);
    let mut t = s
        .tree(
            &root,
            "desktop",
            json!({"default":"filename_only","rules":[],"content_slice_bytes":128}),
        )
        .await;
    let default_nodes = t["nodes"].clone();
    assert_eq!(t["classification_readiness"]["eligible_files"], 3);
    t = s.step(&t).await;
    t = s.run("plan_ai", &t, json!({})).await;
    assert_eq!(t["classification"]["completed"], 3);
    assert!(!array(&t["calls"]).is_empty());
    assert_eq!(t["nodes"], default_nodes);
    assert_eq!(hashes(&root), before);
    let mut legacy = s.post("back", &t, json!({"phase":1})).await;
    legacy=s.post("permissions",&legacy,json!({"permissions":{"default":"filename_only","rules":[{"extensions":["@folder"],"tier":"none"}],"content_slice_bytes":128}})).await;
    legacy = s.step(&legacy).await;
    legacy=s.post("tree",&legacy,json!({"nodes":array(&default_nodes).iter().filter(|n|n["rule_type"]=="simple").collect::<Vec<_>>()})).await;
    legacy = s.step(&legacy).await;
    assert_eq!(legacy["classification_readiness"]["no_semantic_rule"], 2);
    let error = s.blocked("plan_ai", &legacy, json!({})).await;
    assert!(text(&error["error"]).contains("未调用 AI"));
    assert_eq!(s.current(&legacy).await, legacy);
    assert_eq!(legacy["plan_source"], "rules");
    assert!(legacy["classification"].is_null());
    let offset = s.mock.bodies().len();
    let mut t = s.step(&s.scan(&root, "desktop").await).await;
    assert!(array(&s.config().await["permission_presets"]).len() >= 14);
    let entries = ids(&t["entries"]);
    t = s
        .post(
            "directory",
            &t,
            json!({"id":"Protected","class":"container"}),
        )
        .await;
    assert_eq!(ids(&t["entries"]), entries);
    t=s.post("permissions",&t,json!({"permissions":{"default":"filename_only","content_slice_bytes":128,"rules":[{"category":"text","extensions":["txt","customtext"],"tier":"content_slice"}]}})).await;
    t = s.current(&t).await;
    assert_eq!(t["permissions"]["rules"][0]["category"], "text");
    assert_eq!(
        t["permissions"]["rules"][0]["extensions"],
        json!(["txt", "customtext"])
    );
    t = s.step(&t).await;
    t = s
        .run(
            "suggest_tree",
            &t,
            json!({"message":"Suggest desktop project categories using the local template"}),
        )
        .await;
    assert_eq!(t["inspection"]["status"], "complete");
    t = s.merge(&t, "tree", None, "completed").await;
    let mut nodes = array(&t["nodes"]).to_vec();
    let top = nodes
        .iter_mut()
        .find(|n| array(&n["extensions"]).contains(&json!("txt")))
        .unwrap();
    top["name"] = json!("Desktop Projects");
    top["mapping"] = json!("Protected");
    let parent = text(&top["id"]).to_owned();
    nodes.retain(|n| !text(&n["id"]).starts_with("desktop-text-semantic-"));
    let mut notes = node("project-notes", Some(&parent), "Notes", &[]);
    notes["note"] = json!("Project briefs and references");
    notes["examples"] = json!(["reference.txt"]);
    nodes.push(notes);
    let mut unused = node("unused", None, "Unused", &[]);
    unused["rule_type"] = json!("simple");
    unused["position"] = json!([-200, 400]);
    nodes.push(unused);
    t = s.post("tree", &t, json!({"nodes":nodes})).await;
    let nodes = t["nodes"].clone();
    assert_eq!(
        array(&nodes).iter().find(|n| n["id"] == "unused").unwrap()["position"],
        json!([-200.0, 400.0])
    );
    t = s.step(&t).await;
    t = s.run("plan_ai", &t, json!({"batch_size":8})).await;
    assert_eq!(t["nodes"], nodes);
    assert_eq!(t["classification"]["completed"], 2);
    assert_eq!(array(&t["operations"]).len(), 2);
    assert!(array(&t["operations"])
        .iter()
        .all(|o| text(&o["destination"]).starts_with("Protected/Notes/")));
    assert!(array(&t["calls"])
        .iter()
        .all(|c| number(&c["usage"]["prompt_tokens"]) > 0));
    assert_eq!(hashes(&root), before);
    let wire = s.mock.bodies();
    let wire = &wire[offset..];
    assert!(wire
        .iter()
        .flat_map(|r| array(&r["messages"]))
        .any(|m| m["role"] == "tool"));
    assert!(!json!(wire).to_string().contains("private.txt"));
    t = s.approve(&t).await;
    t = s.run("execute", &t, json!({})).await;
    assert!(root.join("Protected/Notes/brief.txt").is_file());
    s.run("rollback", &t, json!({})).await;
    assert_eq!(hashes(&root), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whole_folder_agent_privacy_manifest_and_lossless_move() {
    let s = Server::new("directory-classification-fixture").await;
    let name = "xx中学八年级2026春期末考成绩";
    let allowed = [
        "按班级总分平均分.xlsx",
        "按班级各科平均分.xlsx",
        "全校学生成绩排名.xlsx",
    ];
    for mode in ["desktop", "organize"] {
        let root = s.root(mode);
        for n in allowed {
            write(
                &root,
                &format!("{name}/{n}"),
                "Synthetic spreadsheet fixture",
            );
        }
        write(
            &root,
            &format!("{name}/DENIED_PRIVATE.docx"),
            "PRIVATE_CONTENT_MUST_NOT_LEAK",
        );
        write(&root, "shortcut.url", "URL=http://invalid.local");
        let mut before = hashes(&root);
        let mut t = s.step(&s.scan(&root, mode).await).await;
        t = s
            .post("directory", &t, json!({"id":name,"class":"atomic"}))
            .await;
        t=s.post("permissions",&t,json!({"permissions":{"default":"filename_only","rules":[{"extensions":["docx","url"],"tier":"none"}],"content_slice_bytes":128}})).await;
        t = s.step(&t).await;
        t = s.step(&t).await;
        let offset = s.mock.bodies().len();
        t = s.run("plan_ai", &t, json!({})).await;
        assert_eq!(t["classification"]["completed"], 1);
        let mut op = array(&t["operations"])
            .iter()
            .find(|o| o["source"] == name)
            .unwrap()
            .clone();
        assert_eq!(op["kind"], "directory");
        assert!(text(&op["destination"]).ends_with(&format!("/{name}")));
        assert!(text(&op["destination"])
            .split('/')
            .next()
            .unwrap()
            .contains("文档"));
        assert!(!array(&t["operations"])
            .iter()
            .any(|o| text(&o["source"]).starts_with(&format!("{name}/"))));
        if mode == "desktop" {
            assert!(!op["directory_manifest"].is_null());
        }
        assert_eq!(hashes(&root), before);
        let bodies = s.mock.bodies();
        let payload = json!(&bodies[offset..]).to_string();
        for n in allowed {
            assert!(payload.contains(n));
        }
        assert!(
            !payload.contains("DENIED_PRIVATE")
                && !payload.contains("PRIVATE_CONTENT_MUST_NOT_LEAK")
        );
        assert!(bodies[offset..]
            .iter()
            .flat_map(|r| array(&r["messages"]))
            .any(|m| m["role"] == "tool"));
        t = s.step(&t).await;
        t = s
            .post("review", &t, json!({"selected":[op["id"]],"reviewed":true}))
            .await;
        t = s.step(&t).await;
        if mode == "desktop" {
            write(
                &root,
                &format!("{name}/{}", allowed[0]),
                "Changed fixture after review",
            );
            t = s.wait(s.act("execute", &t).await, "failed").await;
            assert!(root.join(name).is_dir());
            assert!(!root.join(text(&op["destination"])).exists());
            before = hashes(&root);
            t = s.post("back", &t, json!({"phase":3})).await;
            t = s.run("plan_ai", &t, json!({})).await;
            op = array(&t["operations"])
                .iter()
                .find(|o| o["source"] == name)
                .unwrap()
                .clone();
            t = s.step(&t).await;
            t = s
                .post("review", &t, json!({"selected":[op["id"]],"reviewed":true}))
                .await;
            t = s.step(&t).await;
        }
        t = s.run("execute", &t, json!({})).await;
        assert!(!root.join(name).exists());
        assert!(root
            .join(text(&op["destination"]))
            .join(allowed[0])
            .is_file());
        s.run("rollback", &t, json!({})).await;
        assert_eq!(hashes(&root), before);
    }
}
