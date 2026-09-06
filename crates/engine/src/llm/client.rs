//! LLM 客户端 trait。OpenAI 兼容实现见 `openai` 模块。

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::llm::{ChatOptions, ChatResponse, Message, ToolDef};
use crate::Result;

/// Explicit rejection or local request construction failure, rather than an uncertain completion.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RejectedRequest(pub String);

#[async_trait]
pub trait LlmClient: Send + Sync {
    /// 发起一轮对话。`cancel` 用于在流式/请求阶段中断。
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        options: &ChatOptions,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse>;
}
