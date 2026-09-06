#![allow(dead_code)]
pub mod media;
pub mod mock;
pub use mock::Mock;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};
pub fn array(v: &Value) -> &[Value] {
    v.as_array().expect("array")
}
pub fn text(v: &Value) -> &str {
    v.as_str().expect("string")
}
pub fn number(v: &Value) -> u64 {
    v.as_u64().expect("unsigned integer")
}
pub fn ids(v: &Value) -> Value {
    json!(array(v).iter().map(|v| v["id"].clone()).collect::<Vec<_>>())
}
pub fn node(id: &str, parent: Option<&str>, name: &str, extensions: &[&str]) -> Value {
    json!({"id":id,"parent":parent,"name":name,"rule_type":if extensions.is_empty(){"complex"}else{"simple"},"extensions":extensions,"note":"","examples":[],"mapping":null,"position":null})
}
pub fn write(root: &Path, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}
pub fn hashes(root: &Path) -> BTreeMap<String, String> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                format!("{:x}", Sha256::digest(fs::read(e.path()).unwrap())),
            )
        })
        .collect()
}
pub fn command(exe: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(exe);
    c.env_clear();
    // Only OS/runtime essentials. Never inherit .env, keys, proxies or DS_* settings.
    for name in [
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "HOME",
        "USERPROFILE",
        "PATH",
    ] {
        if let Some(value) = std::env::var_os(name) {
            c.env(name, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c
}
pub struct Server {
    child: Option<Child>,
    pub dir: tempfile::TempDir,
    pub base: String,
    pub token: String,
    pub client: reqwest::Client,
    pub mock: Mock,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Server {
    pub async fn new(model: &str) -> Self {
        let mock = Mock::start().await;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(),"config.toml",format!("token_budget=500000\n[llm]\nendpoint=\"{}/v1\"\nmodel=\"{model}\"\napi_key_env=\"DS_TEST_MISSING\"\ncontext_length=256000\nmax_output_tokens=32768\nparallel_requests=1\n", mock.url));
        let mut s = Self {
            child: None,
            dir,
            base: String::new(),
            token: String::new(),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(25))
                .build()
                .unwrap(),
            mock,
        };
        s.start().await;
        s
    }
    pub fn root(&self, name: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    pub async fn restart(&mut self) {
        self.stop();
        self.start().await;
    }
    pub async fn start(&mut self) {
        let log = self.dir.path().join("server.log");
        let out = fs::File::create(&log).unwrap();
        let mut cmd = command(env!("CARGO_BIN_EXE_ds-web"));
        cmd.args(["--port", "0", "--config"])
            .arg(self.dir.path().join("config.toml"))
            .arg("--data-dir")
            .arg(self.dir.path().join("data"))
            .current_dir(self.dir.path())
            .stdout(out.try_clone().unwrap())
            .stderr(out);
        // Native decoding tests must prove no external decoder is necessary.
        if cfg!(windows) {
            cmd.env(
                "PATH",
                PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32"),
            );
        } else {
            cmd.env("PATH", "/usr/bin:/bin");
        }
        if let Some(bin) = std::env::var_os("DS_TEST_MEDIA_BIN") {
            cmd.env("PATH", bin);
        }
        self.child = Some(cmd.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let data = fs::read_to_string(&log).unwrap();
            if let Some(url) = data
                .split_whitespace()
                .find(|w| w.starts_with("http://127.0.0.1:"))
            {
                self.base = url.trim_end_matches('/').into();
                break;
            }
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "server exited: {data}"
            );
            assert!(Instant::now() < deadline, "startup timeout: {data}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.token = text(&self.get("/api/bootstrap").await["token"]).into();
    }
    pub async fn get(&self, path: &str) -> Value {
        let r = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap();
        let status = r.status();
        let v: Value = r.json().await.unwrap();
        assert!(status.is_success(), "GET {path}: {v}");
        v
    }
    pub async fn request(&self, path: &str, payload: Value, expected: u16, session: bool) -> Value {
        let mut r = self
            .client
            .post(format!("{}{path}", self.base))
            .json(&payload);
        if session {
            r = r.header("x-ds-token", &self.token);
        }
        let r = r.send().await.unwrap();
        let status = r.status().as_u16();
        let v: Value = r.json().await.unwrap();
        assert_eq!(status, expected, "{path}: {v}");
        v
    }
    fn payload(action: &str, task: &Value, mut args: Value) -> Value {
        args["action"] = json!(action);
        if !task.is_null() {
            args["task_id"] = task["id"].clone();
            args["revision"] = task["revision"].clone();
        }
        args
    }
    pub async fn post(&self, action: &str, task: &Value, args: Value) -> Value {
        self.request("/api/action", Self::payload(action, task, args), 200, true)
            .await
    }
    pub async fn blocked(&self, action: &str, task: &Value, args: Value) -> Value {
        self.request("/api/action", Self::payload(action, task, args), 400, true)
            .await
    }
    pub async fn act(&self, action: &str, task: &Value) -> Value {
        self.post(action, task, json!({})).await
    }
    pub async fn current(&self, task: &Value) -> Value {
        self.get(&format!("/api/tasks/{}", text(&task["id"]))).await
    }
    pub async fn events(&self, task: &Value) -> Value {
        self.get(&format!("/api/tasks/{}/trajectory", text(&task["id"])))
            .await
    }
    pub async fn wait(&self, result: Value, status: &str) -> Value {
        self.wait_observed(result, status, &mut Vec::new()).await
    }
    pub async fn wait_observed(
        &self,
        result: Value,
        status: &str,
        samples: &mut Vec<Value>,
    ) -> Value {
        let job = &result["job"];
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let state = self.get("/api/bootstrap").await;
            if state["job"].is_null() {
                assert_eq!(state["last_job"]["id"], job["id"]);
                assert_eq!(state["last_job"]["status"], status, "{}", state["last_job"]);
                if job["task_id"].is_string()
                    && job["task_id"] != "00000000-0000-0000-0000-000000000000"
                {
                    return self
                        .get(&format!("/api/tasks/{}", text(&job["task_id"])))
                        .await;
                }
                return state["last_job"].clone();
            }
            if !state["job"]["parallel"].is_null() {
                samples.push(state["job"]["parallel"].clone());
            }
            assert!(Instant::now() < deadline, "job timed out: {}", state["job"]);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    pub async fn run(&self, action: &str, task: &Value, args: Value) -> Value {
        self.wait(self.post(action, task, args).await, "completed")
            .await
    }
    pub async fn step(&self, task: &Value) -> Value {
        let t = self.act("advance", task).await;
        if t["job"].is_object() {
            self.wait(t, "completed").await
        } else {
            t
        }
    }
    pub async fn scan(&self, root: &Path, mode: &str) -> Value {
        self.run(
            "scan",
            &self
                .post("create", &Value::Null, json!({"root":root,"mode":mode}))
                .await,
            json!({}),
        )
        .await
    }
    pub async fn tree(&self, root: &Path, mode: &str, permissions: Value) -> Value {
        let mut t = self.step(&self.scan(root, mode).await).await;
        if !permissions.is_null() {
            t = self
                .post("permissions", &t, json!({"permissions":permissions}))
                .await;
        }
        self.step(&t).await
    }
    pub async fn planning(&self, root: &Path, mode: &str, permissions: Value) -> Value {
        self.step(&self.tree(root, mode, permissions).await).await
    }
    pub async fn approve(&self, t: &Value) -> Value {
        let t = if t["phase"] == 3 {
            self.step(t).await
        } else {
            t.clone()
        };
        let t = self
            .post(
                "review",
                &t,
                json!({"selected":ids(&t["operations"]),"reviewed":true}),
            )
            .await;
        self.step(&t).await
    }
    pub async fn config(&self) -> Value {
        self.get("/api/bootstrap").await["config"].clone()
    }
    pub async fn configure(&self, changes: Value) {
        let mut c = self.config().await;
        for (k, v) in changes.as_object().unwrap() {
            c["llm"][k] = v.clone();
        }
        self.post("config", &Value::Null, json!({"config":c})).await;
    }
    pub async fn budget(&self, budget: Value) {
        let mut c = self.config().await;
        c["token_budget"] = budget;
        self.post("config", &Value::Null, json!({"config":c})).await;
    }
    pub async fn merge(
        &self,
        t: &Value,
        scene: &str,
        selection: Option<Value>,
        status: &str,
    ) -> Value {
        let p = &t["proposal"];
        let r=self.post("proposal",t,json!({"proposal_id":p["id"],"scene":scene,"ids":selection.unwrap_or_else(||ids(&p["changes"]))})).await;
        if r["job"].is_object() {
            self.wait(r, status).await
        } else {
            assert_eq!(status, "completed");
            r
        }
    }
}
