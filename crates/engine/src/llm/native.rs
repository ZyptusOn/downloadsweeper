//! Native Messages and Responses adapters for the shared tool/vision workflow.
use super::{providers, ChatOptions, ChatResponse, Message, Role, ToolCall, ToolDef};
use crate::cost::Usage;
use anyhow::{ensure, Context};
use serde_json::{json, Value};

pub fn anthropic_body(
    messages: &[Message],
    tools: &[ToolDef],
    options: &ChatOptions,
) -> anyhow::Result<Value> {
    let mut turns: Vec<Value> = vec![];
    let mut system = vec![];
    for message in messages {
        if message.role == Role::System {
            system.push(message.content.clone());
            continue;
        }
        let role = if message.role == Role::Assistant {
            "assistant"
        } else {
            "user"
        };
        let mut blocks = vec![];
        if message.role == Role::Assistant
            && message
                .provider_state
                .as_ref()
                .is_some_and(|s| s["format"] == "anthropic")
        {
            blocks = message.provider_state.as_ref().unwrap()["content"]
                .as_array()
                .cloned()
                .unwrap_or_default();
        } else if message.role == Role::Tool {
            blocks.push(json!({"type":"tool_result","tool_use_id":message.tool_call_id,"content":message.content}));
        } else {
            if !message.content.is_empty() {
                blocks.push(json!({"type":"text","text":message.content}));
            }
            for image in &message.images {
                blocks.push(json!({"type":"image","source":{"type":"base64","media_type":image.mime,"data":image.data_base64}}));
            }
            for call in &message.tool_calls {
                blocks.push(
                    json!({"type":"tool_use","id":call.id,"name":call.name,"input":call.args}),
                );
            }
        }
        if blocks.is_empty() {
            continue;
        }
        // All tool results of a parallel assistant turn must be in one user turn.
        if turns.last().is_some_and(|t| t["role"] == role) {
            turns.last_mut().unwrap()["content"]
                .as_array_mut()
                .unwrap()
                .extend(blocks);
        } else {
            turns.push(json!({"role":role,"content":blocks}));
        }
    }
    let max = options.max_tokens.unwrap_or(16384);
    let mut body = json!({"model":options.model,"system":system.join("\n\n"),"messages":turns,"max_tokens":max});
    if !tools.is_empty() {
        body["tools"] = json!(tools.iter().map(|tool| json!({"name":tool.name,"description":tool.description,"input_schema":tool.parameters})).collect::<Vec<_>>());
    }
    let m = options.model.rsplit('/').next().unwrap_or(&options.model);
    let adaptive = [
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-fable-",
    ]
    .iter()
    .any(|prefix| m.starts_with(prefix));
    let always = m.starts_with("claude-fable-");
    if options.thinking_mode || always {
        if adaptive {
            body["thinking"] = json!({"type":"adaptive"});
            body["output_config"] =
                json!({"effort":if options.thinking_mode {"high"} else {"low"}});
        } else {
            ensure!(
                max > 1024,
                "此 Claude 模型的思考模式需要单次输出上限大于 1024 token"
            );
            body["thinking"] = json!({"type":"enabled","budget_tokens":8192_u64.min(max - 1)});
        }
    } else {
        body["thinking"] = json!({"type":"disabled"});
    }
    // New Claude generations reject sampling controls; use provider defaults for all.
    Ok(body)
}

