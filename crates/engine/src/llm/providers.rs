//! Provider routing, request dialects and dated, replaceable capability presets.
use anyhow::ensure;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    #[default]
    Auto,
    ChatCompletions,
    Responses,
    Anthropic,
}

pub fn catalog() -> &'static Value {
    static CATALOG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(include_str!("model_catalog.json")).unwrap())
}

pub fn preset(model: &str) -> Option<Value> {
    let id = model.rsplit('/').next().unwrap_or(model).to_lowercase();
    catalog()["models"]
        .as_array()?
        .iter()
        .find(|entry| {
            let name = entry["id"].as_str().unwrap().to_lowercase();
            id == name
                || id.strip_prefix(&(name + "-")).is_some_and(|suffix| {
                    suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_digit())
                })
        })
        .cloned()
}

pub fn validate_url(endpoint: &str) -> anyhow::Result<reqwest::Url> {
    let url =
        reqwest::Url::parse(endpoint.trim()).map_err(|_| anyhow::anyhow!("API 地址格式无效"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "API 地址必须使用 http 或 https"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "API 地址不能含用户名、密码、查询参数或片段；请在密钥栏填写凭据"
    );
    Ok(url)
}

pub fn provider(endpoint: &str) -> &'static str {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return "custom";
    };
    match url.host_str().unwrap_or("") {
        "api.deepseek.com" => "deepseek",
        "open.bigmodel.cn" | "api.z.ai" => "glm",
        "api.xiaomimimo.com" => "mimo",
        "api.longcat.chat" => "longcat",
        "api.hunyuan.cloud.tencent.com"
        | "tokenhub.tencentmaas.com"
        | "tokenhub-intl.tencentmaas.com"
        | "tokenhub-intl.tencentcloudmaas.com" => "hunyuan",
        "api.openai.com" => "openai",
        "api.anthropic.com" => "anthropic",
        _ => "custom",
    }
}

/// Keep provider-only options out of other compatible services.
pub fn image_url(endpoint: &str, image: &super::ImageData) -> Value {
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned));
    let url = if host.as_deref() == Some("open.bigmodel.cn") {
        image.data_base64.clone()
    } else {
        format!("data:{};base64,{}", image.mime, image.data_base64)
    };
    let mut value = json!({"url":url});
    if host.as_deref() == Some("api.openai.com") {
        value["detail"] = json!(if image.high_detail { "high" } else { "low" });
    }
    value
}

pub fn api_format(endpoint: &str, model: &str, selected: ApiFormat) -> ApiFormat {
    if selected != ApiFormat::Auto {
        return selected;
    }
    let path = reqwest::Url::parse(endpoint)
        .ok()
        .map(|u| u.path().trim_end_matches('/').to_owned())
        .unwrap_or_default();
    if path.ends_with("/messages") || provider(endpoint) == "anthropic" {
        ApiFormat::Anthropic
    } else if path.ends_with("/responses") || (provider(endpoint) == "openai" && modern_gpt(model))
    {
        ApiFormat::Responses
    } else {
        ApiFormat::ChatCompletions
    }
}

pub fn base_url(endpoint: &str) -> anyhow::Result<reqwest::Url> {
    let mut url = validate_url(endpoint)?;
    let path = url.path().trim_end_matches('/');
    let base = ["/chat/completions", "/responses", "/messages", "/models"]
        .iter()
        .find_map(|suffix| path.strip_suffix(suffix))
        .unwrap_or(path)
        .to_owned();
    let base = if base.is_empty() {
        match provider(endpoint) {
            "openai" | "anthropic" | "mimo" | "hunyuan" => "/v1".into(),
            "longcat" => "/openai/v1".into(),
            "glm" => "/api/paas/v4".into(),
            _ => base,
        }
    } else {
        base
    };
    url.set_path(&base);
    Ok(url)
}

pub fn endpoint_for(endpoint: &str, suffix: &str) -> anyhow::Result<String> {
    let mut url = base_url(endpoint)?;
    url.set_path(&format!("{}{suffix}", url.path().trim_end_matches('/')));
    Ok(url.to_string())
}

pub fn same_connection(a: &str, b: &str) -> bool {
    match (base_url(a), base_url(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn authorize(
    request: reqwest::RequestBuilder,
    endpoint: &str,
    format: ApiFormat,
    key: &str,
) -> reqwest::RequestBuilder {
    let mut request = if format == ApiFormat::Anthropic {
        request.header("anthropic-version", "2023-06-01")
    } else {
        request
    };
    if !key.is_empty() {
        request = if provider(endpoint) == "mimo" {
            request.header("api-key", key)
        } else if format == ApiFormat::Anthropic {
            request.header("x-api-key", key)
        } else {
            request.bearer_auth(key)
        };
    }
    request
}

pub fn model_family(model: &str) -> &'static str {
    let m = model.rsplit('/').next().unwrap_or(model).to_lowercase();
    if m.starts_with("deepseek") {
        "deepseek"
    } else if m.starts_with("glm-") {
        "glm"
    } else if m.starts_with("mimo-") {
        "mimo"
    } else if m.starts_with("longcat-") {
        "longcat"
    } else if m.starts_with("hunyuan-") || m.starts_with("hy3") || m.starts_with("hy4") {
        "hunyuan"
    } else if m.starts_with("claude-") {
        "anthropic"
    } else if modern_gpt(&m) || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        "openai_reasoning"
    } else {
        "custom"
    }
}

pub fn modern_gpt(model: &str) -> bool {
    model
        .strip_prefix("gpt-")
        .and_then(|v| v.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| n >= 5)
}

pub fn openai_effort(model: &str, thinking: bool) -> &'static str {
    if thinking {
        return "high";
    }
    let m = model.rsplit('/').next().unwrap_or(model);
    if m == "gpt-5"
        || m.starts_with("gpt-5-mini")
        || m.starts_with("gpt-5-nano")
        || m.starts_with("gpt-5-2025")
    {
        "minimal"
    } else if m.starts_with("gpt-5.1")
        || m.starts_with("gpt-5.2")
        || m.starts_with("gpt-5.4")
        || m.starts_with("gpt-5.5")
        || m.starts_with("gpt-5.6")
    {
        "none"
    } else {
        "low"
    }
}

