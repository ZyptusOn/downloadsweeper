//! One request boundary for all workflow agents. Workers never write TaskStore.
//! Reservations are durably acknowledged before HTTP; only the coordinator saves state.
use crate::{
    config::AppConfig,
    llm::{ChatOptions, LlmClient, Message, OpenAiClient, ToolDef},
    safe_fs::TaskStore,
    workflow::{CallRecord, Progress, Task},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, oneshot, Notify, Semaphore};
use tokio_util::sync::CancellationToken;

pub fn request_text_bytes(messages: &[Message], tools: &[ToolDef]) -> usize {
    serde_json::to_vec(tools).map_or(0, |v| v.len())
        + messages
            .iter()
            .map(|m| {
                m.content.len()
                    + m.reasoning_content.as_ref().map_or(0, String::len)
                    + m.provider_state
                        .as_ref()
                        .and_then(|s| serde_json::to_vec(s).ok())
                        .map_or(0, |v| v.len())
                    + serde_json::to_vec(&m.tool_calls).map_or(0, |v| v.len())
                    + 96
            })
            .sum::<usize>()
}

/// Still conservative (bytes, not a guessed tokenizer); scales with configured context.
pub fn initial_text_limit(cfg: &AppConfig) -> usize {
    (cfg.llm
        .context_length
        .saturating_sub(cfg.llm.max_output_tokens.min(cfg.llm.context_length / 3))
        / 3)
    .clamp(1024, 1024 * 1024) as usize
}
pub fn conversation_text_limit(cfg: &AppConfig) -> usize {
    (cfg.llm.context_length.saturating_sub(512) * 3 / 4).clamp(1024, 2 * 1024 * 1024) as usize
}
pub fn validate_config(cfg: &AppConfig) -> Result<()> {
    crate::pricing::validate(&cfg.llm.pricing)?;
    ensure!(
        (1..=8).contains(&cfg.llm.parallel_requests),
        "并行请求数应在 1 到 8 之间"
    );
    ensure!(
        (1..=32).contains(&cfg.max_iterations),
        "工具轮次上限应在 1 到 32 之间"
    );
    ensure!(
        (512..=4_000_000).contains(&cfg.llm.context_length),
        "上下文长度应在 512 到 4000000 之间"
    );
    ensure!(
        (64..=4_000_000).contains(&cfg.llm.max_output_tokens),
        "单次输出上限应在 64 到 4000000 之间"
    );
    ensure!(
        (1..=3600).contains(&cfg.llm.request_timeout_seconds),
        "请求超时应在 1 到 3600 秒之间"
    );
    Ok(())
}

/// Shared protocol gate, separate from each tool's stricter domain argument validation.
pub fn validate_tool_turn(calls: &[crate::llm::ToolCall], definitions: &[ToolDef]) -> Result<()> {
    let ids: std::collections::HashSet<_> = calls.iter().map(|call| &call.id).collect();
    ensure!(
        !calls.is_empty()
            && calls.len() <= 8
            && ids.len() == calls.len()
            && calls
                .iter()
                .all(|call| !call.id.is_empty() && call.id.len() <= 256),
        "工具调用数量或 ID 无效"
    );
    for call in calls {
        ensure!(
            definitions.iter().any(|tool| tool.name == call.name),
            "工具未授权：本场景只允许明确提供的工具"
        );
        ensure!(
            call.args.is_object() && serde_json::to_vec(&call.args)?.len() <= 1024 * 1024,
            "工具参数必须是有界 JSON 对象"
        );
    }
    Ok(())
}

