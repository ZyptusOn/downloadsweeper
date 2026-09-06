//! Versioned official list prices; no model-name-only guesses for third-party endpoints.
use crate::{
    config::{LlmConfig, Pricing},
    cost::Usage,
    workflow::CallRecord,
};
use anyhow::{ensure, Result};
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rates {
    pub min_input: u64,
    pub input: f64,
    pub output: f64,
    pub cached: Option<f64>,
    pub write: Option<f64>,
    pub write_1h: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub models: Vec<String>,
    pub host: String,
    pub currency: String,
    pub source: String,
    pub bands: Vec<Rates>,
    pub peak: bool,
    pub note: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    pub verified_at: String,
    pub cards: Vec<Card>,
}
pub fn catalog() -> &'static Catalog {
    static VALUE: OnceLock<Catalog> = OnceLock::new();
    VALUE.get_or_init(|| {
        serde_json::from_str(include_str!("pricing_catalog.json")).expect("validated price catalog")
    })
}
pub fn validate(p: &Pricing) -> Result<()> {
    ensure!(
        matches!(p.mode.as_str(), "auto" | "manual"),
        "计费模式应为 auto 或 manual"
    );
    ensure!(
        matches!(p.currency.as_str(), "USD" | "CNY"),
        "手动价格币种应为 USD 或 CNY"
    );
    ensure!(
        p.official_currency
            .as_deref()
            .is_none_or(|v| matches!(v, "USD" | "CNY")),
        "官方价格币种应为 USD 或 CNY"
    );
    for v in [
        Some(p.input_per_1k_usd),
        Some(p.output_per_1k_usd),
        p.cached_input_per_1k,
        p.cache_write_per_1k,
        p.cache_write_1h_per_1k,
    ]
    .into_iter()
    .flatten()
    {
        ensure!(
            v.is_finite() && (0.0..=1_000_000.0).contains(&v),
            "价格必须是有限非负数，且每千 token 不超过 1000000"
        );
    }
    Ok(())
}
pub fn official(cfg: &LlmConfig) -> Option<&'static Card> {
    let url = crate::llm::providers::validate_url(&cfg.endpoint).ok()?;
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return None;
    }
    // Coding/subscription endpoints are not pay-as-you-go even on an official host.
    if url.path().contains("coding") || url.path().contains("tokenplan") {
        return None;
    }
    catalog().cards.iter().find(|c| {
        Some(c.host.as_str()) == url.host_str()
            && c.models.iter().any(|m| m.eq_ignore_ascii_case(&cfg.model))
            && cfg
                .pricing
                .official_currency
                .as_ref()
                .is_none_or(|v| v == &c.currency)
    })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Charge {
    pub status: String,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub source: String,
    pub verified_at: Option<String>,
    pub request_at: String,
    pub rates: Option<Rates>,
    pub note: String,
    #[serde(default)]
    pub usage_details: Value,
}
impl Charge {
    pub fn usd(&self) -> Option<f64> {
        if self.currency.as_deref() == Some("USD") {
            self.amount
        } else {
            None
        }
    }
}
pub fn calculate(cfg: &LlmConfig, usage: &Usage, at: DateTime<Utc>, metadata: &Value) -> Charge {
    let mut charge = Charge {
        status: "unknown".into(),
        amount: None,
        currency: None,
        source: String::new(),
        verified_at: None,
        request_at: at.to_rfc3339(),
        rates: None,
        note: String::new(),
        usage_details: metadata.clone(),
    };
    if let Err(e) = validate(&cfg.pricing) {
        charge.note = e.to_string();
        return charge;
    }
    let mut rates = if cfg.pricing.mode == "manual" {
        let p = &cfg.pricing;
        charge.status = "manual".into();
        charge.currency = Some(p.currency.clone());
        charge.source = "manual".into();
        charge.note = "用户自定义单价；未填写的缓存单价沿用普通输入价。".into();
        Rates {
            min_input: 0,
            input: p.input_per_1k_usd * 1000.,
            output: p.output_per_1k_usd * 1000.,
            cached: p.cached_input_per_1k.map(|p| p * 1000.),
            write: p.cache_write_per_1k.map(|p| p * 1000.),
            write_1h: p.cache_write_1h_per_1k.map(|p| p * 1000.),
        }
    } else {
        let Some(card) = official(cfg) else {
            charge.note = "未匹配已核实的服务地址、模型和结算币种；可切换手动单价。".into();
            return charge;
        };
        charge.status = "official".into();
        charge.currency = Some(card.currency.clone());
        charge.source = card.source.clone();
        charge.verified_at = Some(catalog().verified_at.clone());
        charge.note = card.note.clone();
        let mut r = card
            .bands
            .iter()
            .rev()
            .find(|r| usage.prompt_tokens >= r.min_input)
            .unwrap()
            .clone();
        if card.peak
            && at.weekday().num_days_from_monday() < 5
            && ((1..4).contains(&at.hour()) || (6..10).contains(&at.hour()))
        {
            r.input *= 2.;
            r.output *= 2.;
            r.cached = r.cached.map(|p| p * 2.);
            charge.note.push_str(" 本次使用高峰价。");
        }
        r
    };
    let cache = usage
        .cached_input_tokens
        .checked_add(usage.cache_write_tokens);
    if cache.is_none_or(|n| n > usage.prompt_tokens)
        || usage.cache_write_1h_tokens > usage.cache_write_tokens
    {
        charge.status = "unknown".into();
        charge.note = "缓存明细与输入总量不一致，费用未计算；原始 usage 已保留。".into();
        return charge;
    }
    if cfg.pricing.mode == "auto" {
        let tier = metadata["service_tier"].as_str().unwrap_or("default");
        if !matches!(tier, "default" | "standard" | "auto") {
            charge.status = "unknown".into();
            charge.note = format!(
                "响应使用 {tier} 服务档位，当前官方价格表仅适用标准在线请求；请使用手动价。"
            );
            return charge;
        }
        if (usage.cache_write_tokens > 0 && rates.write.is_none())
            || (usage.cache_write_1h_tokens > 0 && rates.write_1h.is_none())
        {
            charge.status = "unknown".into();
            charge.note = "存在未核实价格的缓存写入，费用未计算。".into();
            return charge;
        }
        if !usage.cache_details_known && usage.prompt_tokens > 0 {
            charge.status = "estimated".into();
            charge
                .note
                .push_str(" 服务未提供缓存明细，暂按普通输入价估算。");
        }
    }
    rates.cached = Some(rates.cached.unwrap_or(rates.input));
    rates.write = Some(rates.write.unwrap_or(rates.input));
    rates.write_1h = Some(rates.write_1h.unwrap_or(rates.input));
    let normal = usage.prompt_tokens - cache.unwrap();
    charge.amount = Some(
        (normal as f64 * rates.input
            + usage.cached_input_tokens as f64 * rates.cached.unwrap()
            + (usage.cache_write_tokens - usage.cache_write_1h_tokens) as f64
                * rates.write.unwrap()
            + usage.cache_write_1h_tokens as f64 * rates.write_1h.unwrap()
            + usage.completion_tokens as f64 * rates.output)
            / 1_000_000.,
    );
    charge.rates = Some(rates);
    charge
}
pub fn preview(cfg: &LlmConfig) -> Value {
    let mut v = serde_json::to_value(calculate(
        cfg,
        &Usage {
            cache_details_known: true,
            ..Usage::default()
        },
        Utc::now(),
        &Value::Null,
    ))
    .unwrap();
    v["bands"] = if cfg.pricing.mode == "manual" {
        Value::Null
    } else {
        official(cfg).map_or(Value::Null, |c| json!(c.bands))
    };
    v
}
#[derive(Default, Serialize)]
pub struct Totals {
    pub currencies: BTreeMap<String, f64>,
    pub unknown: usize,
    pub estimated: usize,
    pub legacy: usize,
}
pub fn totals(calls: &[CallRecord]) -> Totals {
    let mut t = Totals::default();
    for c in calls {
        if let Some(b) = &c.billing {
            if let (Some(amount), Some(currency)) = (b.amount, &b.currency) {
                *t.currencies.entry(currency.clone()).or_default() += amount;
                if b.status == "estimated" {
                    t.estimated += 1;
                }
            } else {
                t.unknown += 1;
            }
        } else if let Some(amount) = c.cost_usd.filter(|n| *n > 0.) {
            *t.currencies.entry("USD".into()).or_default() += amount;
            t.legacy += 1;
        } else {
            t.unknown += 1;
        }
    }
    t
}

/// Normalize provider counters without charging cached input twice. Missing details stay explicit.
pub fn cache_usage(value: &Value, responses: bool) -> Usage {
    let details = &value[if responses {
        "input_tokens_details"
    } else {
        "prompt_tokens_details"
    }];
    let cached = value["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| details["cached_tokens"].as_u64());
    Usage {
        cached_input_tokens: cached.unwrap_or(0),
        cache_write_tokens: details["cache_write_tokens"].as_u64().unwrap_or(0),
        cache_details_known: cached.is_some(),
        ..Usage::default()
    }
}
