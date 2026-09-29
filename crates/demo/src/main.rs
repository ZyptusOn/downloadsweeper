//! Standalone offline demo: real engine/UI behind a loopback, fixture-only gateway.
mod fixtures;
mod model;
mod session;
use anyhow::{ensure, Result};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ds_engine::{config::AppConfig, safe_fs};
use futures::StreamExt;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone)]
struct Demo {
    base: PathBuf,
    upstream: String,
    origin: String,
    root: PathBuf,
    task_id: String,
    client: reqwest::Client,
    live: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    evidence: Arc<Mutex<Vec<Value>>>,
    control: Arc<tokio::sync::Mutex<()>>,
    resetting: Arc<AtomicBool>,
    reset_notify: Arc<tokio::sync::Notify>,
}
fn main() -> Result<()> {
    ds_web::dispatch_media_helper();
    let args = std::env::args().collect::<Vec<_>>();
    let option = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let base = option("--workspace").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("DownloadSweeper-Demo/desktop-v1")
    });
    let base = std::path::absolute(base)?;
    let mut port = option("--port")
        .map(|p| p.parse::<u16>())
        .transpose()?
        .unwrap_or(33187);
    let mut reset = args.iter().any(|a| a == "--reset");
    let mut first = true;
    while let Some(next_port) = run(base.clone(), reset, port, first)? {
        reset = true;
        port = next_port;
        first = false;
    }
    Ok(())
}
#[tokio::main]
async fn run(base: PathBuf, reset: bool, port: u16, first: bool) -> Result<Option<u16>> {
    let args = std::env::args().collect::<Vec<_>>();
    let owned = session::open(base, reset)?;
    let base = owned.base.clone();
    let root = owned.root.clone();
    // Keep native decoder/COM caches beside this run, never in the source tree.
    std::env::set_current_dir(&base)?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let port = listener.local_addr()?.port();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let mut cfg = AppConfig::default();
    cfg.scan_root = root.clone();
    cfg.permissions = fixtures::permissions();
    cfg.llm.endpoint = format!("{origin}/v1");
    cfg.llm.model = "offline-demo".into();
    cfg.llm.multimodal = true;
    cfg.llm.context_length = 131072;
    cfg.llm.max_output_tokens = 8192;
    cfg.llm.parallel_requests = 3;
    cfg.llm.api_key.clear();
    cfg.llm.api_key_env = Some(format!("DS_DEMO_UNUSED_{}", uuid::Uuid::new_v4().simple()));
    cfg.search.enabled = false;
    cfg.llm.pricing.mode = "manual".into();
    cfg.llm.pricing.input_per_1k_usd = 0.001;
    cfg.llm.pricing.output_per_1k_usd = 0.002;
    let config = base.join("config.toml");
    cfg.save(&config)?;
    let task = &owned.task;
    let (upstream, _server) = ds_web::start(0, base.join("data"), config).await?;
    let proof: Value = std::fs::read(base.join("demo-evidence.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let resetting = Arc::new(AtomicBool::new(false));
    let reset_notify = Arc::new(tokio::sync::Notify::new());
    let state = Demo {
        base: base.clone(),
        upstream,
        origin: origin.clone(),
        root,
        task_id: task.id.to_string(),
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        live: Default::default(),
        maximum: Arc::new(AtomicUsize::new(
            proof["maximum"].as_u64().unwrap_or(0) as usize
        )),
        requests: Arc::new(AtomicUsize::new(
            proof["requests"].as_u64().unwrap_or(0) as usize
        )),
        evidence: Arc::new(Mutex::new(
            proof["items"].as_array().cloned().unwrap_or_default(),
        )),
        control: Default::default(),
        resetting: resetting.clone(),
        reset_notify: reset_notify.clone(),
    };
    let app=Router::new().route("/demo/meta",get(meta)).route("/demo/evidence",get(evidence))
        .route("/demo/control",post(control))
        .route("/demo.js",get(||async{([(axum::http::header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../assets/demo.js"))}))
        .route("/demo.css",get(||async{([(axum::http::header::CONTENT_TYPE,"text/css; charset=utf-8")],include_str!("../assets/demo.css"))}))
        .route("/v1/chat/completions",post(model_reply)).route("/v1/models",get(||async{Json(json!({"data":[{"id":"offline-demo","context_length":131072,"capabilities":{"vision":true}}]}))}))
        .fallback(proxy).layer(axum::extract::DefaultBodyLimit::max(32*1024*1024)).with_state(state);
    let url = format!("{origin}/#task={}", task.id);
    std::fs::write(base.join("OPEN_URL.txt"), &url)?;
    println!("DEMO_URL={url}\nDEMO_DATA={}\nOffline scripted model; real synthetic-file operations; no API charges.",base.display());
    if first && !args.iter().any(|a| a == "--no-open") {
        ds_web::open_browser(&url);
    }
    // Dropping both HTTP servers also closes SSE streams; they must not hold reset open.
    let serving = axum::serve(listener, app);
    tokio::select! {
        result = serving => { result?; },
        _ = tokio::signal::ctrl_c() => {},
        _ = reset_notify.notified() => {},
    }
    _server.abort();
    let _ = _server.await;
    Ok(resetting.load(Ordering::SeqCst).then_some(port))
}
async fn meta(State(d): State<Demo>) -> Json<Value> {
    let saved = session::state(&d.base).unwrap_or(Value::Null);
    Json(
        json!({"root":d.root,"task_id":d.task_id,"offline":true,"step":session::effective_step(&d.base,&saved),"generation":saved["generation"].as_u64().unwrap_or(0),"maximum":d.maximum.load(Ordering::SeqCst),"requests":d.requests.load(Ordering::SeqCst)}),
    )
}
async fn control(State(d): State<Demo>, request: Request) -> Response {
    let result = async {
        let _guard = d.control.lock().await;
        ensure!(!d.resetting.load(Ordering::SeqCst), "正在重置演示");
        let (parts, body) = request.into_parts();
        ensure!(
            parts.headers.get("host").and_then(|v| v.to_str().ok())
                == Some(d.origin.trim_start_matches("http://")),
            "拒绝异源访问"
        );
        if let Some(origin) = parts.headers.get("origin") {
            ensure!(origin.to_str()? == d.origin, "拒绝跨站请求");
        }
        let boot: Value = d
            .client
            .get(format!("{}/api/bootstrap", d.upstream))
            .send()
            .await?
            .json()
            .await?;
        ensure!(
            parts
                .headers
                .get("x-ds-token")
                .and_then(|v| v.to_str().ok())
                == boot["token"].as_str(),
            "无效会话令牌"
        );
        let value: Value = serde_json::from_slice(&to_bytes(body, 4096).await?)?;
        if value["reset"] == true {
            ensure!(
                boot["job"].is_null(),
                "请等待当前任务完成，或先暂停任务，再重置演示"
            );
            d.resetting.store(true, Ordering::SeqCst);
            let notify = d.reset_notify.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                notify.notify_one();
            });
        } else {
            session::step(
                &d.base,
                value["step"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("缺少步骤"))?,
            )?;
        }
        Ok::<_, anyhow::Error>(Json(json!({"ok":true})))
    }
    .await;
    match result {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}
async fn evidence(State(d): State<Demo>) -> Json<Value> {
    Json(
        json!({"items":*d.evidence.lock().unwrap(),"maximum":d.maximum.load(Ordering::SeqCst),"requests":d.requests.load(Ordering::SeqCst),"notice":"真实证据提取，预设模拟回答；Token 和费用为模拟数值，实际 API 费用为零。"}),
    )
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn model_reply(State(d): State<Demo>, Json(body): Json<Value>) -> Json<Value> {
    let current = d.live.fetch_add(1, Ordering::SeqCst) + 1;
    d.maximum.fetch_max(current, Ordering::SeqCst);
    d.requests.fetch_add(1, Ordering::SeqCst);
    let _active = Active(d.live.clone());
    {
        let mut records = d.evidence.lock().unwrap();
        for message in model::array(&body["messages"]) {
            for block in model::array(&message["content"]) {
                if let Some(url) = block["image_url"]["url"]
                    .as_str()
                    .filter(|s| s.starts_with("data:image/"))
                {
                    let item = json!({"kind":"image","value":url});
                    if !records.contains(&item) && records.len() < 20 {
                        records.push(item);
                    }
                }
            }
            if message["role"] == "tool" {
                let value = model::text(&message["content"]);
                if value.contains("text_excerpt")
                    || value.contains("children")
                    || value.contains("sampled_pages")
                    || value.contains("image_index")
                {
                    let item =
                        json!({"kind":"text","value":value.chars().take(3500).collect::<String>()});
                    if !records.contains(&item) && records.len() < 20 {
                        records.push(item);
                    }
                }
            }
        }
    }
    {
        let proof = json!({"items":*d.evidence.lock().unwrap(),"maximum":d.maximum.load(Ordering::SeqCst),"requests":d.requests.load(Ordering::SeqCst)});
        if let Err(error) = safe_fs::atomic_json(&d.base.join("demo-evidence.json"), &proof) {
            eprintln!("演示证据保存失败：{error}");
        }
    }
    model::respond(Json(body)).await
}
fn restricted_action(body: &Value, d: &Demo) -> Result<()> {
    match model::text(&body["action"]) {
        "config" => {
            anyhow::bail!("离线演示连接已固定，无需填写 Key；模型、价格与计费用量均为模拟。")
        }
        "create" => {
            let root = body["root"].as_str().unwrap_or("");
            ensure!(
                std::fs::canonicalize(root).ok() == std::fs::canonicalize(&d.root).ok(),
                "演示仅允许使用本包的合成桌面"
            );
        }
        "import" => ensure!(
            body["task"]["format"] == "downloadsweeper-archive",
            "演示仅允许导入只读归档"
        ),
        "resume_archive" => anyhow::bail!("演示归档保持只读；重新启动演示会创建新的合成桌面。"),
        _ => {}
    }
    Ok(())
}
async fn proxy(State(d): State<Demo>, req: Request) -> Response {
    let _guard = if req.method() == axum::http::Method::POST {
        Some(d.control.clone().lock_owned().await)
    } else {
        None
    };
    if d.resetting.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "正在重置演示").into_response();
    }
    match forward(d, req).await {
        Ok(r) => r,
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}
async fn forward(d: Demo, req: Request) -> Result<Response> {
    let (parts, body) = req.into_parts();
    let host = parts
        .headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    ensure!(
        host == d.origin.trim_start_matches("http://"),
        "演示仅接受本机同源访问"
    );
    if let Some(origin) = parts.headers.get("origin") {
        ensure!(origin.to_str()? == d.origin, "拒绝跨站写入");
    }
    let path = parts.uri.path();
    let bytes = to_bytes(body, 32 * 1024 * 1024).await?;
    if path == "/api/action" {
        restricted_action(&serde_json::from_slice::<Value>(&bytes)?, &d)?;
    }
    ensure!(
        !matches!(path, "/api/models" | "/api/pricing-preview"),
        "离线演示使用固定本地模型与模拟单价；不会连接外部服务"
    );
    let uri = format!(
        "{}{}",
        d.upstream,
        parts
            .uri
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("/")
    );
    let mut request = d.client.request(parts.method, uri);
    for name in ["content-type", "x-ds-token", "accept"] {
        if let Some(value) = parts.headers.get(name) {
            request = request.header(name, value);
        }
    }
    request = request.header("origin", &d.upstream).body(bytes);
    let response = request.send().await?;
    let status = response.status();
    let mut headers = response.headers().clone();
    headers.remove("content-length");
    headers.remove("transfer-encoding");
    let body = if path == "/" {
        let html=response.text().await?.replace("</head>","<link rel=\"stylesheet\" href=\"/demo.css\"><script type=\"module\" src=\"/demo.js\"></script></head>");
        Body::from(html)
    } else if path == "/app.js" {
        let js = response.text().await?;
        let marker = "  taskRef.current = task;";
        ensure!(js.contains(marker), "演示 UI 适配标记不匹配");
        Body::from(js.replacen(marker,&format!("{marker}\n  window.dsDemoBridge = {{ open: id => open(id), page: p => pageTo(p), assistant: (visible = true) => setAssistantOpen(visible) }};"),1))
    } else if path == "/api/bootstrap" {
        let mut b: Value = response.json().await?;
        b["config"]["default_desktop_root"] = json!(d.root);
        b["config"]["default_scan_root"] = json!(d.root);
        Body::from(b.to_string())
    } else {
        Body::from_stream(
            response
                .bytes_stream()
                .map(|chunk| chunk.map_err(std::io::Error::other)),
        )
    };
    let mut out = Response::new(body);
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    out.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn demo_rejects_real_roots_and_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let d = Demo {
            base: dir.path().to_path_buf(),
            upstream: String::new(),
            origin: String::new(),
            root: dir.path().to_path_buf(),
            task_id: String::new(),
            client: reqwest::Client::new(),
            live: Default::default(),
            maximum: Default::default(),
            requests: Default::default(),
            evidence: Default::default(),
            control: Default::default(),
            resetting: Default::default(),
            reset_notify: Default::default(),
        };
        assert!(restricted_action(&json!({"action":"create","root":d.root}), &d).is_ok());
        assert!(restricted_action(&json!({"action":"create","root":"C:/"}), &d).is_err());
        assert!(restricted_action(&json!({"action":"config"}), &d).is_err());
        assert!(restricted_action(&json!({"action":"import","task":{"root":"C:/"}}), &d).is_err());
    }
}
