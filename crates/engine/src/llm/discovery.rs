//! Read-only model discovery. Never probes unrelated hosts or sends generation requests.
use super::providers::{self, ApiFormat};
use anyhow::ensure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub vision: Option<bool>,
    pub tools: Option<bool>,
    pub context_source: String,
    pub output_source: String,
    pub capability_source: String,
    pub source_url: Option<String>,
    pub available: bool,
    pub task_compatible: bool,
    pub note: String,
}

fn number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| (64..=4_000_000).contains(n))
}

pub fn model_info(value: &Value, available: bool) -> Option<ModelInfo> {
    let id = value["id"].as_str()?;
    if id.len() > 200 || id.is_empty() || id.chars().any(char::is_control) {
        return None;
    }
    let preset = providers::preset(id).unwrap_or(Value::Null);
    let context = [
        "/context_length",
        "/context_window",
        "/max_input_tokens",
        "/top_provider/context_length",
    ]
    .iter()
    .find_map(|path| number(&value.pointer(path).cloned().unwrap_or(Value::Null)));
    let output = [
        "/max_output_tokens",
        "/max_completion_tokens",
        "/max_tokens",
        "/top_provider/max_completion_tokens",
    ]
    .iter()
    .find_map(|path| number(&value.pointer(path).cloned().unwrap_or(Value::Null)));
    let vision = value
        .pointer("/capabilities/image_input/supported")
        .and_then(Value::as_bool)
        .or_else(|| {
            value
                .pointer("/capabilities/vision/supported")
                .and_then(Value::as_bool)
        })
        .or_else(|| {
            value
                .pointer("/architecture/input_modalities")
                .and_then(Value::as_array)
                .map(|values| values.iter().any(|v| v == "image"))
        });
    let tools = value
        .pointer("/capabilities/tool_use/supported")
        .and_then(Value::as_bool)
        .or_else(|| {
            value
                .pointer("/capabilities/function_calling/supported")
                .and_then(Value::as_bool)
        });
    let lower = id.to_lowercase();
    let task_compatible = ![
        "embedding",
        "whisper",
        "tts",
        "transcribe",
        "asr",
        "dall-e",
        "gpt-image",
        "moderation",
        "realtime",
        "sora",
    ]
    .iter()
    .any(|word| lower.contains(word))
        && tools != Some(false);
    Some(ModelInfo {
        id: id.into(),
        name: value["display_name"]
            .as_str()
            .or_else(|| value["name"].as_str())
            .unwrap_or(id)
            .chars()
            .take(200)
            .collect(),
        context_length: context.or_else(|| number(&preset["context_length"])),
        max_output_tokens: output.or_else(|| number(&preset["max_output_tokens"])),
        vision: vision.or_else(|| preset["vision"].as_bool()),
        tools: tools.or_else(|| preset["tools"].as_bool()),
        context_source: if context.is_some() {
            "api"
        } else if number(&preset["context_length"]).is_some() {
            "preset"
        } else {
            "unknown"
        }
        .into(),
        output_source: if output.is_some() {
            "api"
        } else if number(&preset["max_output_tokens"]).is_some() {
            "preset"
        } else {
            "unknown"
        }
        .into(),
        capability_source: if vision.is_some() || tools.is_some() {
            "api"
        } else if !preset.is_null() {
            "preset"
        } else {
            "unknown"
        }
        .into(),
        source_url: preset["source"].as_str().map(str::to_owned),
        available,
        task_compatible,
        note: preset["note"].as_str().unwrap_or("").into(),
    })
}

