mod support;
use serde_json::{json, Value};
use support::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_protocols_secret_persistence_and_immutable_prices() {
    let s = Server::new("deepseek-v4-flash").await;
    let key = ["synthetic-", "connection-test-credential"].concat();
    let endpoint = format!("{}/v1", s.mock.url);
    let path = s.dir.path().join("config.toml");
    s.request("/api/models", json!({"endpoint":endpoint}), 403, false)
        .await;
    let before = std::fs::read(&path).unwrap();
    let found = s
        .request(
            "/api/models",
            json!({"endpoint":endpoint,"api_format":"auto","api_key":key,"use_saved_key":false}),
            200,
            true,
        )
        .await;
    assert_eq!(found["status"], "ok");
    assert!(!found.to_string().contains(&key));
    let models = array(&found["models"]);
    let find = |id: &str| models.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(find("deepseek-v4-flash")["context_length"], 96000);
    assert_eq!(find("deepseek-v4-flash")["context_source"], "api");
    assert_eq!(find("glm-5.2")["context_source"], "preset");
    assert_eq!(find("deepseek-v4-flash-vision-exp")["vision"], true);
    assert!(find("future-unverified")["context_length"].is_null());
    assert_eq!(find("text-embedding-3-large")["task_compatible"], false);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!s.dir.path().join(".env").exists());
    assert_eq!(
        s.mock.wire().last().unwrap()["headers"]["authorization"],
        format!("Bearer {key}")
    );
    for mode in ["denied", "missing", "redirect", "echo"] {
        let r=s.request("/api/models",json!({"endpoint":format!("{}/{mode}",s.mock.url),"api_format":"auto","api_key":key}),200,true).await;
        assert_eq!(r["status"], "unavailable", "{mode}: {r}");
        assert!(array(&r["models"]).is_empty());
        assert!(!r.to_string().contains(&key));
    }
    assert!(!s
        .mock
        .wire()
        .iter()
        .any(|r| text(&r["path"]).starts_with("/leak/")));
    let native=s.request("/api/models",json!({"endpoint":format!("{}/native/v1",s.mock.url),"api_format":"anthropic","api_key":key}),200,true).await;
    assert_eq!(array(&native["models"]).len(), 2);
    let r = s.mock.wire().pop().unwrap();
    assert_eq!(r["headers"]["x-api-key"], key);
    assert_eq!(r["headers"]["anthropic-version"], "2023-06-01");
    assert!(text(&r["path"]).contains("after_id="));
    let mut cfg = s.config().await;
    cfg["llm"]["api_key"] = json!(key);
    let saved = s.post("config", &Value::Null, json!({"config":cfg})).await;
    assert!(!std::fs::read_to_string(&path).unwrap().contains(&key));
    assert!(s.dir.path().join(".env").is_file());
    assert_eq!(saved["has_api_key"], true);
    assert!(saved["llm"].get("api_key").is_none());
    let found = s
        .request(
            "/api/models",
            json!({"endpoint":endpoint,"api_format":"auto","api_key":"","use_saved_key":true}),
            200,
            true,
        )
        .await;
    assert_eq!(found["status"], "ok");
    let count = s.mock.wire().len();
    s.request("/api/models",json!({"endpoint":format!("{}/different/v1",s.mock.url),"api_format":"auto","api_key":"","use_saved_key":true}),400,true).await;
    assert_eq!(s.mock.wire().len(), count);
    let mut t = s
        .post(
            "create",
            &Value::Null,
            json!({"root":s.root("files"),"mode":"organize"}),
        )
        .await;
    for (format, model) in [
        ("chat_completions", "deepseek-v4-flash"),
        ("chat_completions", "glm-5.2"),
        ("chat_completions", "mimo-v2.5"),
        ("chat_completions", "LongCat-2.0"),
        ("chat_completions", "hy3"),
        ("responses", "gpt-6-astra"),
        ("anthropic", "claude-opus-4-8"),
    ] {
        s.configure(json!({"model":model,"api_format":format,"thinking_mode":true}))
            .await;
        t = s.run("test_connection", &t, json!({})).await;
        let r = s.mock.wire().pop().unwrap();
        let b = &r["body"];
        assert_eq!(b["model"], model);
        match format {
            "responses" => {
                assert!(text(&r["path"]).ends_with("/responses"));
                assert_eq!(b["store"], false);
                assert!(b.get("temperature").is_none() && b.get("max_output_tokens").is_some());
            }
            "anthropic" => {
                assert!(text(&r["path"]).ends_with("/messages"));
                assert_eq!(r["headers"]["x-api-key"], key);
                assert_eq!(b["thinking"]["type"], "adaptive");
            }
            _ => {
                assert_eq!(b["thinking"]["type"], "enabled");
                if model.starts_with("mimo") {
                    assert!(
                        b.get("max_completion_tokens").is_some() && b.get("max_tokens").is_none()
                    );
                }
            }
        }
    }
    let count = s.mock.wire().len();
    let q=s.request("/api/pricing-preview",json!({"endpoint":"https://api.deepseek.com","model":"deepseek-v4-flash","pricing":{"official_currency":"USD"}}),200,true).await;
    assert_eq!(q["status"], "official");
    assert_eq!(q["currency"], "USD");
    assert!([0.007, 0.014].contains(&q["rates"]["cached"].as_f64().unwrap()));
    assert_eq!(q["verified_at"], "2026-09-05");
    assert!(text(&q["source"]).starts_with("https://api-docs.deepseek.com/"));
    let q = s
        .request(
            "/api/pricing-preview",
            json!({"endpoint":"https://proxy.example/v1","model":"gpt-5"}),
            200,
            true,
        )
        .await;
    assert_eq!(q["status"], "unknown");
    assert!(q["amount"].is_null());
    s.request("/api/pricing-preview",json!({"endpoint":"https://api.openai.com","model":"gpt-5","pricing":{"mode":"manual","cached_input_per_1k":-1}}),400,true).await;
    assert_eq!(s.mock.wire().len(), count);
    s.configure(json!({"api_format":"chat_completions","model":"mimo-v2.5","pricing":{"mode":"manual","currency":"CNY","input_per_1k_usd":0.002,"output_per_1k_usd":0.008}})).await;
    t = s.run("test_connection", &t, json!({})).await;
    let recorded = array(&t["calls"]).last().unwrap().clone();
    assert_eq!(recorded["billing"]["status"], "manual");
    assert_eq!(recorded["billing"]["currency"], "CNY");
    assert!((recorded["billing"]["amount"].as_f64().unwrap() - 0.000036).abs() < 1e-12);
    assert!(recorded["cost_usd"].is_null());
    let mut cfg = s.config().await;
    cfg["llm"]["pricing"]["input_per_1k_usd"] = json!(900);
    s.post("config", &Value::Null, json!({"config":cfg})).await;
    assert_eq!(
        array(&s.current(&t).await["calls"]).last().unwrap(),
        &recorded
    );
    let summaries = s.get("/api/tasks").await;
    let summary = array(&summaries)
        .iter()
        .find(|v| v["id"] == t["id"])
        .unwrap();
    assert!((summary["costs"]["currencies"]["CNY"].as_f64().unwrap() - 0.000036).abs() < 1e-12);
    assert!(number(&summary["costs"]["unknown"]) >= 3);
}
