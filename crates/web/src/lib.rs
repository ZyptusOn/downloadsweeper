use anyhow::{ensure, Context};
use axum::{
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use ds_engine::{
    config::AppConfig,
    jobs::Checkpoint,
    safe_fs::{self, TaskStore},
    workflow::{Node, Task},
    workflow_ai,
};
use futures::{FutureExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use ds_engine::jobs::Job;
struct Active {
    job: Job,
    cancel: CancellationToken,
    checkpoint: Checkpoint,
    last_save: std::time::Instant,
    persistence_error: Option<String>,
}
pub struct App {
    config: Mutex<AppConfig>,
    config_path: PathBuf,
    pub store: TaskStore,
    active: Mutex<Option<Active>>,
    last_job: Mutex<Option<Job>>,
    events: broadcast::Sender<Value>,
    token: String,
    origin: String,
}
type Shared = Arc<App>;

pub struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(value: E) -> Self {
        Self(value.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":format!("{:#}",self.0)})),
        )
            .into_response()
    }
}
type ApiResult = Result<Json<Value>, ApiError>;

pub async fn start(
    port: u16,
    data: PathBuf,
    config_path: PathBuf,
) -> anyhow::Result<(String, tokio::task::JoinHandle<std::io::Result<()>>)> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let store = TaskStore::new(data.join("tasks"))?;
    for mut task in store.list()? {
        if let Err(e) = safe_fs::recover(&mut task, &store) {
            eprintln!("任务恢复检查失败 {}: {e}", task.id);
        }
        if let Err(e) = ds_engine::response_cache::recover(&store, &mut task) {
            eprintln!("模型回答检查点恢复失败 {}: {e}", task.id);
        }
    }
    let recovered = store.recover_jobs()?;
    let last_job = recovered.first().map(|c| c.job.clone());
    let (events, _) = broadcast::channel(256);
    let app = Arc::new(App {
        config: Mutex::new(AppConfig::load(&config_path)?),
        config_path,
        store,
        active: Mutex::new(None),
        last_job: Mutex::new(last_job),
        events,
        token: Uuid::new_v4().to_string(),
        origin: origin.clone(),
    });
    let router = router(app);
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
    });
    Ok((origin, handle))
}

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/api/bootstrap", get(bootstrap))
        .route("/api/tasks", get(list_tasks))
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/{id}/result", get(job_result))
        .route("/api/tasks/{id}", get(get_task))
        .route("/api/tasks/{id}/trajectory", get(trajectory))
        .route("/api/tasks/{id}/archive", get(export_archive))
        .route("/api/archives", get(list_archives))
        .route("/api/archives/{id}", get(get_archive))
        .route("/api/events", get(events))
        .route("/api/action", post(action))
        .route("/api/models", post(discover_models))
        .route("/api/pricing-preview", post(pricing_preview))
        .route("/api/model-presets", get(model_presets))
        .route(
            "/api/media-capabilities",
            get(|| async { Json(ds_engine::evidence::capabilities()) }),
        )
        .route("/", get(index))
        .route("/{*path}", get(asset))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

async fn guard(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let expected = app.origin.trim_start_matches("http://");
    let origin = req.headers().get("origin").and_then(|h| h.to_str().ok());
    if host != expected || origin.is_some_and(|o| o != app.origin) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"仅允许本地同源访问，请使用启动时显示的 127.0.0.1 地址"})),
        )
            .into_response();
    }
    if req.method() != axum::http::Method::GET
        && req
            .headers()
            .get("x-ds-token")
            .and_then(|h| h.to_str().ok())
            != Some(&app.token)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"code":"session_expired","error":"页面会话已失效，请刷新后重试"})),
        )
            .into_response();
    }
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    h.insert("content-security-policy",HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'"));
    response
}

fn config_info(cfg: &AppConfig) -> Value {
    let mut value = json!(cfg);
    value["llm"].as_object_mut().unwrap().remove("api_key");
    value["search"].as_object_mut().unwrap().remove("api_key");
    value["has_search_key"] = json!(cfg.search.resolve_key().is_some());
    value["has_api_key"] = json!(cfg.resolve_api_key().is_some());
    let format = ds_engine::llm::providers::api_format(
        &cfg.llm.endpoint,
        &cfg.llm.model,
        cfg.llm.api_format,
    );
    value["api_format_resolved"] = json!(format);
    value["endpoint_normalized"] = json!(ds_engine::llm::providers::endpoint_for(
        &cfg.llm.endpoint,
        match format {
            ds_engine::llm::providers::ApiFormat::Anthropic => "/messages",
            ds_engine::llm::providers::ApiFormat::Responses => "/responses",
            _ => "/chat/completions",
        }
    )
    .unwrap_or_default());
    let default = AppConfig::default().scan_root;
    value["default_scan_root"] = json!(if cfg.scan_root.is_dir() {
        cfg.scan_root.clone()
    } else {
        default
    });
    value["scan_root_unavailable"] = json!(!cfg.scan_root.is_dir());
    value["default_desktop_root"] = json!(ds_engine::config::desktop_root());
    value["recycle_supported"] = json!(ds_engine::recycle::supported());
    value["permission_presets"] = json!(ds_engine::permission::category_presets());
    value
}
async fn bootstrap(State(app): State<Shared>) -> ApiResult {
    let config = app.config.lock().unwrap().clone();
    let job = app.active.lock().unwrap().as_ref().map(|a| a.job.clone());
    Ok(Json(
        json!({"config":config_info(&config),"token":app.token,"job":job,"last_job":app.last_job.lock().unwrap().clone(),"version":"0.2.0","fingerprint_policy":"blake3-adaptive-v2"}),
    ))
}

