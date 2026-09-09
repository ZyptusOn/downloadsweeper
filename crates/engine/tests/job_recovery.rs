use ds_engine::{
    ai_runtime,
    config::AppConfig,
    cost::Usage,
    jobs::Checkpoint,
    llm::Message,
    permission::PermissionConfig,
    response_cache,
    safe_fs::{self, TaskStore},
    workflow::{CallRecord, Task},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[test]
fn retired_pipeline_removal_preserves_existing_configuration_digests() {
    for examples in [
        json!([]),
        json!([{"filename":"sample.txt","category":"文档","path":"sample.txt"}]),
    ] {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value["scan_root"] = json!("synthetic-root");
        value["few_shot"] = examples;
        let config: AppConfig = serde_json::from_value(value).unwrap();
        let wire = serde_json::to_value(&config).unwrap();
        // This is the pre-cleanup AppConfig wire order, used by saved job digests.
        let legacy_wire = format!(
            "{{\"llm\":{},\"search\":{},\"scan_root\":{},\"permissions\":{},\"few_shot\":{},\"token_budget\":{},\"max_iterations\":{}}}",
            serde_json::to_string(&config.llm).unwrap(),
            serde_json::to_string(&config.search).unwrap(),
            serde_json::to_string(&config.scan_root).unwrap(),
            serde_json::to_string(&config.permissions).unwrap(),
            // Rebuild the original example field order explicitly as well.
            if wire["few_shot"].as_array().unwrap().is_empty() { "[]" } else {
                "[{\"filename\":\"sample.txt\",\"category\":\"文档\",\"path\":\"sample.txt\"}]"
            },
            serde_json::to_string(&config.token_budget).unwrap(), config.max_iterations,
        );
        let digest = blake3::hash(legacy_wire.as_bytes()).to_hex().to_string();
        assert_eq!(ds_engine::jobs::config_digest(&config).unwrap(), digest);
        let reloaded = AppConfig::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(ds_engine::jobs::config_digest(&reloaded).unwrap(), digest);
    }
}

fn fixture() -> (Task, TaskStore) {
    let base = std::env::temp_dir().join(format!("ds-checkpoint-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(base.join("files")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(base.join("files").join(name), name).unwrap();
    }
    let task = Task::new(base.join("files"), "organize", PermissionConfig::default()).unwrap();
    let store = TaskStore::new(base.join("state")).unwrap();
    store.save(&task).unwrap();
    (task, store)
}

#[test]
fn restart_recovers_job_but_rejects_stale_or_damaged_checkpoints() {
    let (mut task, store) = fixture();
    let cfg = AppConfig::default();
    let mut checkpoint = Checkpoint::new(&task, "scan", json!({}), &cfg).unwrap();
    checkpoint.job.current = 120;
    checkpoint.job.status = "pausing".into();
    store.save_job(&mut checkpoint).unwrap();
    let recovered = store.recover_jobs().unwrap().remove(0);
    assert_eq!(recovered.job.status, "interrupted");
    assert_eq!(recovered.job.current, 120);
    recovered.validate_resume(&task, &cfg).unwrap();
    task.touch();
    assert!(recovered.validate_resume(&task, &cfg).is_err());
    std::fs::write(store.job_path(checkpoint.job.id), b"damaged").unwrap();
    assert!(store.load_job(checkpoint.job.id).is_err());
    let damaged = store.recover_jobs().unwrap().remove(0);
    assert!(!damaged.job.resumable);
    assert_eq!(damaged.job.kind, "damaged");
    assert_eq!(
        std::fs::read(store.job_path(checkpoint.job.id)).unwrap(),
        b"damaged"
    );
}

#[test]
fn paused_execution_continues_without_duplicate_moves_and_detects_changes() {
    let (mut task, store) = fixture();
    let cancel = CancellationToken::new();
    task.scan(&cancel, &|_, _, _| {}).unwrap();
    for _ in 0..3 {
        task.advance().unwrap();
    }
    task.generate_rules(&cancel, &|_, _, _| {}).unwrap();
    task.advance().unwrap();
    task.reviewed = true;
    task.advance().unwrap();
    safe_fs::execute(&mut task, &store, &cancel, &|done, _, message| {
        if done == 1 && message == "文件移动已记录，可随时停止" {
            cancel.cancel();
        }
    })
    .unwrap();
    assert_eq!(
        task.operations
            .iter()
            .filter(|o| o.status == "done")
            .count(),
        1
    );
    assert_eq!(task.status, "partial");
    let mut restored = store.load(task.id).unwrap();
    safe_fs::recover(&mut restored, &store).unwrap();
    safe_fs::resume_execute(
        &mut restored,
        &store,
        &CancellationToken::new(),
        &|_, _, _| {},
    )
    .unwrap();
    assert_eq!(restored.status, "completed");
    assert_eq!(
        store
            .events(task.id)
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "move_intent")
            .count(),
        3
    );
    for operation in &restored.operations {
        assert_eq!(
            std::fs::read_to_string(task.root.join(&operation.destination)).unwrap(),
            operation.source
        );
    }
    let destination = task.root.join(&restored.operations[0].destination);
    std::fs::write(&destination, "changed").unwrap();
    assert!(safe_fs::resume_execute(
        &mut restored,
        &store,
        &CancellationToken::new(),
        &|_, _, _| {}
    )
    .is_err());
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "changed");
}

