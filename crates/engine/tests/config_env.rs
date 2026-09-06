//! Subprocess isolation keeps environment changes out of other tests.
use ds_engine::config::AppConfig;
use std::{fs, process::Command};

#[test]
fn token_budget_defaults_and_explicit_choices_survive_saving() {
    assert_eq!(AppConfig::default().token_budget, Some(500_000));
    assert_eq!(AppConfig::from_toml("").unwrap().token_budget, Some(500_000));
    assert_eq!(AppConfig::from_toml(include_str!("../../../config.example.toml")).unwrap().token_budget, Some(500_000));
    for budget in [None, Some(0), Some(50_000), Some(500_000)] {
        let mut config = AppConfig::default();
        config.token_budget = budget;
        assert_eq!(AppConfig::from_toml(&config.to_toml().unwrap()).unwrap().token_budget, budget);
        let restored: AppConfig = serde_json::from_value(serde_json::to_value(config).unwrap()).unwrap();
        assert_eq!(restored.token_budget, budget);
    }
}

#[test]
fn environment_workflow() {
    let directory = std::env::temp_dir().join(format!("ds-config-env-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let config_path = directory.join("config.toml");
    fs::write(&config_path, include_str!("../../../config.example.toml")).unwrap();
    for scenario in ["empty", "dotenv", "process", "fallback", "custom", "legacy"] {
        fs::write(
            directory.join(".env"),
            if scenario == "dotenv" || scenario == "process" {
                "DS_API_KEY=synthetic-dotenv-value\n"
            } else {
                include_str!("../../../.env.example")
            },
        )
        .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "environment_workflow_child", "--nocapture"])
            .env("DS_CONFIG_TEST_SCENARIO", scenario)
            .env("DS_CONFIG_TEST_PATH", &config_path)
            .env_remove("DS_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("DS_CUSTOM_TEST_KEY");
        if scenario == "process" {
            command.env("DS_API_KEY", "synthetic-process-value");
        }
        if scenario == "fallback" {
            command.env("OPENAI_API_KEY", "synthetic-fallback-value");
        }
        if scenario == "custom" {
            command.env("DS_CUSTOM_TEST_KEY", "synthetic-custom-value");
        }
        assert!(command.status().unwrap().success(), "scenario: {scenario}");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn environment_workflow_child() {
    let Ok(scenario) = std::env::var("DS_CONFIG_TEST_SCENARIO") else {
        return;
    };
    let path = std::path::PathBuf::from(std::env::var_os("DS_CONFIG_TEST_PATH").unwrap());
    let mut config = AppConfig::load(&path).unwrap();
    assert!(config.llm.api_key.is_empty());
    if scenario == "fallback" || scenario == "legacy" {
        config.llm.api_key_env = None;
    }
    if scenario == "custom" {
        config.llm.api_key_env = Some("DS_CUSTOM_TEST_KEY".into());
    }
    if scenario == "legacy" {
        config.llm.api_key = String::from("synthetic-legacy-value");
    }
    let expected = match scenario.as_str() {
        "empty" => None,
        "dotenv" => Some("synthetic-dotenv-value"),
        "process" => Some("synthetic-process-value"),
        "fallback" => Some("synthetic-fallback-value"),
        "custom" => Some("synthetic-custom-value"),
        "legacy" => Some("synthetic-legacy-value"),
        _ => panic!("unexpected scenario"),
    };
    assert_eq!(config.resolve_api_key().as_deref(), expected);
    if scenario != "legacy" {
        let saved = path.with_file_name("saved.toml");
        config.save(&saved).unwrap();
        let text = fs::read_to_string(saved).unwrap();
        assert!(!text.contains("synthetic-"));
        assert!(AppConfig::from_toml(&text).unwrap().llm.api_key.is_empty());
    }
}

#[test]
fn saved_model_key_survives_reload_without_entering_toml() {
    let directory = std::env::temp_dir().join(format!("ds-private-env-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("config.toml");
    fs::write(
        directory.join(".env"),
        "# preserve this comment\nUNRELATED_SETTING=keep\n",
    )
    .unwrap();
    let mut config = AppConfig::default();
    let key = "synthetic-private-persistence";
    config.store_model_key(&path, key).unwrap();
    config.save(&path).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains(key));
    assert!(!serde_json::to_string(&config).unwrap().contains(key));
    assert!(fs::read_to_string(directory.join(".env"))
        .unwrap()
        .contains("UNRELATED_SETTING=keep"));
    let loaded = AppConfig::load(&path).unwrap();
    assert_eq!(loaded.resolve_api_key().as_deref(), Some(key));
    let mut unbound = loaded.clone();
    unbound.llm.api_key_env = Some(format!("DS_MISSING_{}", uuid::Uuid::new_v4().simple()));
    assert!(unbound.resolve_api_key().is_none());
    assert!(config.store_model_key(&path, "invalid\nheader").is_err());
    fs::remove_dir_all(directory).unwrap();
}
