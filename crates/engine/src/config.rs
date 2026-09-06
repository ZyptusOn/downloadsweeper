//! 应用配置：从 `config.toml` + 环境变量加载。
//!
//! LLM 的 endpoint / key / 模型 / 价格 / 上下文长度均可在此配置，
//! 也可在运行时通过 UI/CLI 覆盖（运行时覆盖走 `LlmConfig` 的 Clone）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::classify::FewShotExample;
use crate::permission::PermissionConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Private .env values, never serialized or returned to the browser.
    #[serde(skip)]
    pub local_env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub search: SearchConfig,
    /// 默认扫描根（系统下载目录）。CLI 可覆盖。
    #[serde(default = "default_scan_root")]
    pub scan_root: PathBuf,
    #[serde(default)]
    pub permissions: PermissionConfig,
    /// few-shot 分类示例（用户可自定义，引导 LLM 对齐意图）。
    #[serde(default)]
    pub few_shot: Vec<FewShotExample>,
    /// token 预算上限（输入+输出合计），None 表示不限。
    #[serde(default = "default_token_budget", deserialize_with = "deserialize_token_budget")]
    pub token_budget: Option<u64>,
    /// 每个分类批次的工具轮次上限；旧 Agent 兼容使用此值。
    #[serde(default = "default_max_iter")]
    pub max_iterations: usize,
}

fn default_scan_root() -> PathBuf {
    dirs_download()
}

fn default_token_budget() -> Option<u64> {
    Some(500_000)
}

fn deserialize_token_budget<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Budget { Tokens(u64), Named(String) }
    match Option::<Budget>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Budget::Tokens(tokens)) => Ok(Some(tokens)),
        Some(Budget::Named(value)) if value == "unlimited" => Ok(None),
        _ => Err(serde::de::Error::custom("token_budget 必须是非负整数或 unlimited")),
    }
}