pub fn tune_chat(body: &mut Value, model: &str, thinking: bool) {
    let family = model_family(model);
    match family {
        "deepseek" | "glm" | "mimo" | "longcat" => {
            body["thinking"] = json!({"type": if thinking {"enabled"} else {"disabled"}});
            if thinking {
                body.as_object_mut().unwrap().remove("temperature");
                if family == "deepseek" || (family == "glm" && model.contains("5.2")) {
                    body["reasoning_effort"] = json!("high");
                }
            }
        }
        "hunyuan" => {
            body["thinking"] = json!({"type":if thinking {"enabled"} else {"disabled"}});
        }
        "openai_reasoning" => {
            body.as_object_mut().unwrap().remove("temperature");
            body["reasoning_effort"] = json!(openai_effort(model, thinking));
        }
        // Unknown gateways/models get only standard fields; do not guess extra parameters.
        _ => {}
    }
    if family == "openai_reasoning" || family == "mimo" {
        if let Some(max) = body.as_object_mut().unwrap().remove("max_tokens") {
            body["max_completion_tokens"] = max;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_use_exact_hosts_and_preserve_custom_paths() {
        assert_eq!(
            endpoint_for("https://api.anthropic.com", "/models").unwrap(),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            endpoint_for("https://api.longcat.chat", "/models").unwrap(),
            "https://api.longcat.chat/openai/v1/models"
        );
        assert_eq!(
            endpoint_for(
                "https://proxy.example/custom/v4/chat/completions",
                "/models"
            )
            .unwrap(),
            "https://proxy.example/custom/v4/models"
        );
        assert_eq!(
            provider("https://api.deepseek.com.attacker.example/v1"),
            "custom"
        );
        assert!(!same_connection(
            "https://api.deepseek.com",
            "https://api.deepseek.com.attacker.example"
        ));
        assert!(validate_url("https://user:password@example.org/v1").is_err());
        assert!(validate_url("https://example.org/v1?token=hidden").is_err());
        assert_eq!(
            api_format("https://api.openai.com/v1", "gpt-6-astra", ApiFormat::Auto),
            ApiFormat::Responses
        );
        assert_eq!(
            api_format("https://proxy.example/v1", "claude-opus-5", ApiFormat::Auto),
            ApiFormat::ChatCompletions
        );
    }

    #[test]
    fn model_dialects_do_not_send_incompatible_sampling_or_thinking_fields() {
        for name in [
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "deepseek-v4-flash-vision-exp",
            "glm-5",
            "glm-5.1",
            "glm-5.2",
            "mimo-v2.5",
            "LongCat-2.0",
            "hy3",
        ] {
            let mut body = json!({"temperature":0.2,"max_tokens":16000});
            tune_chat(&mut body, name, false);
            assert_eq!(body["thinking"]["type"], "disabled", "{name}");
        }
        for name in ["gpt-5", "gpt-5.1", "gpt-5.6-sol", "gpt-6-astra"] {
            let mut body = json!({"temperature":0.2,"max_tokens":16000});
            tune_chat(&mut body, name, false);
            assert!(body.get("temperature").is_none());
            assert!(body.get("max_tokens").is_none());
            assert_eq!(body["max_completion_tokens"], 16000);
        }
        assert_eq!(openai_effort("gpt-5", false), "minimal");
        assert_eq!(openai_effort("gpt-6-astra", false), "low");
        let mut unknown = json!({"temperature":0.2});
        tune_chat(&mut unknown, "unverified-model", true);
        assert!(unknown.get("reasoning_effort").is_none());
        assert!(preset("gpt-9-future").is_none());
    }

    #[test]
    fn provider_auth_headers_are_specific() {
        let client = reqwest::Client::new();
        let request = authorize(
            client.get("https://api.xiaomimimo.com/v1/models"),
            "https://api.xiaomimimo.com/v1",
            ApiFormat::ChatCompletions,
            "synthetic-key",
        )
        .build()
        .unwrap();
        assert!(request.headers().contains_key("api-key"));
        assert!(!request.headers().contains_key("authorization"));
        let request = authorize(
            client.get("https://api.anthropic.com/v1/models"),
            "https://api.anthropic.com/v1",
            ApiFormat::Anthropic,
            "synthetic-key",
        )
        .build()
        .unwrap();
        assert!(request.headers().contains_key("x-api-key"));
        assert!(request.headers().contains_key("anthropic-version"));
    }
}