pub fn fallback(endpoint: &str) -> Vec<ModelInfo> {
    let provider = providers::provider(endpoint);
    providers::catalog()["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["provider"] == provider)
        .filter_map(|v| model_info(v, false))
        .map(|mut info| {
            info.context_source = if info.context_length.is_some() {
                "preset"
            } else {
                "unknown"
            }
            .into();
            info.output_source = if info.max_output_tokens.is_some() {
                "preset"
            } else {
                "unknown"
            }
            .into();
            info
        })
        .collect()
}

pub async fn discover(endpoint: &str, selected: ApiFormat, key: &str) -> anyhow::Result<Value> {
    let url = providers::endpoint_for(endpoint, "/models")?;
    let format = providers::api_format(endpoint, "", selected);
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let mut models = std::collections::BTreeMap::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let mut request = http.get(&url);
            if format == ApiFormat::Anthropic {
                request = request.query(&[("limit", "1000")]);
            }
            if let Some(cursor) = &cursor {
                request = request.query(&[("after_id", cursor)]);
            }
            let mut response = providers::authorize(request, endpoint, format, key)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("无法连接模型列表接口，请检查地址、代理或网络"))?;
            let status = response.status();
            ensure!(
                status.is_success(),
                "{}",
                match status.as_u16() {
                    401 | 403 => "密钥无效或无模型列表权限；请核对服务商、地域及密钥",
                    404 | 405 => "此地址未提供模型列表接口；可选择官方预设或手动填写模型 ID",
                    429 => "模型列表查询限流，请稍后刷新",
                    300..=399 => "模型接口发生重定向，未转发密钥；请填写服务商的最终 API 地址",
                    _ => "模型列表服务暂时不可用，请稍后重试",
                }
            );
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("读取模型列表失败"))?
            {
                ensure!(
                    bytes.len() + chunk.len() <= 2 * 1024 * 1024,
                    "模型列表超过 2 MiB 限制"
                );
                bytes.extend_from_slice(&chunk);
            }
            // Discard an untrusted response that echoes the credential anywhere.
            ensure!(
                key.is_empty() || !bytes.windows(key.len()).any(|w| w == key.as_bytes()),
                "模型接口异常回显凭据，响应已丢弃"
            );
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("模型列表不是有效 JSON；请确认填写的是 API 地址"))?;
            let entries = value["data"]
                .as_array()
                .or_else(|| value.as_array())
                .ok_or_else(|| anyhow::anyhow!("模型列表格式不兼容（缺少 data 数组）"))?;
            for entry in entries {
                if let Some(info) = model_info(entry, true) {
                    models.insert(info.id.clone(), info);
                }
            }
            if value["has_more"] != true {
                return Ok::<_, anyhow::Error>(models.into_values().collect::<Vec<_>>());
            }
            let next = value["last_id"]
                .as_str()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| anyhow::anyhow!("模型列表分页缺少游标"))?;
            ensure!(cursor.as_deref() != Some(next), "模型列表分页游标重复");
            cursor = Some(next.into());
        }
        anyhow::bail!("模型列表分页超过上限，未将不完整结果标记为成功")
    })
    .await;
    match result {
        Ok(Ok(models)) => Ok(
            json!({"status":"ok","models":models,"presets":fallback(endpoint),"message":"列表来自当前账号接口；列出不代表已通过实际生成测试。","verified_at":providers::catalog()["verified_at"]}),
        ),
        error => {
            let message = match error {
                Ok(Err(e)) => e.to_string(),
                _ => "查询模型列表超时，请稍后刷新".into(),
            };
            Ok(
                json!({"status":"unavailable","models":[],"presets":fallback(endpoint),"message":message,"verified_at":providers::catalog()["verified_at"]}),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_limits_override_presets_and_unknowns_are_not_guessed() {
        let info = model_info(&json!({"id":"deepseek-v4-flash-vision-exp","context_length":32000,"max_output_tokens":4000}), true).unwrap();
        assert_eq!(info.context_length, Some(32000));
        assert_eq!(info.context_source, "api");
        assert_eq!(info.vision, Some(true));
        let info = model_info(&json!({"id":"glm-5.1"}), true).unwrap();
        assert_eq!(info.context_length, Some(200000));
        assert_eq!(info.context_source, "preset");
        let info = model_info(&json!({"id":"future-model"}), true).unwrap();
        assert!(info.context_length.is_none());
        assert!(info.vision.is_none());
        assert_eq!(info.context_source, "unknown");
        assert!(
            !model_info(&json!({"id":"text-embedding-3-large"}), true)
                .unwrap()
                .task_compatible
        );
    }
}
