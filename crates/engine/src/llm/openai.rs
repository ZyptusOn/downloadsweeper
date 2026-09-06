//! Chat Completions、Responses 与 Anthropic Messages 客户端实现。
//!
//! 非流式：骨架阶段先保证“能跑通一轮工具调用”。流式（SSE）进度渲染
//! 留作后续扩展点——届时把 `chat` 改为返回事件流即可，trait 不变。

use super::providers::{self, ApiFormat};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::cost::Usage;
use crate::llm::client::LlmClient;
use crate::llm::types::{ChatOptions, ChatResponse, Message, Role, ToolCall, ToolDef};
use crate::Result;

/// 归一化 endpoint：同时接受 base（`https://api.deepseek.com` 或 `.../v1`）
/// 与完整路径（`.../v1/chat/completions`）；末尾不带 `/chat/completions` 时追加。
pub fn normalize_endpoint(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/').to_string();
    if e.ends_with("/chat/completions") {
        e
    } else {
        format!("{e}/chat/completions")
    }
}

pub struct OpenAiClient {
    http: reqwest::Client,
    endpoint: String,
    api_key: String,
    timeout_seconds: u64,
    format: ApiFormat,
}

impl OpenAiClient {
    pub fn new(endpoint: String, api_key: String) -> Result<Self> {
        Self::with_timeout(endpoint, api_key, 600)
    }