async fn pricing_preview(Json(cfg): Json<ds_engine::config::LlmConfig>) -> ApiResult {
    ds_engine::pricing::validate(&cfg.pricing)?;
    Ok(Json(ds_engine::pricing::preview(&cfg)))
}

async fn model_presets() -> Json<Value> {
    {
        let mut catalog = ds_engine::llm::providers::catalog().clone();
        catalog["pricing"] = json!(ds_engine::pricing::catalog());
        Json(catalog)
    }
}

#[derive(Deserialize)]
struct DiscoverModels {
    endpoint: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    api_format: ds_engine::llm::providers::ApiFormat,
    #[serde(default)]
    use_saved_key: bool,
}

async fn discover_models(
    State(app): State<Shared>,
    Json(request): Json<DiscoverModels>,
) -> ApiResult {
    let cfg = app.config.lock().unwrap().clone();
    let key = if !request.api_key.trim().is_empty() {
        request.api_key.trim().to_owned()
    } else if request.use_saved_key {
        if !ds_engine::llm::providers::same_connection(&request.endpoint, &cfg.llm.endpoint) {
            return Err(anyhow::anyhow!(
                "API 地址已改变，请填写该服务商的密钥；不会向新地址发送原密钥"
            )
            .into());
        }
        cfg.resolve_api_key().unwrap_or_default()
    } else {
        String::new()
    };
    Ok(Json(
        ds_engine::llm::discovery::discover(&request.endpoint, request.api_format, &key).await?,
    ))
}
async fn list_tasks(State(app): State<Shared>) -> ApiResult {
    let tasks=app.store.list()?.iter().map(|t|json!({"id":t.id,"root":t.root,"mode":t.mode,"phase":t.phase,"status":t.status,"updated_at":t.updated_at,"file_count":t.entries.iter().filter(|e|!e.is_dir()).count(),"operations":t.operations.len(),"usage":t.usage(),"cost_usd":ds_engine::pricing::totals(&t.calls).currencies.get("USD"),"costs":ds_engine::pricing::totals(&t.calls)})).collect::<Vec<_>>();
    Ok(Json(json!(tasks)))
}
async fn list_jobs(State(app): State<Shared>) -> ApiResult {
    let store = app.store.clone();
    let cfg = app.config.lock().unwrap().clone();
    Ok(Json(json!(
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Job>> {
            let mut jobs = store.list_jobs()?;
            let mut snapshots = std::collections::HashMap::new();
            for checkpoint in &mut jobs {
                if checkpoint.job.resumable && checkpoint.job.kind != "archive_import" {
                    let task = snapshots
                        .entry(checkpoint.job.task_id)
                        .or_insert_with(|| store.load(checkpoint.job.task_id));
                    let validation = match task {
                        Ok(task) => checkpoint.validate_resume(task, &cfg),
                        Err(e) => Err(anyhow::anyhow!("{e:#}")),
                    };
                    if let Err(e) = validation {
                        checkpoint.job.resumable = false;
                        checkpoint.job.recovery_note = e.to_string();
                    }
                }
            }
            Ok(jobs.into_iter().map(|c| c.job).collect())
        })
        .await??
    )))
}
async fn job_result(State(app): State<Shared>, Path(id): Path<Uuid>) -> ApiResult {
    let store = app.store.clone();
    Ok(Json(
        tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
            let checkpoint = store.load_job(id)?;
            ensure!(
                checkpoint.job.status == "completed",
                "归档尚未完成，请继续运行或等待保存完成"
            );
            match checkpoint.job.kind.as_str() {
                "archive_export" => {
                    let archive: ds_engine::archive::Archive = serde_json::from_value(
                        ds_engine::archive::read_json(&store.job_result_path(id))?,
                    )?;
                    archive.validate()?;
                    Ok(serde_json::to_value(archive)?)
                }
                "archive_import" => Ok(json!({"archive_id":id})),
                _ => anyhow::bail!("该任务没有归档结果"),
            }
        })
        .await??,
    ))
}
async fn get_task(State(app): State<Shared>, Path(id): Path<Uuid>) -> ApiResult {
    Ok(Json(task_view(&app.store.load(id)?)))
}
fn task_view(task: &Task) -> Value {
    let mut value = json!(task);
    if task.is_organizing() && matches!(task.phase, 2 | 3 | 4) {
        value["classification_readiness"] = json!(workflow_ai::classification_readiness(task));
    }
    value
}
async fn trajectory(State(app): State<Shared>, Path(id): Path<Uuid>) -> ApiResult {
    Ok(Json(json!(app.store.events(id)?)))
}
async fn export_archive(State(app): State<Shared>, Path(id): Path<Uuid>) -> ApiResult {
    let active = app.active.lock().unwrap();
    if active.is_some() {
        return Err(anyhow::anyhow!("请等待任务完成或停止后归档，以保持任务和轨迹一致").into());
    }
    Ok(Json(json!(app.store.export_archive(id)?)))
}
async fn list_archives(State(app): State<Shared>) -> ApiResult {
    Ok(Json(json!(app.store.list_archives()?)))
}
async fn get_archive(State(app): State<Shared>, Path(id): Path<Uuid>) -> ApiResult {
    Ok(Json(json!(app.store.load_archive(id)?)))
}
async fn events(
    State(app): State<Shared>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let stream = tokio_stream::wrappers::BroadcastStream::new(app.events.subscribe()).filter_map(
        |value| async move { value.ok().map(|v| Ok(Event::default().data(v.to_string()))) },
    );
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(10))
            .text("heartbeat"),
    )
}