type Ack = oneshot::Sender<std::result::Result<(), String>>;
enum Event {
    Request(Value, Ack),
    Response(String, CallRecord, Value, Message, String, Ack),
    Domain(String, Value, Ack),
}
#[derive(Default)]
struct Ledger {
    used: u64,
    reserved: u64,
}
#[derive(Clone)]
pub struct Runtime {
    config: Arc<AppConfig>,
    client: Arc<dyn LlmClient>,
    cancel: CancellationToken,
    events: mpsc::UnboundedSender<Event>,
    ledger: Arc<Mutex<Ledger>>,
    changed: Arc<Notify>,
    slots: Arc<Semaphore>,
    stopped: Arc<AtomicBool>,
    cache_directory: Option<std::path::PathBuf>,
}
impl Runtime {
    pub fn stop_queued(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.changed.notify_waiters();
    }
    pub fn ready(&self) -> Result<()> {
        ensure!(!self.cancel.is_cancelled(), "任务已取消；已完成结果保留");
        ensure!(
            !self.stopped.load(Ordering::SeqCst),
            "前序请求失败，尚未发送的请求已停止"
        );
        ensure!(
            !self
                .config
                .token_budget
                .is_some_and(|limit| self.ledger.lock().unwrap().used >= limit),
            "已达到 token 预算（包括未确认请求的预留）"
        );
        Ok(())
    }
    pub async fn event(&self, kind: &str, value: Value) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.events.send(Event::Domain(kind.into(), value, tx))?;
        rx.await?.map_err(anyhow::Error::msg)
    }
    pub async fn call(
        &self,
        purpose: &str,
        messages: &[Message],
        tools: &[ToolDef],
        thinking: bool,
        progress: &Progress<'_>,
    ) -> Result<Message> {
        ensure!(!self.cancel.is_cancelled(), "任务已暂停");
        let request_key = blake3::hash(&serde_json::to_vec(
            &json!({"config":crate::jobs::config_digest(&self.config)?,
            "purpose":purpose,"messages":messages,"tools":tools,"thinking":thinking}),
        )?)
        .to_hex()
        .to_string();
        if let Some(path) = self
            .cache_directory
            .as_ref()
            .map(|d| d.join(format!("{request_key}.json")))
            .filter(|p| p.is_file())
        {
            let response = crate::response_cache::load(&path)?;
            ensure!(
                response.request_key == request_key,
                "模型回答检查点与请求不匹配"
            );
            ensure!(
                response.record.finish_reason.as_deref() != Some("length"),
                "已保存的模型回答被截断；不会自动重发付费请求，请调整参数后重新启动"
            );
            ensure!(
                !response.message.content.trim().is_empty()
                    || !response.message.tool_calls.is_empty(),
                "已保存的模型回答为空，不会自动重发"
            );
            self.event(
                "llm_response_replayed",
                json!({"id":response.record.id,"purpose":purpose,"network_request":false}),
            )
            .await?;
            progress(0, 0, "还原已保存的模型回答；不重复调用 API");
            return Ok(response.message);
        }
        self.ready()?;
        let _permit = tokio::select! {
            _ = self.cancel.cancelled() => anyhow::bail!("任务已取消"),
            p = self.slots.acquire() => p?,
        };
        self.ready()?;
        let cfg = &self.config;
        let text_bytes = request_text_bytes(messages, tools);
        let image_count = messages.iter().map(|m| m.images.len()).sum::<usize>();
        ensure!(
            image_count <= 8
                && messages
                    .iter()
                    .flat_map(|m| &m.images)
                    .all(|i| i.data_base64.len() <= 512 * 1024 && i.mime == "image/jpeg"),
            "图像证据必须是有界 JPEG 预览，每次最多 8 张"
        );
        ensure!(
            image_count == 0
                || (cfg.llm.multimodal
                    && !crate::llm::providers::preset(&cfg.llm.model)
                        .is_some_and(|p| p["vision"] == false)),
            "当前模型或设置不允许图像输入"
        );
        let estimate = text_bytes as u64 + messages.iter().flat_map(|m| &m.images)
            .map(|image| if image.high_detail { 4096 } else { 2048 }).sum::<u64>();
        let limits = crate::llm::providers::preset(&cfg.llm.model).unwrap_or(Value::Null);
        ensure!(
            estimate + 64 < limits["max_input_tokens"].as_u64().unwrap_or(u64::MAX),
            "超过模型最大输入限制，请缩小范围"
        );
        ensure!(
            estimate + 64 < cfg.llm.context_length,
            "当前内容超过配置的上下文长度，请提高长度或缩小范围"
        );
        let context_max = cfg.llm.context_length.saturating_sub(estimate);
        let provider_max = limits["max_output_tokens"].as_u64().unwrap_or(u64::MAX);
        // If concurrent reservations occupy the budget, wait instead of overspending or failing immediately.
        let (max_tokens, available) = loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            self.ready()?;
            let decision = {
                let mut budget = self.ledger.lock().unwrap();
                let available = cfg
                    .token_budget
                    .unwrap_or(u64::MAX)
                    .saturating_sub(budget.used)
                    .saturating_sub(budget.reserved);
                if available > estimate + 64 {
                    let max = cfg
                        .llm
                        .max_output_tokens
                        .min(context_max)
                        .min(provider_max)
                        .min(available - estimate);
                    budget.reserved = budget.reserved.saturating_add(estimate + max);
                    Some((max, available))
                } else {
                    ensure!(
                        budget.reserved > 0,
                        "剩余 token 预算不足（包括未确认请求的预留），本次未发送；请调整任务预算"
                    );
                    None
                }
            };
            if let Some(value) = decision {
                break value;
            }
            tokio::select! { _ = self.cancel.cancelled() => anyhow::bail!("任务已取消"), _ = &mut notified => {} }
        };
        let reserved = estimate + max_tokens;
        let id = uuid::Uuid::new_v4().to_string();
        let request_at = chrono::Utc::now();
        let mut reasons = vec![];
        if max_tokens < cfg.llm.max_output_tokens {
            if max_tokens == context_max {
                reasons.push("上下文剩余空间");
            }
            if max_tokens == provider_max {
                reasons.push("模型官方输出上限");
            }
            if max_tokens == available - estimate {
                reasons.push("剩余任务 token 预算（含并行预留）");
            }
        }
        let detail = json!({"request_at":request_at.to_rfc3339(),"id":id,"purpose":purpose,"model":cfg.llm.model,"reserved_tokens":reserved,
            "message_count":messages.len(),"max_output_tokens":max_tokens,"configured_max_output_tokens":cfg.llm.max_output_tokens,
            "output_limit_reasons":reasons,"estimated_input_tokens":estimate,"remaining_task_budget":cfg.token_budget.map(|_|available),
            "input_text_bytes":text_bytes,"image_count":image_count,"thinking":thinking,"tools":tools.iter().map(|t|&t.name).collect::<Vec<_>>()});
        let (tx, rx) = oneshot::channel();
        self.events.send(Event::Request(detail, tx))?;
        rx.await?.map_err(anyhow::Error::msg)?;
        let options = ChatOptions {
            model: cfg.llm.model.clone(),
            temperature: cfg.llm.temperature,
            max_tokens: Some(max_tokens),
            thinking_mode: thinking,
        };
        let request = self.client.chat(messages, tools, &options, &self.cancel);
        tokio::pin!(request);
        let started = std::time::Instant::now();
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(1));
        let response = loop {
            tokio::select! {
                result = &mut request => break result,
                _ = heartbeat.tick() => progress(0, 0, &format!("{purpose}：等待模型回答 {} 秒 / {} 秒",started.elapsed().as_secs(),cfg.llm.request_timeout_seconds)),
            }
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let rejected = error
                    .downcast_ref::<crate::llm::client::RejectedRequest>()
                    .is_some();
                // An interrupted HTTP request may have been billed. Keep its durable reservation.
                {
                    let mut budget = self.ledger.lock().unwrap();
                    budget.reserved = budget.reserved.saturating_sub(reserved);
                    if !rejected {
                        budget.used = budget.used.saturating_add(reserved);
                    }
                }
                self.stop_queued();
                if rejected {
                    self.event(
                        "llm_rejected",
                        json!({"id":id,"purpose":purpose,"error":error.to_string()}),
                    )
                    .await?;
                    return Err(error);
                }
                self.event("llm_unconfirmed", json!({"id":id,"purpose":purpose,"reserved_tokens":reserved,"error":error.to_string()})).await?;
                return Err(error)
                    .map_err(|e| e.context("未收到完整 usage，预留额度暂未释放；没有自动重试"));
            }
        };
        let usage = response.usage.clone();
        let exceeded = {
            let mut budget = self.ledger.lock().unwrap();
            budget.reserved = budget.reserved.saturating_sub(reserved);
            budget.used = budget.used.saturating_add(usage.total());
            cfg.token_budget.is_some_and(|limit| budget.used >= limit)
        };
        self.changed.notify_waiters();
        let billing =
            crate::pricing::calculate(&cfg.llm, &usage, request_at, &response.billing_metadata);
        let record = CallRecord {
            id: id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            purpose: purpose.into(),
            model: cfg.llm.model.clone(),
            cost_usd: billing.usd(),
            billing: Some(billing),
            usage,
            finish_reason: response.finish_reason.clone(),
            max_output_tokens: max_tokens,
        };
        let (tx, rx) = oneshot::channel();
        self.events.send(Event::Response(
            id,
            record,
            json!({"elapsed_ms":started.elapsed().as_millis(),"input_text_bytes":text_bytes}),
            response.message.clone(),
            request_key,
            tx,
        ))?;
        rx.await?.map_err(anyhow::Error::msg)?;
        if exceeded {
            self.stop_queued();
            anyhow::bail!("已达到 token 预算，后续调用已中断；本次实际用量已保存");
        }
        ensure!(response.finish_reason.as_deref() != Some("length"),
            "模型回答被截断（finish_reason=length），本次请求输出上限 {max_tokens} token，设置值 {} token。截断内容未采用；本次实际用量已保存。请缩小批次或提高输出上限；限制原因：{}",cfg.llm.max_output_tokens,if reasons.is_empty(){"设置的完整输出额度".into()}else{reasons.join("、")});
        ensure!(
            !response.message.content.trim().is_empty() || !response.message.tool_calls.is_empty(),
            "模型没有返回最终回答或工具调用；本次实际用量已保存"
        );
        Ok(response.message)
    }
}

