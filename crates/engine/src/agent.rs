//! Agent：工具调用编排循环 + 会话上下文（可保存/加载为 JSON）。
//!
//! 循环：用户消息 → LLM → 若有 tool_calls 则逐个执行（经权限网关）→
//! 把结果回灌 → 再 LLM → 直至无 tool_calls 产出最终答复。
//! 全程写轨迹、累计 token、检查预算与取消。

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::cost::CostTracker;
use crate::llm::{ChatOptions, LlmClient, Message};
use crate::tools::{ToolContext, ToolRegistry};
use crate::trajectory::EventKind;
use crate::Result;

/// 一次会话的完整上下文，可序列化为 JSON 保存/加载。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub id: Uuid,
    pub created_at: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<Uuid>,
}

impl SessionContext {
    pub fn new(system_prompt: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: Utc::now().to_rfc3339(),
            messages: vec![Message::system(system_prompt)],
            batch_id: None,
        }
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

/// Agent 主控。
pub struct Agent {
    client: Arc<dyn LlmClient>,
    tools: Arc<ToolRegistry>,
    ctx: Arc<ToolContext>,
    cost: Arc<CostTracker>,
    config: Arc<AppConfig>,
    cancel: CancellationToken,
}

impl Agent {
    pub fn new(
        client: Arc<dyn LlmClient>,
        tools: Arc<ToolRegistry>,
        ctx: Arc<ToolContext>,
        cost: Arc<CostTracker>,
        config: Arc<AppConfig>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            client,
            tools,
            ctx,
            cost,
            config,
            cancel,
        }
    }

    /// 默认系统提示词：说明工具用途与“规则优先、LLM 兜底”的工作方式。
    pub fn default_system_prompt(scan_root: &std::path::Path) -> String {
        format!(
            "你是 DownloadSweeper，一个专门整理下载目录的助手。\n\
             工作方式：\n\
             1) 先 scan_downloads 获取文件清单与每个文件的访问层级(tier)。\n\
             2) 能用 classify_by_rules（按扩展名，零成本）解决的优先用它，输出计划。\n\
             3) 仅对 tier 允许且规则未命中的歧义文件，用 read_content_slice 读取后分类。\n\
             4) 用 move_batch(dry_run=true) 先给出预览，确认后再真正执行。\n\
             权限由系统强制：tier=none 的文件只能按扩展名规则处理，不要尝试读取其内容。\n\
             下载目录根：{root}",
            root = scan_root.display()
        )
    }

    /// 执行一轮用户请求，返回最终文本答复。
    pub async fn run(&self, session: &mut SessionContext, user_msg: &str) -> Result<String> {
        // 会话首次运行：自动分配批次号以贯穿轨迹审计链
        if session.batch_id.is_none() {
            session.batch_id = Some(Uuid::new_v4());
            self.log(
                EventKind::SessionStarted,
                session.batch_id,
                serde_json::json!({ "session_id": session.id.to_string() }),
            )?;
        }
        session.messages.push(Message::user(user_msg));
        let tool_defs = self.tools.defs();
        let options = ChatOptions {
            model: self.config.llm.model.clone(),
            temperature: self.config.llm.temperature,
            max_tokens: None,
            thinking_mode: self.config.llm.thinking_mode,
        };

        for _ in 0..self.config.max_iterations {
            // 取消传播
            if self.cancel.is_cancelled() {
                self.log(EventKind::Cancelled, None, serde_json::json!({}))?;
                anyhow::bail!("任务已被用户取消");
            }

            // 预算预检
            if self.cost.total().total() > self.config.token_budget.unwrap_or(u64::MAX) {
                self.log(
                    EventKind::BudgetExceeded,
                    session.batch_id,
                    serde_json::json!({}),
                )?;
                anyhow::bail!("token 预算已耗尽");
            }

            self.log(
                EventKind::LlmRequest,
                session.batch_id,
                crate::trajectory::llm_request_detail(
                    &options.model,
                    session.messages.len(),
                    tool_defs.len(),
                ),
            )?;

            let resp = self
                .client
                .chat(&session.messages, &tool_defs, &options, &self.cancel)
                .await?;

            // 累计用量；触达预算则记录并中断
            if let Err(e) = self.cost.add(&resp.usage) {
                self.log(
                    EventKind::BudgetExceeded,
                    session.batch_id,
                    serde_json::json!({ "error": e.to_string(), "usage": format!("{:?}", resp.usage) }),
                )?;
                session.messages.push(resp.message);
                return Err(anyhow::Error::new(e));
            }

            self.log(
                EventKind::LlmResponse,
                session.batch_id,
                crate::trajectory::llm_response_detail(
                    resp.usage.prompt_tokens,
                    resp.usage.completion_tokens,
                    &resp
                        .message
                        .tool_calls
                        .iter()
                        .map(|t| t.name.clone())
                        .collect::<Vec<_>>(),
                ),
            )?;

            let has_tool_calls = !resp.message.tool_calls.is_empty();
            session.messages.push(resp.message);

            if !has_tool_calls {
                // 最终答复
                return Ok(session
                    .messages
                    .last()
                    .map(|m| m.content.clone())
                    .unwrap_or_default());
            }

            // 执行所有 tool_calls
            let tool_calls = session
                .messages
                .last()
                .map(|m| m.tool_calls.clone())
                .unwrap_or_default();
            for tc in tool_calls {
                // An LLM cannot authorize mutations. The UI/CLI execution boundary owns that decision.
                if tc.name == "trash_batch"
                    || tc.name == "move_batch"
                        && tc.args.get("dry_run") != Some(&serde_json::Value::Bool(true))
                {
                    session.messages.push(Message::tool_result(&tc.id, serde_json::json!({"error":"AI 只能生成预览；真实文件操作必须经过用户审查的计划执行"})));
                    continue;
                }
                self.log(
                    EventKind::ToolCall,
                    session.batch_id,
                    serde_json::json!({ "id": tc.id, "name": tc.name, "args": tc.args }),
                )?;
                let result = self
                    .tools
                    .call(&tc.name, tc.args.clone(), &self.ctx, &self.cancel)
                    .await?;
                let is_err = result.get("error").is_some();
                self.log(
                    EventKind::ToolResult,
                    session.batch_id,
                    serde_json::json!({ "id": tc.id, "name": tc.name, "ok": !is_err, "result": result }),
                )?;
                session.messages.push(Message::tool_result(&tc.id, result));
            }
        }
        anyhow::bail!("达到最大迭代次数 {} 仍未结束", self.config.max_iterations);
    }

    fn log(
        &self,
        kind: EventKind,
        batch_id: Option<Uuid>,
        detail: serde_json::Value,
    ) -> Result<()> {
        self.ctx.trajectory.log(kind, batch_id, detail)?;
        Ok(())
    }
}