#[derive(Deserialize)]
struct Action {
    action: String,
    #[serde(default)]
    task_id: Option<Uuid>,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(flatten)]
    args: Value,
}
fn str_arg<'a>(args: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    args[key]
        .as_str()
        .with_context(|| format!("缺少参数 {key}"))
}

async fn action(State(app): State<Shared>, Json(request): Json<Action>) -> ApiResult {
    dispatch(app, request).map_err(ApiError)
}
fn dispatch(app: Shared, request: Action) -> anyhow::Result<Json<Value>> {
    let Action {
        mut action,
        task_id,
        revision,
        mut args,
    } = request;
    if action == "cancel" {
        if let Some(active) = app.active.lock().unwrap().as_mut() {
            ensure!(
                args["job_id"].as_str() == Some(&active.job.id.to_string()),
                "取消请求不属于当前任务"
            );
            // Persist the pause request before acknowledging it. A crash after this
            // response must still present the interrupted run on next startup.
            active.job.status = "pausing".into();
            active.job.message = "正在保存并暂停，等待当前安全边界".into();
            active.checkpoint.job = active.job.clone();
            let saved = app.store.save_job(&mut active.checkpoint);
            active.cancel.cancel();
            saved?;
            let _ = app.events.send(json!({"type":"progress","job":active.job}));
        }
        return Ok(Json(json!({"ok":true})));
    }
    // All mutations take the same lock; two clients cannot execute or edit competing tasks.
    let mut active = app.active.lock().unwrap();
    ensure!(active.is_none(), "当前任务正在运行，请完成或停止后再操作");
    if action == "resume_job" {
        let mut checkpoint = app
            .store
            .load_job(Uuid::parse_str(str_arg(&args, "job_id")?)?)?;
        if checkpoint.job.kind == "archive_import" {
            ensure!(
                checkpoint.job.resumable
                    && matches!(
                        checkpoint.job.status.as_str(),
                        "paused" | "interrupted" | "failed"
                    ),
                "该归档导入不能继续"
            );
            checkpoint.job.status = "running".into();
            checkpoint.job.error = None;
            checkpoint.job.resumable = false;
            app.store.save_job(&mut checkpoint)?;
            let job = checkpoint.job.clone();
            let cancel = CancellationToken::new();
            *active = Some(Active {
                job: job.clone(),
                cancel: cancel.clone(),
                checkpoint,
                last_save: std::time::Instant::now(),
                persistence_error: None,
            });
            tokio::spawn(run_import(app.clone(), job.id, cancel));
            return Ok(Json(json!({"job":job})));
        }
        let task = app.store.load(checkpoint.job.task_id)?;
        let cfg = app.config.lock().unwrap().clone();
        checkpoint.validate_resume(&task, &cfg)?;
        checkpoint.job.status = "running".into();
        checkpoint.job.error = None;
        checkpoint.job.resumable = false;
        checkpoint.job.message = "正在校验并还原检查点".into();
        let kind = checkpoint.job.kind.clone();
        let mut parameters = checkpoint.args.clone();
        parameters["resume_checkpoint"] = json!(true);
        app.store.save_job(&mut checkpoint)?;
        let job = checkpoint.job.clone();
        let cancel = CancellationToken::new();
        *active = Some(Active {
            job: job.clone(),
            cancel: cancel.clone(),
            checkpoint,
            last_save: std::time::Instant::now(),
            persistence_error: None,
        });
        tokio::spawn(run_job(app.clone(), task, cfg, kind, parameters, cancel));
        return Ok(Json(json!({"job":job})));
    }
    if action == "archive_import" {
        ensure!(
            args["task"]["format"] == "downloadsweeper-archive",
            "请选择完整归档"
        );
        let cfg = app.config.lock().unwrap().clone();
        let mut checkpoint =
            Checkpoint::detached(Uuid::nil(), 0, "archive_import", json!({}), &cfg)?;
        // Persist the bounded input once. Heartbeats only rewrite the small control record.
        safe_fs::atomic_json(&app.store.job_input_path(checkpoint.job.id), &args["task"])?;
        app.store.save_job(&mut checkpoint)?;
        let job = checkpoint.job.clone();
        let cancel = CancellationToken::new();
        *active = Some(Active {
            job: job.clone(),
            cancel: cancel.clone(),
            checkpoint,
            last_save: std::time::Instant::now(),
            persistence_error: None,
        });
        tokio::spawn(run_import(app.clone(), job.id, cancel));
        return Ok(Json(json!({"job":job})));
    }
    if action == "config" {
        let previous = app.config.lock().unwrap().clone();
        let mut cfg: AppConfig = serde_json::from_value(args["config"].clone())?;
        cfg.local_env = previous.local_env.clone();
        cfg.search.local_env_key = previous.search.local_env_key.clone();
        let same_connection =
            ds_engine::llm::providers::same_connection(&cfg.llm.endpoint, &previous.llm.endpoint);
        let new_key = cfg.llm.api_key.trim().to_owned();
        if new_key.is_empty() {
            if same_connection && args["clear_key"] != true {
                cfg.llm.api_key = previous.llm.api_key.clone();
                cfg.llm.api_key_env = previous.llm.api_key_env.clone();
            } else {
                cfg.llm.api_key.clear();
                // Explicit missing variable prevents an unrelated DS_API_KEY fallback.
                cfg.llm.api_key_env = Some(format!("DS_MODEL_KEY_{}", Uuid::new_v4().simple()));
            }
        }
        if cfg.search.api_key.is_empty() && args["clear_search_key"] != true {
            cfg.search.api_key = previous.search.api_key;
        }
        let search_url = reqwest::Url::parse(&cfg.search.endpoint)?;
        ensure!(
            matches!(search_url.scheme(), "http" | "https"),
            "搜索 Endpoint 必须使用 http 或 https"
        );
        ensure!(!cfg.llm.model.trim().is_empty(), "模型名不能为空");
        ds_engine::ai_runtime::validate_config(&cfg)?;
        ensure!(
            cfg.llm.pricing.input_per_1k_usd.is_finite()
                && cfg.llm.pricing.output_per_1k_usd.is_finite()
                && cfg.llm.pricing.input_per_1k_usd >= 0.0
                && cfg.llm.pricing.output_per_1k_usd >= 0.0,
            "价格必须是非负数"
        );
        let _ = ds_engine::llm::providers::validate_url(&cfg.llm.endpoint)?;
        if !new_key.is_empty() {
            cfg.store_model_key(&app.config_path, &new_key)?;
        } else if !cfg.llm.api_key.is_empty() {
            // Migrate a legacy inline model key when its settings are next saved.
            let legacy = cfg.llm.api_key.clone();
            cfg.store_model_key(&app.config_path, &legacy)?;
        }
        cfg.save(&app.config_path)?;
        *app.config.lock().unwrap() = cfg.clone();
        return Ok(Json(config_info(&cfg)));
    }
    if action == "create" {
        let cfg = app.config.lock().unwrap().clone();
        let task = Task::new(
            PathBuf::from(str_arg(&args, "root")?),
            args["mode"].as_str().unwrap_or("organize"),
            cfg.permissions,
        )?;
        app.store.save(&task)?;
        app.store.event(
            &task,
            "task_created",
            json!({"mode":task.mode,"root":task.root}),
        )?;
        return Ok(Json(task_view(&task)));
    }
    if action == "import" {
        if args["task"]["format"] == "downloadsweeper-archive" {
            let id = app
                .store
                .import_archive(serde_json::from_value(args["task"].clone())?)?;
            return Ok(Json(json!({"archive_id":id})));
        }
        let task: Task = serde_json::from_value(args["task"].clone())?;
        let task = task.imported()?;
        app.store.save(&task)?;
        app.store.event(
            &task,
            "task_imported",
            json!({"note":"保留上下文与规则，执行前必须重新扫描并生成计划"}),
        )?;
        return Ok(Json(task_view(&task)));
    }
    if action == "resume_archive" {
        let id = Uuid::parse_str(str_arg(&args, "archive_id")?)?;
        let mut source = app.store.load_archive(id)?.task;
        if let Some(root) = args["root"].as_str().filter(|s| !s.trim().is_empty()) {
            source.root = PathBuf::from(root);
        }
        let task = source.imported()?;
        app.store.save(&task)?;
        app.store.event(
            &task,
            "archive_resumed",
            json!({"archive_id":id,"note":"原归档保持只读；新任务必须重新扫描"}),
        )?;
        return Ok(Json(task_view(&task)));
    }
    let id = task_id.context("请先创建或打开整理任务")?;
    let mut task = app.store.load(id)?;
    ensure!(
        revision == Some(task.revision),
        "任务已被更新，请刷新后重试"
    );
    if action == "advance" && task.phase == 2 && task.is_organizing() {
        task.advance()?;
        app.store.save(&task)?;
        app.store
            .event(&task, "advance", json!({"automatic_plan":"rules"}))?;
        action = "plan_rules".into();
    }
    if action == "proposal" && args["scene"] == "review" { action = "review_proposal".into(); }
    let job_kinds = [
        "review_proposal",
        "scan",
        "desktop_plan",
        "plan_rules",
        "plan_ai",
        "rename",
        "chat",
        "suggest_tree",
        "test_connection",
        "execute",
        "rollback",
        "cleanup_ai",
        "cleanup_options",
        "cleanup_trash",
        "cleanup_restore",
        "archive_export",
    ];
    if job_kinds.contains(&action.as_str()) {
        match action.as_str() {
            "review_proposal" => {
                task.editable()?;
                ensure!(task.is_organizing() && task.phase == 4 && task.status == "planned", "请在整理计划审查阶段合并建议");
            }
            "cleanup_trash" => {
                ensure!(
                    task.status == "completed" && args["confirmed"].as_bool() == Some(true),
                    "请完成整理并确认所选文件移入回收站"
                );
                let selected: Vec<String> = serde_json::from_value(args["selected"].clone())?;
                ensure!(
                    !selected.is_empty() && selected.len() <= 500,
                    "请选择要移入回收站的文件"
                );
            }
            "cleanup_restore" => {
                let batch = Uuid::parse_str(str_arg(&args, "batch")?)?;
                ensure!(
                    task.recycled.iter().any(|r| r.batch == batch),
                    "回收批次不属于本任务"
                );
            }
            "desktop_plan" => {
                task.editable()?;
                ensure!(
                    task.mode == "desktop" && task.scanned && task.phase < 5,
                    "请先扫描桌面目录"
                );
            }
            "execute" => ensure!(
                task.phase == 5 && task.reviewed && task.status == "planned",
                "请先审查确认计划"
            ),
            "plan_rules" | "plan_ai" => {
                task.editable()?;
                ensure!(
                    task.is_organizing() && matches!(task.phase, 3 | 4),
                    "请依次完成扫描、权限和目标树设置"
                );
                task.validate_graph(true)?;
                if action == "plan_ai" {
                    workflow_ai::classification_readiness(&task).ensure_ready()?;
                    let options: workflow_ai::RefineOptions = serde_json::from_value(args.clone())?;
                    ensure!(
                        (1..=1024).contains(&options.batch_size),
                        "每批文件数上限应在 1 到 1024 之间"
                    );
                }
            }
            "rename" => {
                task.editable()?;
                ensure!(
                    task.mode == "rename" && task.phase == 3,
                    "请完成扫描、权限和范围设置"
                );
            }
            "cleanup_ai" | "cleanup_options" => {
                ensure!(task.status == "completed", "请先完成整理");
                if action == "cleanup_options" || args.get("options").is_some() {
                    let options: ds_engine::cleanup::Options =
                        serde_json::from_value(args["options"].clone())?;
                    options.validate()?;
                    args["options"] = serde_json::to_value(options)?;
                }
            }
            "suggest_tree" => {
                task.editable()?;
                ensure!(
                    task.scanned && task.phase == 2 && task.is_organizing(),
                    "请在目标结构阶段进行分类型检查"
                );
            }
            "chat" => {
                let scene = str_arg(&args, "scene")?;
                ensure!(
                    ["home", "settings", "history"].contains(&scene)
                        || scene == task.scene()
                        || (scene == "directories" && task.phase == 2),
                    "AI 场景与当前任务阶段不一致"
                );
            }
            _ => {}
        }
        // Persist only parameters belonging to the operation. Never serialize arbitrary
        // client fields (in particular keys/config objects) into a checkpoint.
        let allowed: &[&str] = match action.as_str() {
            "cleanup_trash" => &["selected", "confirmed"],
            "cleanup_restore" => &["batch"],
            "plan_ai" => &["batch_size", "thinking"],
            "review_proposal" => &["proposal_id", "scene", "ids", "selected"],
            "chat" => &["scene", "message"],
            "suggest_tree" => &["message"],
            "cleanup_options" | "cleanup_ai" => &["options"],
            _ => &[],
        };
        args.as_object_mut()
            .context("操作参数必须是对象")?
            .retain(|key, _| allowed.contains(&key.as_str()));
        let cfg = app.config.lock().unwrap().clone();
        let mut checkpoint = Checkpoint::new(&task, &action, args.clone(), &cfg)?;
        if ds_engine::jobs::is_ai(&action) {
            let current_review = action == "review_proposal" && task.proposal.as_ref().is_some_and(|p| p.revision == task.revision);
            task.runtime_run = Some(checkpoint.job.id);
            task.touch();
            // Starting the approved transaction changes runtime state, not the reviewed plan.
            if current_review {
                if let Some(p) = task.proposal.as_mut() { p.revision = task.revision; }
            }
            checkpoint.revision = task.revision;
            app.store.save(&task)?;
        }
        app.store.save_job(&mut checkpoint)?;
        let job = checkpoint.job.clone();
        let cancel = CancellationToken::new();
        *active = Some(Active {
            job: job.clone(),
            cancel: cancel.clone(),
            checkpoint,
            last_save: std::time::Instant::now(),
            persistence_error: None,
        });
        let cloned = app.clone();
        let action_name = action.clone();
        tokio::spawn(async move {
            run_job(cloned, task, cfg, action_name, args, cancel).await;
        });
        return Ok(Json(json!({"job":job})));
    }
    match action.as_str() {
        "advance" => task.advance()?,
        "back" => {
            let phase = args["phase"].as_u64().context("缺少阶段")?;
            ensure!(phase < task.phase as u64, "只能返回之前的阶段");
            task.go_back(phase as u8)?;
        }
        "permissions" => {
            task.editable()?;
            ensure!(task.phase == 1, "请在读取权限阶段修改");
            task.permissions = serde_json::from_value(args["permissions"].clone())?;
            task.validate_permissions()?;
            task.invalidate(1);
        }
        "directory" => {
            ensure!(
                task.phase == 1 || task.phase == 2,
                "请在权限或目标结构阶段指定目录类型"
            );
            task.set_directory(
                str_arg(&args, "id")?,
                serde_json::from_value(args["class"].clone())?,
            )?;
        }
        "tree" => {
            task.editable()?;
            ensure!(task.phase == 2, "请在目标结构阶段编辑");
            let nodes: Vec<Node> = serde_json::from_value(args["nodes"].clone())?;
            task.nodes = nodes;
            task.validate_graph(false)?;
            task.invalidate(2);
        }
        "rename_scope" => {
            task.editable()?;
            ensure!(
                task.phase == 2 && task.mode == "rename",
                "请在命名范围阶段选择"
            );
            task.rename_extensions = serde_json::from_value(args["extensions"].clone())?;
            if let Some(value) = args["web_search"].as_bool() {
                task.rename_web_search = value;
            }
            task.invalidate(2);
        }
        "review" => {
            task.editable()?;
            ensure!(task.phase == 4, "请先生成计划");
            let selected: Vec<String> = serde_json::from_value(args["selected"].clone())?;
            for o in &mut task.operations {
                o.selected = selected.contains(&o.id);
            }
            task.reviewed = args["reviewed"] == true;
            task.touch();
        }
        "proposal" => {
            let ids: Vec<String> = serde_json::from_value(args["ids"].clone())?;
            task.apply_proposal(
                str_arg(&args, "proposal_id")?,
                &ids,
                str_arg(&args, "scene")?,
            )?;
        }
        "dismiss_proposal" => {
            task.proposal = None;
            task.touch();
        }
        _ => anyhow::bail!("未知操作"),
    }
    app.store.save(&task)?;
    app.store
        .event(&task, &action, json!({"phase":task.phase}))?;
    Ok(Json(task_view(&task)))
}