    pub fn with_timeout(endpoint: String, api_key: String, timeout_seconds: u64) -> Result<Self> {
        anyhow::ensure!(
            (1..=3600).contains(&timeout_seconds),
            "请求超时应在 1 到 3600 秒之间"
        );
        let url = providers::validate_url(&endpoint)?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "Endpoint 必须使用 http 或 https"
        );
        static POOL: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
        let http = if let Some(client) = POOL.get() {
            client.clone()
        } else {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()?;
            let _ = POOL.set(client.clone());
            client
        };
        Ok(Self {
            http,
            endpoint: normalize_endpoint(&endpoint),
            api_key,
            timeout_seconds,
            format: ApiFormat::ChatCompletions,
        })
    }

    pub fn from_config(config: &crate::config::AppConfig) -> Result<Self> {
        let format = providers::api_format(
            &config.llm.endpoint,
            &config.llm.model,
            config.llm.api_format,
        );
        let mut client = Self::with_timeout(
            config.llm.endpoint.clone(),
            config.resolve_api_key().unwrap_or_default(),
            config.llm.request_timeout_seconds,
        )?;
        client.format = format;
        client.endpoint = providers::endpoint_for(
            &config.llm.endpoint,
            match format {
                ApiFormat::Anthropic => "/messages",
                ApiFormat::Responses => "/responses",
                _ => "/chat/completions",
            },
        )?;
        Ok(client)
    }

    fn request_error(&self, error: reqwest::Error) -> anyhow::Error {
        let protocol = match self.format {
            ApiFormat::Anthropic => "Anthropic Messages",
            ApiFormat::Responses => "Responses",
            _ => "Chat Completions",
        };
        if error.is_timeout() {
            anyhow::anyhow!("等待 {protocol} 响应超时（请求上限 {} 秒，连接上限 15 秒）。可在模型设置中增加「请求超时」，或减少输出上限、关闭思考模式后重试。未收到完整 usage，本次用量无法确认；没有自动重试。", self.timeout_seconds)
        } else {
            anyhow::anyhow!("{protocol} 网络请求失败：{}", error.without_url())
        }
    }

    fn build_body(&self, messages: &[Message], tools: &[ToolDef], options: &ChatOptions) -> Value {
        let msgs: Vec<Value> = messages
            .iter()
            .map(|m| {
                let mut o = json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Tool => "tool",
                    },
                });
                // 多模态：content 为数组 [text, image_url...]
                if !m.images.is_empty() {
                    let mut parts = vec![json!({ "type": "text", "text": m.content })];
                    for img in &m.images {
                        parts.push(json!({
                            "type": "image_url",
                            "image_url": providers::image_url(&self.endpoint, img)
                        }));
                    }
                    o["content"] = Value::Array(parts);
                } else {
                    o["content"] = Value::String(m.content.clone());
                }
                if !m.tool_calls.is_empty() {
                    let tcs: Vec<Value> = m
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.args.to_string(),
                                }
                            })
                        })
                        .collect();
                    o["tool_calls"] = Value::Array(tcs);
                }
                if let Some(id) = &m.tool_call_id {
                    o["tool_call_id"] = Value::String(id.clone());
                }
                if let Some(reasoning) = &m.reasoning_content {
                    o["reasoning_content"] = json!(reasoning);
                }
                o
            })
            .collect();

        let mut body = json!({
            "model": options.model,
            "messages": msgs,
            "temperature": options.temperature,
        });
        if let Some(max) = options.max_tokens {
            body["max_tokens"] = json!(max);
        }
        let model = options.model.to_lowercase();
        providers::tune_chat(&mut body, &options.model, options.thinking_mode);
        if !tools.is_empty() {
            let tools_json: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools_json);
            // DeepSeek thinking tool turns reject tool_choice, and default to automatic selection.
            if !(model.contains("deepseek") && options.thinking_mode) {
                body["tool_choice"] = json!("auto");
            }
        }
        body
    }

    fn parse_response(v: &Value) -> Result<ChatResponse> {
        let choice = v
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or_else(|| anyhow::anyhow!("LLM 响应缺少 choices"))?;
        let msg = choice
            .get("message")
            .ok_or_else(|| anyhow::anyhow!("缺少 message"))?;

        let content = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let tool_calls = msg
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        let id = tc.get("id")?.as_str()?.to_string();
                        let func = tc.get("function")?;
                        let name = func.get("name")?.as_str()?.to_string();
                        let args_str = func
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .unwrap_or("{}");
                        let args = serde_json::from_str(args_str).unwrap_or(Value::Null);
                        Some(ToolCall { id, name, args })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let usage = Usage {
            prompt_tokens: v["usage"]["prompt_tokens"].as_u64().ok_or_else(|| {
                anyhow::anyhow!(
                    "服务未返回输入 token 用量，无法进行准确计费；请使用支持 usage 的兼容服务"
                )
            })?,
            completion_tokens: v["usage"]["completion_tokens"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("服务未返回输出 token 用量，无法进行准确计费"))?,
            ..crate::pricing::cache_usage(&v["usage"], false)
        };

        let mut message = Message::assistant(content, tool_calls);
        message.reasoning_content = msg["reasoning_content"].as_str().map(str::to_owned);
        Ok(ChatResponse {
            billing_metadata: serde_json::json!({"usage":v["usage"],"service_tier":v["service_tier"]}),
            message,
            usage,
            finish_reason: choice["finish_reason"].as_str().map(str::to_owned),
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        options: &ChatOptions,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse> {
        let body = match self.format {
            ApiFormat::Anthropic => super::native::anthropic_body(messages, tools, options)
                .map_err(|e| super::client::RejectedRequest(e.to_string()))?,
            ApiFormat::Responses => super::native::responses_body(messages, tools, options),
            _ => self.build_body(messages, tools, options),
        };
        let request = self
            .http
            .post(&self.endpoint)
            .timeout(std::time::Duration::from_secs(self.timeout_seconds))
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        let request = providers::authorize(request, &self.endpoint, self.format, &self.api_key);
        let req = request.send();

        // 与取消令牌竞速。
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(anyhow::anyhow!("任务已取消"));
            }
            r = req => r.map_err(|e| self.request_error(e))?,
        };

        let status = resp.status();
        let text = tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("任务已取消"),
            text = async {
                use futures::StreamExt;
                anyhow::ensure!(resp.content_length().unwrap_or(0)<=8*1024*1024,"模型响应超过 8 MiB 安全上限");
                let mut stream=resp.bytes_stream(); let mut bytes=Vec::new();
                while let Some(chunk)=stream.next().await {
                    let chunk=chunk.map_err(|e|self.request_error(e))?;
                    anyhow::ensure!(bytes.len()+chunk.len()<=8*1024*1024,"模型响应超过 8 MiB 安全上限");
                    bytes.extend_from_slice(&chunk);
                }
                Ok::<String,anyhow::Error>(String::from_utf8(bytes)?)
            } => text?,
        };
        if !status.is_success() {
            let detail = if self.api_key.is_empty() {
                text
            } else {
                text.replace(&self.api_key, "[REDACTED]")
            };
            let detail = detail.chars().take(800).collect::<String>();
            if status.is_client_error() {
                return Err(super::client::RejectedRequest(format!(
                    "LLM 请求被拒绝 [{status}]: {detail}"
                ))
                .into());
            }
            anyhow::bail!("LLM 请求失败 [{status}]: {detail}");
        }
        let v: Value = serde_json::from_str(&text)?;
        match self.format {
            ApiFormat::Anthropic => super::native::anthropic_response(&v),
            ApiFormat::Responses => super::native::responses_response(&v),
            _ => Self::parse_response(&v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ImageData;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn stalled_body_server() -> (
        String,
        tokio::sync::oneshot::Receiver<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let count = stream.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..count])
                .starts_with("POST /v1/chat/completions HTTP/1.1"));
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n").await.unwrap();
            let _ = tx.send(());
            std::future::pending::<()>().await;
        });
        (format!("http://{address}/v1"), rx, server)
    }

    #[tokio::test]
    async fn stalled_completion_body_uses_configured_timeout_and_clear_error() {
        let (endpoint, _, server) = stalled_body_server().await;
        let client = OpenAiClient::with_timeout(endpoint, String::new(), 1).unwrap();
        let error = client
            .chat(
                &[Message::user("test")],
                &[],
                &ChatOptions {
                    model: "fixture".into(),
                    temperature: 0.2,
                    max_tokens: Some(64),
                    thinking_mode: false,
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Chat Completions 响应超时"));
        assert!(error.to_string().contains("请求上限 1 秒"));
        server.abort();
    }

    #[tokio::test]
    async fn stalled_completion_body_can_be_cancelled_before_long_timeout() {
        let (endpoint, headers, server) = stalled_body_server().await;
        let client = OpenAiClient::with_timeout(endpoint, String::new(), 600).unwrap();
        let cancel = CancellationToken::new();
        let request_cancel = cancel.clone();
        let request = tokio::spawn(async move {
            client
                .chat(
                    &[Message::user("test")],
                    &[],
                    &ChatOptions {
                        model: "fixture".into(),
                        temperature: 0.2,
                        max_tokens: Some(64),
                        thinking_mode: false,
                    },
                    &request_cancel,
                )
                .await
        });
        headers.await.unwrap();
        cancel.cancel();
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("已取消"));
        server.abort();
    }

    fn msg_with_img() -> Message {
        Message::user_with_images(
            "看看这张图",
            vec![ImageData {
                high_detail: false,
                data_base64: "QUJD".into(),
                mime: "image/png".into(),
            }],
        )
    }

    #[test]
    fn multimodal_content_becomes_array() {
        let client = OpenAiClient::new("http://x".into(), "k".into()).unwrap();
        let body = client.build_body(
            &[msg_with_img()],
            &[],
            &ChatOptions {
                model: "gpt-4o".into(),
                temperature: 0.1,
                max_tokens: None,
                thinking_mode: false,
            },
        );
        let content = &body["messages"][0]["content"];
        assert!(content.is_array(), "多模态 content 应为数组: {content}");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        let url = content[1]["image_url"]["url"].as_str().unwrap();
        assert!(
            url.starts_with("data:image/png;base64,QUJD"),
            "data uri 拼接错误: {url}"
        );
    }

    #[test]
    fn plain_message_stays_string() {
        let client = OpenAiClient::new("http://x".into(), "k".into()).unwrap();
        let body = client.build_body(
            &[Message::user("hello")],
            &[],
            &ChatOptions {
                model: "gpt-4o".into(),
                temperature: 0.1,
                max_tokens: None,
                thinking_mode: false,
            },
        );
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn deepseek_tool_turn_preserves_continuation_without_tool_choice() {
        let response = OpenAiClient::parse_response(&json!({
            "choices":[{"finish_reason":"tool_calls","message":{"content":"","reasoning_content":"opaque continuation",
                "tool_calls":[{"id":"call_1","function":{"name":"read_file_evidence","arguments":"{\"file_ids\":[\"f1\"]}"}}]}}],
            "usage":{"prompt_tokens":80,"completion_tokens":20}
        })).unwrap();
        let client = OpenAiClient::new("http://fixture".into(), String::new()).unwrap();
        let body = client.build_body(
            &[
                response.message,
                Message::tool_result("call_1", json!({"files":[]})),
            ],
            &[ToolDef {
                name: "read_file_evidence".into(),
                description: "fixture".into(),
                parameters: json!({"type":"object"}),
            }],
            &ChatOptions {
                model: "deepseek-v4-flash".into(),
                temperature: 0.2,
                max_tokens: Some(1024),
                thinking_mode: true,
            },
        );
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            "opaque continuation"
        );
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            "read_file_evidence"
        );
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn endpoint_normalization() {
        assert_eq!(
            normalize_endpoint("https://api.deepseek.com"),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            normalize_endpoint("https://api.deepseek.com/"),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            normalize_endpoint("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_endpoint("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        // 客户端内部存的是归一化后的
        let c = OpenAiClient::new("http://localhost:11434/v1".into(), "k".into()).unwrap();
        assert_eq!(c.endpoint, "http://localhost:11434/v1/chat/completions");
    }
}
