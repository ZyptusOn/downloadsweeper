//! LLM 模块。

pub mod client;
pub mod discovery;
mod native;
pub mod openai;
pub mod providers;
pub mod types;

pub use client::LlmClient;
pub use openai::OpenAiClient;
pub use types::{ChatOptions, ChatResponse, ImageData, Message, Role, ToolCall, ToolDef};