async fn run_import(app: Shared, id: Uuid, cancel: CancellationToken) {
    let heartbeat_app = app.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            report_progress(&heartbeat_app, None, None);
        }
    });
    let worker_app = app.clone();
    let worker_cancel = cancel.clone();
    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let store = &worker_app.store;
        let report = |current, total, message: &str| {
            report_progress(&worker_app, Some((current, total)), Some(message))
        };
        report(0, 0, "读取并校验归档，原任务保持不变");
        ensure!(!worker_cancel.is_cancelled(), "归档导入已暂停");
        let archive: ds_engine::archive::Archive =
            serde_json::from_value(ds_engine::archive::read_json(&store.job_input_path(id))?)?;
        archive.validate()?;
        ensure!(!worker_cancel.is_cancelled(), "归档导入已暂停");
        let path = store.directory.join("archives").join(format!("{id}.json"));
        if path.exists() {
            ensure!(
                store.load_archive(id)?.checksum == archive.checksum,
                "归档输出与输入不一致，已停止"
            );
        } else {
            safe_fs::atomic_json_with_cancel(&path, &archive, &worker_cancel, &report)?;
        }
        Ok(())
    })
    .await;
    heartbeat.abort();
    let error = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(format!("{e:#}")),
        Err(e) => Some(e.to_string()),
    };
    if let Some(mut active) = app.active.lock().unwrap().take() {
        active.job.status = if active.persistence_error.is_some() {
            "failed"
        } else if cancel.is_cancelled() {
            "paused"
        } else if error.is_none() {
            "completed"
        } else {
            "failed"
        }
        .into();
        active.job.error = active.persistence_error.or(error);
        active.job.resumable = active.job.status == "paused";
        active.job.recovery_note = "继续时重新校验输入；完整校验后才发布只读归档。".into();
        active.job.message = if active.job.status == "completed" {
            "归档已只读保存，可在历史页查看"
        } else {
            "归档导入尚未完成，输入已保存在本机"
        }
        .into();
        active.checkpoint.job = active.job;
        if let Err(e) = app.store.save_job(&mut active.checkpoint) {
            active.checkpoint.job.status = "failed".into();
            active.checkpoint.job.resumable = false;
            active.checkpoint.job.error = Some(format!("检查点保存失败：{e:#}"));
        }
        *app.last_job.lock().unwrap() = Some(active.checkpoint.job.clone());
        let _ = app
            .events
            .send(json!({"type":"finished","job":active.checkpoint.job}));
    }
}