#[test]
fn cancelling_atomic_output_never_publishes_partial_archive() {
    let (_, store) = fixture();
    let path = store.directory.join("export.json");
    safe_fs::atomic_json(&path, &json!({"original":true})).unwrap();
    let before = std::fs::read(&path).unwrap();
    let cancel = CancellationToken::new();
    let value = json!({"body":"x".repeat(1024*1024)});
    assert!(
        safe_fs::atomic_json_with_cancel(&path, &value, &cancel, &|_, _, _| cancel.cancel())
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!std::fs::read_dir(&store.directory).unwrap().any(|e| e
        .unwrap()
        .path()
        .extension()
        .is_some_and(|s| s == "tmp")));
}

#[tokio::test]
async fn saved_response_reconciles_usage_and_replays_without_network_or_double_billing() {
    let (mut task, store) = fixture();
    let mut cfg = AppConfig::default();
    cfg.llm.endpoint = "http://127.0.0.1:9/v1".into();
    cfg.llm.api_key.clear();
    cfg.llm.api_key_env = Some("DS_CHECKPOINT_TEST_MISSING".into());
    cfg.llm.model = "checkpoint-fixture".into();
    task.runtime_run = Some(uuid::Uuid::new_v4());
    let messages = vec![Message::user("fixture request")];
    let purpose = "fixture";
    let key = blake3::hash(
        &serde_json::to_vec(
            &json!({"config":ds_engine::jobs::config_digest(&cfg).unwrap(),
        "purpose":purpose,"messages":messages,"tools":[],"thinking":false}),
        )
        .unwrap(),
    )
    .to_hex()
    .to_string();
    let record = CallRecord {
        id: "acknowledged-response".into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        purpose: purpose.into(),
        model: cfg.llm.model.clone(),
        usage: Usage {
            prompt_tokens: 100,
            completion_tokens: 20,
            ..Usage::default()
        },
        cost_usd: None,
        billing: None,
        finish_reason: Some("stop".into()),
        max_output_tokens: 200,
    };
    task.pending_calls
        .push(json!({"id":record.id,"reserved_tokens":1000}));
    store.save(&task).unwrap();
    let response = response_cache::Response {
        request_key: key.clone(),
        record,
        message: Message::assistant("durable answer", vec![]),
    };
    response_cache::save(
        &response_cache::directory(&store, &task)
            .unwrap()
            .join(format!("{key}.json")),
        &response,
    )
    .unwrap();
    // Simulate death after the response envelope commits, before task.json does.
    task = store.load(task.id).unwrap();
    response_cache::recover(&store, &mut task).unwrap();
    response_cache::recover(&store, &mut task).unwrap();
    assert!(task.pending_calls.is_empty());
    assert_eq!(task.usage().total(), 120);
    let replay = ai_runtime::run(
        &mut task,
        &cfg,
        &store,
        &CancellationToken::new(),
        |_, _, _| Ok(()),
        |runtime, _| async move {
            runtime
                .call(purpose, &messages, &[], false, &|_, _, _| {})
                .await
        },
    )
    .await
    .unwrap();
    assert_eq!(replay.content, "durable answer");
    assert_eq!(task.calls.len(), 1);
    assert_eq!(task.usage().total(), 120);
    assert!(store
        .events(task.id)
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "llm_response_replayed"));
    assert!(task.clone().imported().unwrap().runtime_run.is_none());
    let mut checkpoint = Checkpoint::new(&task, "plan_ai", json!({}), &cfg).unwrap();
    task.pending_calls
        .push(json!({"id":"unknown-response","reserved_tokens":1000}));
    checkpoint.job.status = "paused".into();
    checkpoint.stopped(&task);
    assert!(!checkpoint.job.resumable);
    assert!(checkpoint.validate_resume(&task, &cfg).is_err());
}