pub fn anthropic_response(value: &Value) -> anyhow::Result<ChatResponse> {
    let blocks = value["content"]
        .as_array()
        .context("Claude 响应缺少 content")?;
    let mut text = String::new();
    let mut calls = vec![];
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
            Some("tool_use") => calls.push(ToolCall {
                id: block["id"].as_str().context("工具调用缺少 ID")?.into(),
                name: block["name"].as_str().context("工具调用缺少名称")?.into(),
                args: block["input"].clone(),
            }),
            _ => {}
        }
    }
    let mut message = Message::assistant(text, calls);
    message.provider_state = Some(json!({"format":"anthropic","content":blocks}));
    let usage = &value["usage"];
    let input = usage["input_tokens"]
        .as_u64()
        .context("服务未返回输入 token 用量")?;
    let prompt_tokens = input
        .saturating_add(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0))
        .saturating_add(usage["cache_read_input_tokens"].as_u64().unwrap_or(0));
    Ok(ChatResponse {
        billing_metadata: json!({"usage":value["usage"],"service_tier":value["service_tier"]}),
        message,
        usage: Usage {
            prompt_tokens,
            cached_input_tokens: usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
            cache_write_tokens: usage["cache_creation_input_tokens"].as_u64().unwrap_or(0),
            cache_write_1h_tokens: usage["cache_creation"]["ephemeral_1h_input_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_details_known: usage["cache_read_input_tokens"].is_u64(),
            completion_tokens: usage["output_tokens"]
                .as_u64()
                .context("服务未返回输出 token 用量")?,
        },
        finish_reason: Some(
            match value["stop_reason"].as_str().unwrap_or("unknown") {
                "max_tokens" | "model_context_window_exceeded" => "length",
                "tool_use" => "tool_calls",
                "end_turn" | "stop_sequence" => "stop",
                other => other,
            }
            .into(),
        ),
    })
}

pub fn responses_body(messages: &[Message], tools: &[ToolDef], options: &ChatOptions) -> Value {
    let mut input = vec![];
    for message in messages {
        if message.role == Role::Assistant
            && message
                .provider_state
                .as_ref()
                .is_some_and(|s| s["format"] == "responses")
        {
            input.extend(
                message.provider_state.as_ref().unwrap()["output"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
        } else if message.role == Role::Tool {
            input.push(json!({"type":"function_call_output","call_id":message.tool_call_id,"output":message.content}));
        } else {
            let role = match message.role {
                Role::System => "developer",
                Role::Assistant => "assistant",
                _ => "user",
            };
            if message.role == Role::Assistant {
                if !message.content.is_empty() {
                    input.push(json!({"role":role,"content":message.content}));
                }
                for call in &message.tool_calls {
                    input.push(json!({"type":"function_call","call_id":call.id,"name":call.name,"arguments":call.args.to_string()}));
                }
            } else {
                let mut content = vec![json!({"type":"input_text","text":message.content})];
                for image in &message.images {
                    let mut block = json!({"type":"input_image","image_url":format!("data:{};base64,{}", image.mime, image.data_base64)});
                    if providers::model_family(&options.model) == "openai_reasoning" {
                        block["detail"] = json!(if image.high_detail { "high" } else { "low" });
                    }
                    content.push(block);
                }
                input.push(json!({"role":role,"content":content}));
            }
        }
    }
    let mut body = json!({"model":options.model,"input":input,"store":false,"max_output_tokens":options.max_tokens.unwrap_or(16384)});
    if providers::model_family(&options.model) == "openai_reasoning" {
        body["reasoning"] =
            json!({"effort":providers::openai_effort(&options.model, options.thinking_mode)});
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools.iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters,"strict":false})).collect::<Vec<_>>());
    }
    body
}