fn report_progress(app: &Shared, current: Option<(usize, usize)>, message: Option<&str>) {
    if let Some(active) = app.active.lock().unwrap().as_mut() {
        if let Some((current, total)) = current {
            active.job.current = current;
            active.job.total = total;
        }
        if active.job.status != "pausing" {
            if let Some(message) = message {
                active.job.message = message.into();
            }
        }
        if active.last_save.elapsed() >= std::time::Duration::from_millis(750) {
            active.checkpoint.job = active.job.clone();
            match app.store.save_job(&mut active.checkpoint) {
                Ok(()) => active.job.saved_at = active.checkpoint.job.saved_at.clone(),
                Err(e) => {
                    active.persistence_error =
                        Some(format!("检查点保存失败，已停止后续工作：{e:#}"));
                    active.cancel.cancel();
                }
            }
            active.last_save = std::time::Instant::now();
        }
        let _ = app.events.send(json!({"type":"progress","job":active.job}));
    }
}

async fn run_job(
    app: Shared,
    mut task: Task,
    cfg: AppConfig,
    kind: String,
    mut args: Value,
    cancel: CancellationToken,
) {
    let resume = args
        .as_object_mut()
        .and_then(|m| m.remove("resume_checkpoint"))
        == Some(json!(true));
    // Progress remains visible even while a blocking OS/serialization call is pending.
    let heartbeat_app = app.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            report_progress(&heartbeat_app, None, None);
        }
    });
    let notify_app = app.clone();
    let progress = move |current: usize, total: usize, message: &str| {
        report_progress(
            &notify_app,
            if total > 0 {
                Some((current, total))
            } else {
                None
            },
            Some(message),
        );
    };
    let store = app.store.clone();
    let outcome: anyhow::Result<Task> = std::panic::AssertUnwindSafe(async {
        if matches!(
            kind.as_str(),
            "scan"
                | "desktop_plan"
                | "plan_rules"
                | "execute"
                | "rollback"
                | "cleanup_options"
                | "cleanup_trash"
                | "cleanup_restore"
                | "archive_export"
        ) {
            let k = kind.clone();
            let c = cancel.clone();
            let papp = app.clone();
            let local_args = args.clone();
            return tokio::task::spawn_blocking(move || -> anyhow::Result<Task> {
                let report = |current, total, message: &str| {
                    report_progress(&papp, Some((current, total)), Some(message));
                };
                if resume && matches!(k.as_str(), "execute" | "rollback") {
                    safe_fs::recover(&mut task, &store)?;
                }
                let result = match k.as_str() {
                    "scan" => task.scan(&c, &report),
                    "desktop_plan" => task.prepare_desktop(&c, &report),
                    "cleanup_trash" => ds_engine::recycle::recycle(
                        &mut task,
                        &store,
                        &serde_json::from_value::<Vec<String>>(local_args["selected"].clone())?,
                        &c,
                        &report,
                    ),
                    "cleanup_restore" => ds_engine::recycle::undo(
                        &mut task,
                        &store,
                        Uuid::parse_str(local_args["batch"].as_str().context("缺少回收批次")?)?,
                        &c,
                        &report,
                    ),
                    "plan_rules" => task.generate_rules(&c, &report),
                    "execute" if resume => safe_fs::resume_execute(&mut task, &store, &c, &report),
                    "execute" => safe_fs::execute(&mut task, &store, &c, &report),
                    "cleanup_options" => {
                        let mut draft = task.clone();
                        draft.cleanup_options =
                            serde_json::from_value(local_args["options"].clone())?;
                        ds_engine::cleanup::prepare_with_cancel(
                            &mut draft,
                            chrono::Utc::now().timestamp_millis().max(0) as u64,
                            &c,
                            &report,
                        )?;
                        draft.touch();
                        task = draft;
                        Ok(())
                    }
                    "archive_export" => {
                        let job_id = papp
                            .active
                            .lock()
                            .unwrap()
                            .as_ref()
                            .context("运行记录不存在")?
                            .job
                            .id;
                        let path = store.job_result_path(job_id);
                        // An atomic result left by a crash is already the frozen export.
                        if !path.exists() {
                            let archive = store.export_archive_with_cancel(task.id, &c, &report)?;
                            safe_fs::atomic_json_with_cancel(&path, &archive, &c, &report)?;
                        } else {
                            let archive: ds_engine::archive::Archive =
                                serde_json::from_value(ds_engine::archive::read_json(&path)?)?;
                            archive.validate()?;
                        }
                        Ok(())
                    }
                    _ => safe_fs::rollback(&mut task, &store, &c, &report),
                };
                store.save(&task)?;
                result?;
                Ok(task)
            })
            .await?;
        }
        let result = match kind.as_str() {
            "review_proposal" => workflow_ai::apply_review_proposal(
                &mut task,&cfg,&store,str_arg(&args,"proposal_id")?,
                &serde_json::from_value::<Vec<String>>(args["ids"].clone())?,
                args.get("selected").filter(|v|!v.is_null()).map(|v|serde_json::from_value(v.clone())).transpose()?,
                &cancel,&progress,&|state| {
                    if let Some(active)=app.active.lock().unwrap().as_mut() {active.job.parallel=Some(state);}
                    report_progress(&app,None,None);
                },
            ).await,

            "plan_ai" => {
                let options = serde_json::from_value(args.clone())?;
                workflow_ai::refine_with_parallel_progress(
                    &mut task, &cfg, &store, options, &cancel, &progress,
                    &|state| {
                        if let Some(active) = app.active.lock().unwrap().as_mut() { active.job.parallel = Some(state); }
                        report_progress(&app, None, None);
                    },
                )
                .await
            }
            "rename" => workflow_ai::rename(&mut task, &cfg, &store, &cancel, &progress).await,
            "cleanup_ai" => {
                if !resume {
                    let mut draft = task.clone();
                    if let Some(options) = args.get("options") {
                        draft.cleanup_options = serde_json::from_value(options.clone())?;
                    }
                    let c = cancel.clone();
                    let papp = app.clone();
                    task = tokio::task::spawn_blocking(move || -> anyhow::Result<Task> {
                        ds_engine::cleanup::prepare_with_cancel(
                            &mut draft,
                            chrono::Utc::now().timestamp_millis().max(0) as u64,
                            &c,
                            &|current, total, message| {
                                report_progress(&papp, Some((current, total)), Some(message))
                            },
                        )?;
                        draft.touch();
                        Ok(draft)
                    })
                    .await??;
                    store.save(&task)?;
                }
                workflow_ai::cleanup_review_resume(
                    &mut task, &cfg, &store, &cancel, &progress, true,
                )
                .await
            }
            "chat" => {
                workflow_ai::chat(
                    &mut task,
                    &cfg,
                    &store,
                    str_arg(&args, "scene")?,
                    str_arg(&args, "message")?,
                    &cancel,
                    &progress,
                )
                .await
            }
            "suggest_tree" => {
                workflow_ai::suggest_tree(
                    &mut task,
                    &cfg,
                    &store,
                    str_arg(&args, "message")?,
                    &cancel,
                    &progress,
                )
                .await
            }
            "test_connection" => {
                workflow_ai::test_connection(&mut task, &cfg, &store, &cancel, &progress)
                    .await
                    .map(|_| ())
            }
            _ => unreachable!(),
        };
        store.save(&task)?;
        result?;
        Ok(task)
    })
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "运行发生内部异常；已停止，请从持久化状态检查恢复"
        ))
    });
    heartbeat.abort();
    let mut active = app.active.lock().unwrap();
    if let Some(mut a) = active.take() {
        let persistence_failed = a.persistence_error.is_some();
        let partial = outcome
            .as_ref()
            .ok()
            .filter(|t| matches!(kind.as_str(), "execute" | "rollback") && t.status == "partial")
            .map(|t| {
                t.operations
                    .iter()
                    .find_map(|o| o.error.clone())
                    .unwrap_or_else(|| "操作仅部分完成，可查看逐项记录并恢复已完成的移动".into())
            });
        a.job.status = if a.persistence_error.is_some() {
            "failed"
        } else if cancel.is_cancelled() {
            "paused"
        } else if outcome.is_ok() && partial.is_none() {
            "completed"
        } else {
            "failed"
        }
        .into();
        a.job.error = a
            .persistence_error
            .or_else(|| outcome.err().map(|e| format!("{e:#}")))
            .or(partial);
        a.job.message = match a.job.status.as_str() {
            "completed" if kind == "chat" || kind == "suggest_tree" => {
                if let Ok(task) = app.store.load(a.job.task_id) {
                    if let Some(proposal) = task.proposal.as_ref().filter(|p| {
                        Some(p.scene.as_str()) == args["scene"].as_str()
                            || kind == "suggest_tree" && p.scene == "tree"
                    }) {
                        format!(
                            "AI 已生成 {} 项建议，审查合并后生效",
                            proposal.changes.len()
                        )
                    } else {
                        "AI 已回复，本次没有生成可合并改动".into()
                    }
                } else {
                    "AI 已回复".into()
                }
            }
            "completed" if kind == "plan_rules" => {
                "基础规则计划已生成，可继续 AI 细化或进入审查".into()
            }
            "completed" if kind == "review_proposal" => "具体位置规划已完成，整理后预览已更新".into(),
            "completed" if kind == "plan_ai" => "AI 细化已完成，请审查更新后的计划".into(),
            "completed" if kind == "archive_export" => "完整归档已保存，可下载".into(),
            "completed" => "任务完成".into(),
            "paused" => "任务已暂停；已保存任务状态、完成的操作与用量".into(),
            _ => "任务未完成，请查看原因".into(),
        };
        a.checkpoint.job = a.job;
        if let Ok(task) = app.store.load(a.checkpoint.job.task_id) {
            if a.checkpoint.job.status != "completed" {
                a.checkpoint.stopped(&task);
            }
            if persistence_failed {
                a.checkpoint.job.resumable = false;
            }
            if let Err(e) = app
                .store
                .event(&task, "job_finished", json!({"job":a.checkpoint.job}))
            {
                a.checkpoint.job.error = Some(format!("任务轨迹保存失败：{e:#}"));
                a.checkpoint.job.status = "failed".into();
                a.checkpoint.job.resumable = false;
            }
        }
        if let Err(e) = app.store.save_job(&mut a.checkpoint) {
            a.checkpoint.job.status = "failed".into();
            a.checkpoint.job.resumable = false;
            a.checkpoint.job.error = Some(format!(
                "最终检查点保存失败，请检查磁盘后重启以核对状态：{e:#}"
            ));
        }
        *app.last_job.lock().unwrap() = Some(a.checkpoint.job.clone());
        let _ = app
            .events
            .send(json!({"type":"finished","job":a.checkpoint.job}));
    }
}