/// A single writer commits usage and domain checkpoints while read-only workers run concurrently.
pub async fn run<T, F, Fut, Commit>(
    task: &mut Task,
    cfg: &AppConfig,
    store: &TaskStore,
    cancel: &CancellationToken,
    mut commit: Commit,
    work: F,
) -> Result<T>
where
    T: Send,
    F: FnOnce(Runtime, Task) -> Fut + Send,
    Fut: Future<Output = Result<T>> + Send,
    Commit: FnMut(&mut Task, &str, &Value) -> Result<()> + Send,
{
    validate_config(cfg)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runtime = Runtime {
        config: Arc::new(cfg.clone()),
        client: Arc::new(OpenAiClient::from_config(cfg)?),
        cancel: cancel.child_token(),
        events: tx,
        ledger: Arc::new(Mutex::new(Ledger {
            used: task.usage().total().saturating_add(
                task.pending_calls.iter().fold(0u64, |sum, c| {
                    sum.saturating_add(c["reserved_tokens"].as_u64().unwrap_or(0))
                }),
            ),
            reserved: 0,
        })),
        changed: Arc::new(Notify::new()),
        slots: Arc::new(Semaphore::new(cfg.llm.parallel_requests)),
        stopped: Arc::new(AtomicBool::new(false)),
        cache_directory: crate::response_cache::directory(store, task),
    };
    let stop = runtime.cancel.clone();
    let future = work(runtime, task.clone());
    tokio::pin!(future);
    loop {
        tokio::select! {
            outcome = &mut future => return outcome,
            Some(event) = rx.recv() => {
                let (outcome, ack) = match event {
                    Event::Request(detail, ack) => {
                        task.pending_calls.push(detail.clone());
                        (store.save(task).and_then(|_| store.event(task,"llm_request",detail)),ack)
                    }
                    Event::Response(id, record, detail, message, request_key, ack) => {
                        // The answer and usage share one atomic envelope. A crash before
                        // task.json commits leaves a reservation that startup can reconcile.
                        if let Some(directory) = crate::response_cache::directory(store, task) {
                            crate::response_cache::save(&directory.join(format!("{request_key}.json")),
                                &crate::response_cache::Response { request_key, record: record.clone(), message })?;
                        }
                        task.pending_calls.retain(|c|c["id"]!=id);
                        let mut response = serde_json::to_value(&record)?;
                        response.as_object_mut().unwrap().extend(detail.as_object().unwrap().clone());
                        task.calls.push(record);
                        (store.save(task).and_then(|_|store.event(task,"llm_response",response)),ack)
                    }
                    Event::Domain(kind, detail, ack) => {
                        if kind=="llm_rejected" { task.pending_calls.retain(|c|c["id"]!=detail["id"]); }
                        (commit(task,&kind,&detail).and_then(|_|store.save(task)).and_then(|_|store.event(task,&kind,detail)),ack)
                    }
                };
                let failed = outcome.is_err();
                let _ = ack.send(outcome.as_ref().map(|_|()).map_err(|e|e.to_string()));
                if failed { stop.cancel(); outcome?; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_limits_grow_and_stay_bounded() {
        let mut cfg = AppConfig::default();
        cfg.llm.context_length = 32_000;
        let small = initial_text_limit(&cfg);
        cfg.llm.context_length = 1_000_000;
        assert!(initial_text_limit(&cfg) > small * 10);
        assert!(conversation_text_limit(&cfg) > 64 * 1024);
        cfg.llm.parallel_requests = 0;
        assert!(validate_config(&cfg).is_err());
    }
}
