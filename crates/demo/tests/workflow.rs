//! Exercise the packaged executable and real engine. No paid network calls.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
};
struct Server {
    child: std::process::Child,
    _dir: tempfile::TempDir,
    url: String,
    id: String,
    root: String,
    http: reqwest::Client,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn spawn(dir: &Path) -> (std::process::Child, String) {
        let executable = std::env::var_os("DS_DEMO_EXECUTABLE")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_ds-demo").into());
        let mut cmd = Command::new(executable);
        cmd.args(["--no-open", "--port", "0", "--workspace"])
            .arg(dir)
            .current_dir(dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().unwrap();
        let mut line = String::new();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        stdout.read_line(&mut line).unwrap();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        });
        let url = line
            .trim()
            .strip_prefix("DEMO_URL=")
            .unwrap()
            .split("/#")
            .next()
            .unwrap()
            .to_string();
        (child, url)
    }
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (child, url) = Self::spawn(dir.path());
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let meta: Value = http
            .get(format!("{url}/demo/meta"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        Self {
            child,
            _dir: dir,
            url,
            id: meta["task_id"].as_str().unwrap().into(),
            root: meta["root"].as_str().unwrap().into(),
            http,
        }
    }
    async fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        (self.child, self.url) = Self::spawn(self._dir.path());
    }
    async fn control(&mut self, value: Value) {
        let previous = self.get("/demo/meta").await;
        let boot = self.get("/api/bootstrap").await;
        let response = self
            .http
            .post(format!("{}/demo/control", self.url))
            .header("x-ds-token", boot["token"].as_str().unwrap())
            .json(&value)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
        if value["reset"] == true {
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Ok(response) = self
                    .http
                    .get(format!("{}/demo/meta", self.url))
                    .send()
                    .await
                {
                    if let Ok(meta) = response.json::<Value>().await {
                        if meta["generation"].as_u64().unwrap_or(0)
                            > previous["generation"].as_u64().unwrap_or(0)
                        {
                            self.id = meta["task_id"].as_str().unwrap().into();
                            return;
                        }
                    }
                }
            }
            panic!("Reset did not reopen the same server");
        }
    }
    async fn get(&self, path: &str) -> Value {
        self.http
            .get(format!("{}{path}", self.url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn task(&self) -> Value {
        self.get(&format!("/api/tasks/{}", self.id)).await
    }
    async fn act(&self, name: &str, mut args: Value) -> Value {
        let boot = self.get("/api/bootstrap").await;
        let t = self.task().await;
        args["action"] = json!(name);
        args["task_id"] = json!(self.id);
        args["revision"] = t["revision"].clone();
        let response = self
            .http
            .post(format!("{}/api/action", self.url))
            .header("x-ds-token", boot["token"].as_str().unwrap())
            .json(&args)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let v: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{name}: {v}");
        if let Some(id) = v["job"]["id"].as_str() {
            for _ in 0..600 {
                let jobs = self.get("/api/jobs").await;
                if let Some(j) = jobs.as_array().unwrap().iter().find(|j| j["id"] == id) {
                    if !["running", "pausing"].contains(&j["status"].as_str().unwrap()) {
                        assert_eq!(j["status"], "completed", "{name}: {j}");
                        return j.clone();
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            panic!("timeout {name}");
        }
        v
    }
    async fn phase(&self, n: u64) {
        while self.task().await["phase"].as_u64().unwrap() < n {
            self.act("advance", json!({})).await;
        }
    }
    async fn merge(&self) {
        let t = self.task().await;
        let p = &t["proposal"];
        assert!(p.is_object(), "{t}");
        self.act("proposal",json!({"proposal_id":p["id"],"scene":p["scene"],"ids":p["changes"].as_array().unwrap().iter().map(|c|c["id"].clone()).collect::<Vec<_>>()})).await;
    }
}
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, result: &mut BTreeMap<String, Vec<u8>>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                visit(root, &e.path(), result);
            } else {
                result.insert(
                    e.path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fs::read(e.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
#[tokio::test]
async fn desktop_agent_review_execute_restore_archive_and_rename() {
    let mut s = Server::new().await;
    let before = files(Path::new(&s.root));
    s.act("scan", json!({})).await;
    s.phase(1).await;
    s.act("directory", json!({"id":"课程资料库","class":"container"}))
        .await;
    s.phase(2).await;
    s.act(
        "suggest_tree",
        json!({"message":"完善音乐、课程分类并复用资料库"}),
    )
    .await;
    s.merge().await;
    let mut t = s.task().await;
    let example = t["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "progress1.docx")
        .unwrap()["id"]
        .clone();
    for n in t["nodes"].as_array_mut().unwrap() {
        if n["name"] == "课程项目" {
            n["examples"] = json!([example]);
        }
    }
    s.act("tree", json!({"nodes":t["nodes"]})).await;
    s.phase(3).await;
    s.act("plan_ai", json!({"batch_size":2,"thinking":false}))
        .await;
    s.phase(4).await;
    s.act(
        "chat",
        json!({"scene":"review","message":"把视频从其他独立出来"}),
    )
    .await;
    s.merge().await;
    let t = s.task().await;
    let ops = t["operations"].as_array().unwrap();
    assert!(
        ops.iter().any(|o| o["source"] == "FSG1.mp4"
            && o["destination"]
                .as_str()
                .unwrap()
                .starts_with("视频/剪辑素材/")),
        "{ops:?}"
    );
    assert!(ops
        .iter()
        .any(|o| o["source"] == "示例中学八年级2026春期末成绩"
            && o["destination"].as_str().unwrap().starts_with("文档/")));
    assert!(ops
        .iter()
        .all(|o| !o["source"].as_str().unwrap().contains("课程资料库/")));
    let proof = s.get("/demo/evidence").await;
    assert!(proof["maximum"].as_u64().unwrap() >= 2, "{proof}");
    assert!(proof["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["kind"] == "image"));
    #[cfg(windows)]
    for backend in ["windows_pdf", "windows_media"] {
        assert!(
            proof["items"].as_array().unwrap().iter().any(|i| {
                i["kind"] == "text" && i["value"].as_str().unwrap_or("").contains(backend)
            }),
            "Native preview evidence missing: {backend}"
        );
    }
    s.act("review",json!({"selected":ops.iter().filter(|o|o["selected"]==true).map(|o|o["id"].clone()).collect::<Vec<_>>(),"reviewed":true})).await;
    s.phase(5).await;
    s.act("execute", json!({})).await;
    assert!(Path::new(&s.root).join("视频/剪辑素材/FSG1.mp4").is_file());
    s.act("cleanup_ai", json!({})).await;
    assert!(s.task().await["cleanup"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"].as_str().unwrap_or("").ends_with("render.log")));
    s.act("rollback", json!({})).await;
    assert_eq!(files(Path::new(&s.root)), before);
    let job = s.act("archive_export", json!({})).await;
    let result = s
        .get(&format!("/api/jobs/{}/result", job["id"].as_str().unwrap()))
        .await;
    assert_eq!(result["format"], "downloadsweeper-archive");
    s.act("import", json!({"task":result})).await;
    let t = s
        .act("create", json!({"root":s.root,"mode":"rename"}))
        .await;
    s.id = t["id"].as_str().unwrap().into();
    s.act("scan", json!({})).await;
    s.phase(2).await;
    s.act(
        "rename_scope",
        json!({"extensions":["txt"],"web_search":false}),
    )
    .await;
    s.phase(3).await;
    s.act("rename", json!({})).await;
    assert!(s.task().await["operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["destination"] == "Rust Agent 课程答辩提纲.txt"));
    s.control(json!({"reset":true})).await;
    assert_eq!(s.task().await["phase"], 0);
    assert_eq!(files(Path::new(&s.root)), before);
}

#[tokio::test]
async fn restart_and_reset_repeat_fixed_plan() {
    let mut s = Server::new().await;
    let id = s.id.clone();
    let root = s.root.clone();
    let initial = files(Path::new(&root));
    s.act("scan", json!({})).await;
    s.phase(1).await;
    s.control(json!({"step":1})).await;
    let snapshot = s.task().await["entries"].clone();
    s.restart().await;
    assert_eq!(s.task().await["entries"], snapshot);
    assert_eq!(s.get("/demo/meta").await["step"], 1);
    let mut layout = Value::Null;
    for run in 0..2 {
        s.phase(4).await;
        let t = s.task().await;
        let ops = t["operations"].as_array().unwrap();
        let actual = json!(ops
            .iter()
            .map(|o| json!([o["source"], o["destination"]]))
            .collect::<Vec<_>>());
        if run == 0 {
            layout = actual;
        } else {
            assert_eq!(actual, layout);
        }
        s.act("review",json!({"selected":ops.iter().filter(|o|o["selected"]==true).map(|o|o["id"].clone()).collect::<Vec<_>>(),"reviewed":true})).await;
        s.phase(5).await;
        s.act("execute", json!({})).await;
        s.control(json!({"reset":true})).await;
        assert_eq!(s.id, id);
        assert_eq!(s.get("/demo/meta").await["root"], root);
        assert_eq!(s.get("/demo/meta").await["step"], 0);
        assert_eq!(files(Path::new(&root)), initial);
        if run == 0 {
            s.act("scan", json!({})).await;
            s.phase(1).await;
        }
    }
}