async fn index() -> Response {
    #[cfg(debug_assertions)]
    if let Ok(html) = tokio::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../frontend/index.html"),
    )
    .await
    {
        return ([("content-type", "text/html; charset=utf-8")], html).into_response();
    }
    (
        [("content-type", "text/html; charset=utf-8")],
        include_str!("../../../frontend/index.html"),
    )
        .into_response()
}
async fn asset(Path(path): Path<String>) -> Response {
    let data: Option<(&str, &[u8])> = match path.as_str() {
        "app.js" => Some((
            "text/javascript",
            include_bytes!("../../../frontend/app.js"),
        )),
        "graph.js" => Some((
            "text/javascript",
            include_bytes!("../../../frontend/graph.js"),
        )),
        "parallel_progress.js" => Some(("text/javascript; charset=utf-8", include_bytes!("../../../frontend/parallel_progress.js"))),
        "proposal_selection.js" => Some(("text/javascript; charset=utf-8", include_bytes!("../../../frontend/proposal_selection.js"))),
        "permissions.js" => Some((
            "text/javascript",
            include_bytes!("../../../frontend/permissions.js"),
        )),
        "styles.css" => Some(("text/css", include_bytes!("../../../frontend/styles.css"))),
        "vendor/react.production.min.js" => Some((
            "text/javascript",
            include_bytes!("../../../frontend/vendor/react.production.min.js"),
        )),
        "vendor/react-dom.production.min.js" => Some((
            "text/javascript",
            include_bytes!("../../../frontend/vendor/react-dom.production.min.js"),
        )),
        "vendor/reactflow.umd.min.js" => Some((
            "text/javascript",
            include_bytes!("../../../frontend/vendor/reactflow.umd.min.js"),
        )),
        "vendor/reactflow.style.css" => Some((
            "text/css",
            include_bytes!("../../../frontend/vendor/reactflow.style.css"),
        )),
        _ => None,
    };
    match data {
        Some((mime, bytes)) => {
            #[cfg(debug_assertions)]
            if let Ok(content) = tokio::fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../frontend")
                    .join(&path),
            )
            .await
            {
                return ([("content-type", mime)], content).into_response();
            }
            ([("content-type", mime)], bytes).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

pub use ds_engine::config::runtime_paths as default_runtime_paths;
/// Dispatch isolated media workers before the desktop runtime loads settings.
pub use ds_engine::evidence::dispatch_helper as dispatch_media_helper;