/// Finder launches have no stable working directory. Mac frontends share a user
/// data location; explicit environment/command-line paths still support portable use.
pub fn runtime_paths() -> anyhow::Result<(PathBuf, PathBuf)> {
    #[cfg(target_os = "macos")]
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("无法确定用户应用数据目录"))?
        .join("DownloadSweeper");
    #[cfg(not(target_os = "macos"))]
    let base = PathBuf::from(".");
    let data = std::env::var_os("DS_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| base.join(".ds-data"));
    let config = std::env::var_os("DS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| base.join("config.toml"));
    Ok((data, config))
}

fn default_max_iter() -> usize {
    12
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(skip)]
    pub local_env_key: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_search_endpoint")]
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
}
fn default_search_endpoint() -> String {
    "https://api.tavily.com/search".into()
}
impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            local_env_key: None,
            enabled: false,
            endpoint: default_search_endpoint(),
            api_key: String::new(),
        }
    }
}
impl SearchConfig {
    pub fn resolve_key(&self) -> Option<String> {
        std::env::var("DS_SEARCH_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| self.local_env_key.clone().filter(|k| !k.trim().is_empty()))
            .or_else(|| (!self.api_key.trim().is_empty()).then(|| self.api_key.trim().to_owned()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Bounded parallel read-only model requests. Filesystem mutations stay serial.
    #[serde(default = "default_parallel_requests")]
    pub parallel_requests: usize,
    /// auto / chat_completions / responses / anthropic.
    #[serde(default)]
    pub api_format: crate::llm::providers::ApiFormat,
    /// OpenAI 兼容的 chat completions 端点。
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    /// 模型名。
    #[serde(default = "default_model")]
    pub model: String,
    /// 显式绑定的密钥变量名；设定后不会回退到其他服务商的密钥。
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// 旧版本内联 key，网页保存时迁移至 .env；读取接口必须移除此字段。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// 上下文长度上限（token），用于批次容量与请求准入。
    #[serde(default = "default_context_len")]
    pub context_length: u64,
    /// 单次输出上限；推理模型的思考过程也可能占用此额度。
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u64,
    /// 模型请求（含读取正文）的最长等待时间。
    #[serde(default = "default_request_timeout")]
    pub request_timeout_seconds: u64,
    /// 是否启用思考模式（仅部分模型支持，传递给客户端做策略提示）。
    #[serde(default)]
    pub thinking_mode: bool,
    /// 模型是否支持多模态（视觉）。开启后，权限为 image/content_slice 的图片
    /// 会以缩略图形式进入 prompt。
    #[serde(default)]
    pub multimodal: bool,
    #[serde(default = "default_temp")]
    pub temperature: f32,
    /// 预设价格（美元/千 token）。
    #[serde(default)]
    pub pricing: Pricing,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            parallel_requests: default_parallel_requests(),
            api_format: Default::default(),
            endpoint: default_endpoint(),
            model: default_model(),
            api_key_env: None,
            api_key: String::new(),
            context_length: default_context_len(),
            max_output_tokens: default_max_output_tokens(),
            request_timeout_seconds: default_request_timeout(),
            thinking_mode: false,
            multimodal: false,
            temperature: default_temp(),
            pricing: Pricing::default(),
        }
    }
}
fn default_parallel_requests() -> usize {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    #[serde(default = "automatic_pricing")]
    pub mode: String,
    #[serde(default = "usd_currency")]
    pub currency: String,
    /// None chooses the provider's first published currency; never an FX conversion.
    #[serde(default)]
    pub official_currency: Option<String>,
    #[serde(default)]
    pub cached_input_per_1k: Option<f64>,
    #[serde(default)]
    pub cache_write_per_1k: Option<f64>,
    #[serde(default)]
    pub cache_write_1h_per_1k: Option<f64>,
    #[serde(default)]
    pub input_per_1k_usd: f64,
    #[serde(default)]
    pub output_per_1k_usd: f64,
}

impl Default for Pricing {
    fn default() -> Self {
        Self {
            mode: automatic_pricing(),
            currency: usd_currency(),
            official_currency: None,
            cached_input_per_1k: None,
            cache_write_per_1k: None,
            cache_write_1h_per_1k: None,
            input_per_1k_usd: 0.0,
            output_per_1k_usd: 0.0,
        }
    }
}
fn automatic_pricing() -> String {
    "auto".into()
}
fn usd_currency() -> String {
    "USD".into()
}

fn default_endpoint() -> String {
    "https://api.openai.com/v1/chat/completions".into()
}
fn default_model() -> String {
    "gpt-4o-mini".into()
}
fn default_context_len() -> u64 {
    16_384
}
fn default_max_output_tokens() -> u64 {
    16_384
}
fn default_request_timeout() -> u64 {
    600
}
fn default_temp() -> f32 {
    0.2
}

pub fn desktop_root() -> Option<PathBuf> {
    dirs::desktop_dir()
}

fn dirs_download() -> PathBuf {
    std::env::var_os("DS_SCAN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // 使用系统配置的下载目录，兼容重定向目录与非英文用户名。
            dirs::download_dir().unwrap_or_else(|| PathBuf::from("./Downloads"))
        })
}

impl AppConfig {
    /// 从 TOML 字符串解析。
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(s)?)
    }

    /// 从文件加载；文件不存在时返回带合理默认的配置。
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let mut config = if path.exists() {
            let s = std::fs::read_to_string(path)?;
            Self::from_toml(&s)?
        } else {
            Self::default()
        };
        let env_path = path.with_file_name(".env");
        if env_path.exists() {
            let entries = dotenvy::from_path_iter(env_path)
                .map_err(|_| anyhow::anyhow!("无法读取本地 .env"))?;
            for entry in entries {
                let (name, value) =
                    entry.map_err(|_| anyhow::anyhow!("本地 .env 格式无效，请检查引号与换行"))?;
                config.local_env.insert(name, value);
            }
        }
        config.search.local_env_key = config.local_env.get("DS_SEARCH_API_KEY").cloned();
        Ok(config)
    }

    /// 序列化为 TOML 字符串。
    pub fn to_toml(&self) -> anyhow::Result<String> {
        let mut value = toml::Value::try_from(self)?;
        // TOML has no null: distinguish an explicit unlimited choice from a missing default.
        if self.token_budget.is_none() {
            value.as_table_mut().ok_or_else(|| anyhow::anyhow!("配置必须是 TOML 表"))?
                .insert("token_budget".into(), toml::Value::String("unlimited".into()));
        }
        Ok(toml::to_string_pretty(&value)?)
    }

    /// 保存到文件。
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let s = self.to_toml()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, s)?;
        Ok(())
    }

    /// An explicit variable binds a credential to this connection, without falling
    /// through to an unrelated provider's global key. Process env overrides .env.
    pub fn resolve_api_key(&self) -> Option<String> {
        let read = |name: &str| {
            std::env::var(name)
                .ok()
                .or_else(|| self.local_env.get(name).cloned())
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        if let Some(name) = self
            .llm
            .api_key_env
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            return read(name);
        }
        read("DS_API_KEY")
            .or_else(|| read("OPENAI_API_KEY"))
            .or_else(|| {
                (!self.llm.api_key.trim().is_empty()).then(|| self.llm.api_key.trim().to_owned())
            })
    }

    /// Store a newly entered model key in private .env; TOML keeps only its name.
    /// Unique names avoid changing process-wide environment or overwriting other keys.
    pub fn store_model_key(&mut self, path: &std::path::Path, key: &str) -> anyhow::Result<()> {
        use std::io::Write;
        let key = key.trim();
        anyhow::ensure!(
            !key.is_empty()
                && key.len() <= 4096
                && key
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && b != b'\'' && b != b'"' && b != b'\\'),
            "密钥包含不支持的字符，请检查是否复制了引号或换行"
        );
        let name = format!("DS_MODEL_KEY_{}", uuid::Uuid::new_v4().simple());
        let env_path = path.with_file_name(".env");
        anyhow::ensure!(!env_path.is_symlink(), ".env 不能是符号链接");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = options.open(&env_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        writeln!(file, "\n{name}='{key}'")?;
        file.sync_all()?;
        self.local_env.insert(name.clone(), key.to_owned());
        self.llm.api_key_env = Some(name);
        self.llm.api_key.clear();
        Ok(())
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            local_env: Default::default(),
            llm: LlmConfig::default(),
            search: SearchConfig::default(),
            scan_root: default_scan_root(),
            permissions: PermissionConfig {
                rules: PermissionConfig::default_presets(),
                ..PermissionConfig::default()
            },
            few_shot: Vec::new(),
            token_budget: default_token_budget(),
            max_iterations: default_max_iter(),
        }
    }
}