pub fn responses_response(value: &Value) -> anyhow::Result<ChatResponse> {
    ensure!(
        value["error"].is_null() && value["status"] != "failed",
        "Responses 返回失败状态"
    );
    let output = value["output"]
        .as_array()
        .context("Responses 响应缺少 output")?;
    let mut text = String::new();
    let mut calls = vec![];
    for item in output {
        if item["type"] == "function_call" {
            calls.push(ToolCall {
                id: item["call_id"]
                    .as_str()
                    .context("工具调用缺少 call_id")?
                    .into(),
                name: item["name"].as_str().context("工具调用缺少名称")?.into(),
                args: serde_json::from_str(item["arguments"].as_str().context("工具调用缺少参数")?)
                    .map_err(|_| anyhow::anyhow!("工具调用参数不是有效 JSON"))?,
            });
        } else if item["type"] == "message" {
            for block in item["content"].as_array().into_iter().flatten() {
                if block["type"] == "output_text" {
                    text.push_str(block["text"].as_str().unwrap_or(""));
                }
            }
        }
    }
    let finish = if value["status"] == "incomplete" {
        "length"
    } else if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    let mut message = Message::assistant(text, calls);
    message.provider_state = Some(json!({"format":"responses","output":output}));
    Ok(ChatResponse {
        billing_metadata: json!({"usage":value["usage"],"service_tier":value["service_tier"]}),
        message,
        usage: Usage {
            prompt_tokens: value["usage"]["input_tokens"]
                .as_u64()
                .context("服务未返回输入 token 用量")?,
            completion_tokens: value["usage"]["output_tokens"]
                .as_u64()
                .context("服务未返回输出 token 用量")?,
            ..crate::pricing::cache_usage(&value["usage"], true)
        },
        finish_reason: Some(finish.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(model: &str) -> ChatOptions {
        ChatOptions {
            model: model.into(),
            temperature: 0.2,
            max_tokens: Some(16384),
            thinking_mode: true,
        }
    }
    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "inspect".into(),
            description: "read allowed metadata".into(),
            parameters: json!({"type":"object","properties":{"id":{"type":"string"}}}),
        }]
    }

    #[test]
    fn claude_preserves_signed_blocks_and_combines_parallel_results() {
        let response = anthropic_response(&json!({"content":[
            {"type":"thinking","thinking":"opaque","signature":"signed-state"},
            {"type":"tool_use","id":"one","name":"inspect","input":{"id":"a"}},
            {"type":"tool_use","id":"two","name":"inspect","input":{"id":"b"}}
        ],"usage":{"input_tokens":4,"cache_read_input_tokens":20,"cache_creation_input_tokens":6,"output_tokens":7},"stop_reason":"tool_use"})).unwrap();
        assert_eq!(response.usage.prompt_tokens, 30);
        assert_eq!(response.usage.cached_input_tokens, 20);
        assert_eq!(response.usage.cache_write_tokens, 6);
        assert!(response.usage.cache_details_known);
        let body = anthropic_body(
            &[
                Message::system("rules"),
                Message::user("classify"),
                response.message,
                Message::tool_result("one", json!({"ok":true})),
                Message::tool_result("two", json!({"ok":true})),
            ],
            &tools(),
            &options("claude-opus-4-8"),
        )
        .unwrap();
        assert_eq!(body["system"], "rules");
        assert_eq!(
            body["messages"][1]["content"][0]["signature"],
            "signed-state"
        );
        assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body.get("temperature").is_none());
        assert!(body["tools"][0].get("input_schema").is_some());
    }

    #[test]
    fn responses_preserves_encrypted_reasoning_and_function_call_ids() {
        let response = responses_response(&json!({"status":"completed","output":[
            {"type":"reasoning","id":"r1","summary":[],"encrypted_content":"opaque-state"},
            {"type":"function_call","call_id":"c1","name":"inspect","arguments":"{\"id\":\"a\"}"}
        ],"usage":{"input_tokens":11,"output_tokens":12}}))
        .unwrap();
        let body = responses_body(
            &[
                Message::user("classify"),
                response.message,
                Message::tool_result("c1", json!({"ok":true})),
            ],
            &tools(),
            &options("gpt-6-astra"),
        );
        assert_eq!(body["input"][1]["encrypted_content"], "opaque-state");
        assert_eq!(body["input"][3]["call_id"], "c1");
        assert_eq!(body["store"], false);
        assert_eq!(body["tools"][0]["strict"], false);
        assert_eq!(body["max_output_tokens"], 16384);
    }

    #[test]
    fn native_usage_is_required_and_images_are_encoded() {
        assert!(anthropic_response(&json!({"content":[],"usage":{}})).is_err());
        assert!(responses_response(&json!({"output":[],"usage":{}})).is_err());
        let message = Message::user_with_images(
            "image",
            vec![super::super::ImageData {
                high_detail: false,
                mime: "image/png".into(),
                data_base64: "AA==".into(),
            }],
        );
        let body = anthropic_body(&[message.clone()], &[], &options("claude-haiku-4-5")).unwrap();
        assert_eq!(
            body["messages"][0]["content"][1]["source"]["media_type"],
            "image/png"
        );
        let body = responses_body(&[message], &[], &options("gpt-5.6-sol"));
        assert_eq!(
            body["input"][0]["content"][1]["image_url"],
            "data:image/png;base64,AA=="
        );
    }
}
