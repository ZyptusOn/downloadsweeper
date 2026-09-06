//! LLM 相关类型。

use serde::{Deserialize, Serialize};

use crate::cost::Usage;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// 已解析为 JSON 值的参数（OpenAI 原生用字符串，出入站做转换）。
    pub args: serde_json::Value,
}

/// 图像内容块：base64 数据 + MIME（缩略图走 data URI 注入）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    pub data_base64: String,
    pub mime: String,
    /// Document pages need legible text; ordinary photo/video previews stay low detail.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub high_detail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    /// 文本内容（assistant 带 tool_calls 时可能为空字符串）。
    #[serde(default)]
    pub content: String,
    /// 附加图像块（用户消息多模态用；OpenAI content 转数组）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// 仅 role=Tool 时使用，对应它回复的 tool_call.id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Provider continuation state for thinking-mode tool turns; never shown as user-facing reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Opaque continuation blocks (signed thinking / encrypted reasoning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<serde_json::Value>,
}

impl Message {
    pub fn system(s: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: s.into(),
            images: vec![],
            tool_calls: vec![],
            tool_call_id: None,
            reasoning_content: None,
            provider_state: None,
        }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: s.into(),
            images: vec![],
            tool_calls: vec![],
            tool_call_id: None,
            reasoning_content: None,
            provider_state: None,
        }
    }
    /// 多模态用户消息：文本 + 若干图像块。
    pub fn user_with_images(s: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self {
            role: Role::User,
            content: s.into(),
            images,
            tool_calls: vec![],
            tool_call_id: None,
            reasoning_content: None,
            provider_state: None,
        }
    }
    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            images: vec![],
            tool_calls,
            tool_call_id: None,
            reasoning_content: None,
            provider_state: None,
        }
    }
    pub fn tool_result(tool_call_id: &str, result: serde_json::Value) -> Self {
        Self {
            role: Role::Tool,
            content: result.to_string(),
            images: vec![],
            tool_calls: vec![],
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
            provider_state: None,
        }
    }
}

/// 工具定义（OpenAI function-calling 形式）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema 对象。
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: Option<u64>,
    pub thinking_mode: bool,
}

/// 单次 chat 响应（非流式；流式留作后续扩展点）。
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub billing_metadata: serde_json::Value,
    pub message: Message,
    pub usage: Usage,
    pub finish_reason: Option<String>,
}
