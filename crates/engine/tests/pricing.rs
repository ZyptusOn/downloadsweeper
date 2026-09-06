use chrono::{TimeZone, Utc};
use ds_engine::{
    config::LlmConfig,
    cost::Usage,
    pricing::{self, calculate},
    workflow::CallRecord,
};
use serde_json::{json, Value};
fn cfg(endpoint: &str, model: &str) -> LlmConfig {
    LlmConfig {
        endpoint: endpoint.into(),
        model: model.into(),
        ..LlmConfig::default()
    }
}
fn usage(input: u64, output: u64, cached: u64) -> Usage {
    Usage {
        prompt_tokens: input,
        completion_tokens: output,
        cached_input_tokens: cached,
        cache_details_known: true,
        ..Usage::default()
    }
}
fn at(day: u32, hour: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, 0, 0).unwrap()
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-10, "{a} != {b}");
}
#[test]
fn official_cache_math_peak_boundaries_weekend_and_currency() {
    let mut c = cfg("https://api.deepseek.com/v1", "deepseek-v4-flash");
    c.pricing.official_currency = Some("USD".into());
    let u = usage(1_000_000, 100_000, 800_000);
    let off = calculate(&c, &u, at(4, 0), &Value::Null);
    close(off.amount.unwrap(), 0.1156);
    close(
        calculate(&c, &u, at(4, 1), &Value::Null).amount.unwrap(),
        0.2312,
    );
    close(
        calculate(&c, &u, at(4, 4), &Value::Null).amount.unwrap(),
        0.1156,
    );
    close(
        calculate(&c, &u, at(4, 6), &Value::Null).amount.unwrap(),
        0.2312,
    );
    close(
        calculate(&c, &u, at(4, 10), &Value::Null).amount.unwrap(),
        0.1156,
    );
    close(
        calculate(&c, &u, at(5, 6), &Value::Null).amount.unwrap(),
        0.1156,
    );
    c.pricing.official_currency = Some("CNY".into());
    let cn = calculate(&c, &u, at(5, 6), &Value::Null);
    close(cn.amount.unwrap(), 0.79);
    assert!(cn.usd().is_none());
}
#[test]
fn tiers_are_whole_request_not_only_excess_tokens() {
    let c = cfg("https://open.bigmodel.cn/api/paas/v4", "glm-5.1");
    close(
        calculate(&c, &usage(31999, 1000, 0), at(5, 0), &Value::Null)
            .amount
            .unwrap(),
        0.215994,
    );
    close(
        calculate(&c, &usage(32000, 1000, 0), at(5, 0), &Value::Null)
            .amount
            .unwrap(),
        0.284,
    );
    let c = cfg("https://api.openai.com/v1", "gpt-5.6-sol");
    assert_eq!(
        calculate(&c, &usage(272000, 1, 0), at(5, 0), &Value::Null)
            .rates
            .unwrap()
            .input,
        4.
    );
    assert_eq!(
        calculate(&c, &usage(272001, 1, 0), at(5, 0), &Value::Null)
            .rates
            .unwrap()
            .input,
        8.
    );
}
#[test]
fn anthropic_writes_are_disjoint_from_normal_and_cached_input() {
    let c = cfg("https://api.anthropic.com/v1", "claude-fable-5-1");
    let u = Usage {
        cache_write_tokens: 300,
        cache_write_1h_tokens: 100,
        ..usage(1000, 100, 500)
    };
    close(
        calculate(&c, &u, at(5, 0), &Value::Null).amount.unwrap(),
        0.011625,
    );
    let parsed = pricing::cache_usage(
        &json!({"prompt_cache_hit_tokens":500,"prompt_tokens_details":{"cached_tokens":500}}),
        false,
    );
    assert_eq!(parsed.cached_input_tokens, 500);
    let parsed = pricing::cache_usage(
        &json!({"input_tokens_details":{"cached_tokens":500,"cache_write_tokens":300}}),
        true,
    );
    assert_eq!(parsed.cache_write_tokens, 300);
}
#[test]
fn manual_overrides_zero_and_history_survives_config_changes() {
    let mut c = cfg("https://proxy.example/v1", "gpt-6-astra");
    c.pricing.mode = "manual".into();
    c.pricing.currency = "CNY".into();
    c.pricing.input_per_1k_usd = 0.002;
    c.pricing.output_per_1k_usd = 0.008;
    c.pricing.cached_input_per_1k = Some(0.);
    let mut official_host = c.clone();
    official_host.endpoint = "https://api.openai.com/v1".into();
    assert!(pricing::preview(&official_host)["bands"].is_null());
    let b = calculate(&c, &usage(1000, 100, 500), at(5, 0), &Value::Null);
    close(b.amount.unwrap(), 0.0018);
    assert!(b.usd().is_none());
    let saved = serde_json::to_string(&b).unwrap();
    c.pricing.input_per_1k_usd = 500.;
    assert_eq!(serde_json::to_string(&b).unwrap(), saved);
    c.pricing.input_per_1k_usd = 0.;
    c.pricing.output_per_1k_usd = 0.;
    assert_eq!(
        calculate(&c, &usage(1, 1, 0), at(5, 0), &Value::Null).amount,
        Some(0.)
    );
    c.pricing.cache_write_per_1k = Some(-1.);
    assert!(pricing::validate(&c.pricing).is_err());
    c.pricing.cache_write_per_1k = Some(f64::INFINITY);
    assert!(pricing::validate(&c.pricing).is_err());
}
#[test]
fn unknown_prices_and_invalid_usage_are_never_reported_free() {
    for (endpoint, model) in [
        ("https://proxy.example/v1", "gpt-5"),
        ("https://api.openai.com.evil.example/v1", "gpt-5"),
        ("https://api.openai.com/v1", "gpt-99"),
        ("https://open.bigmodel.cn/api/coding/paas/v4", "glm-5.1"),
        ("http://api.openai.com/v1", "gpt-5"),
    ] {
        let b = calculate(
            &cfg(endpoint, model),
            &usage(100, 10, 0),
            at(5, 0),
            &Value::Null,
        );
        assert!(b.amount.is_none());
        assert!(b.usd().is_none());
    }
    let c = cfg("https://api.openai.com/v1", "gpt-5");
    assert!(calculate(&c, &usage(100, 10, 101), at(5, 0), &Value::Null)
        .amount
        .is_none());
    assert!(calculate(
        &c,
        &usage(100, 10, 0),
        at(5, 0),
        &json!({"service_tier":"fast"})
    )
    .amount
    .is_none());
    let b = calculate(
        &c,
        &Usage {
            cache_details_known: false,
            ..usage(100, 10, 0)
        },
        at(5, 0),
        &Value::Null,
    );
    assert_eq!(b.status, "estimated");
}
#[test]
fn catalogs_and_currency_totals_are_consistent() {
    let mut keys = std::collections::HashSet::new();
    for c in &pricing::catalog().cards {
        assert!(c.source.starts_with("https://"));
        assert_eq!(c.bands[0].min_input, 0);
        for m in &c.models {
            assert!(keys.insert((&c.host, m.to_lowercase(), &c.currency)));
        }
        for r in &c.bands {
            assert!(r.input >= 0. && r.output >= 0.);
        }
    }
    let legacy:CallRecord=serde_json::from_value(json!({"id":"old","timestamp":"old","purpose":"test","model":"test","usage":{"prompt_tokens":1,"completion_tokens":1},"cost_usd":1.})).unwrap();
    let mut cn = legacy.clone();
    cn.billing = Some(calculate(
        &cfg("https://open.bigmodel.cn/api/paas/v4", "glm-5.2"),
        &usage(1_000_000, 0, 0),
        at(5, 0),
        &Value::Null,
    ));
    cn.cost_usd = None;
    let mut unknown = legacy.clone();
    unknown.cost_usd = None;
    let t = pricing::totals(&[legacy, cn, unknown]);
    assert_eq!(t.currencies["USD"], 1.);
    assert_eq!(t.currencies["CNY"], 8.);
    assert_eq!(t.unknown, 1);
    assert_eq!(t.legacy, 1);
}
