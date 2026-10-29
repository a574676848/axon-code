//! Axon 配置 schema、默认值与用户配置读写。

use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_domain::{AxonError, AxonResult};
use serde::{Deserialize, Serialize};

#[path = "atomic_write.rs"]
pub mod atomic_write;
#[path = "config_paths.rs"]
mod config_paths;
mod config_persistence;
#[path = "credential_store.rs"]
mod credential_store;
#[path = "schema/defaults.rs"]
mod defaults;
#[path = "schema/keys.rs"]
mod keys;
#[path = "schema/keys_specs.rs"]
mod keys_specs;
#[path = "schema/sections.rs"]
mod sections;
#[path = "validation/validators.rs"]
mod validators;
#[cfg(test)]
pub(crate) use config_paths::default_user_axon_root_path_from_env;
pub use config_paths::{
    AXON_RUNTIME_ROOT_ENV, runtime_axon_log_path, runtime_axon_root_path,
    set_runtime_root_override, user_home_dir,
};
pub(crate) use config_paths::{default_user_axon_root_path, default_user_config_path};
pub use config_persistence::write_with_backup;
pub use credential_store::{
    load_provider_credentials_from_env_file, provider_credential, set_provider_credential,
};

/// 纯函数 Setup 规划与补丁原语。
pub mod setup;

pub use defaults::*;
pub use keys::{
    ConfigKeySpec, ConfigValue, ConfigValueKind, config_key_specs, get_config_value,
    set_config_value,
};
pub use sections::{
    AgentConfig, ChannelBalancerConfig, MemoryConfig, MemoryEmbeddingConfig, RemoteConfig,
    RemoteFeishuWebSocketConfig, RemoteGatewayConfig, RemoteMode, SkillsConfig, TuiConfig,
    VisionConfig,
};
pub use setup::*;

/// OpenAI 默认协议（映射到 Responses API）。
pub const PROVIDER_PROTOCOL_OPENAI: &str = "openai";
/// OpenAI Chat Completions provider 协议。
pub const PROVIDER_PROTOCOL_OPENAI_CHAT: &str = "openai-chatcompletions";
/// Anthropic Messages provider 协议。
pub const PROVIDER_PROTOCOL_ANTHROPIC: &str = "anthropic";
/// 阿里云百炼原生多模态 embedding provider 协议。
pub const PROVIDER_PROTOCOL_DASHSCOPE: &str = "dashscope";
/// 火山方舟多模态 embedding provider 协议。
pub const PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL: &str = "volcengine-ark-multimodal";

/// OpenAI 兼容多模态 embeddings provider 协议（网关多向量返回）。
pub const PROVIDER_PROTOCOL_OPENAI_MULTIMODAL_EMBEDDINGS: &str = "openai-multimodal-embeddings";

/// Provider 请求历史与缓存能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// 是否支持 provider prompt cache。
    pub supports_prompt_cache: bool,
    /// 是否支持显式 cache breakpoint。
    pub supports_explicit_cache_breakpoint: bool,
    /// 是否支持服务端会话状态引用。
    pub supports_server_conversation_state: bool,
    /// 是否要求每条历史 assistant 消息回传 `reasoning_content`。
    pub requires_reasoning_content_on_assistant_messages: bool,
}

impl ProviderCapabilities {
    /// 根据 provider wire 协议声明默认能力。
    #[must_use]
    pub fn for_protocol(protocol: &str) -> Self {
        match protocol {
            PROVIDER_PROTOCOL_OPENAI => Self {
                supports_prompt_cache: true,
                supports_explicit_cache_breakpoint: false,
                supports_server_conversation_state: true,
                requires_reasoning_content_on_assistant_messages: false,
            },
            PROVIDER_PROTOCOL_ANTHROPIC => Self {
                supports_prompt_cache: true,
                supports_explicit_cache_breakpoint: true,
                supports_server_conversation_state: false,
                requires_reasoning_content_on_assistant_messages: false,
            },
            _ => Self::stateless(),
        }
    }

    /// 无 provider 状态和缓存时的能力集合。
    #[must_use]
    pub const fn stateless() -> Self {
        Self {
            supports_prompt_cache: false,
            supports_explicit_cache_breakpoint: false,
            supports_server_conversation_state: false,
            requires_reasoning_content_on_assistant_messages: false,
        }
    }

    /// 应用模型级兼容声明：模型要求回传推理正文时打开对应能力位。
    #[must_use]
    pub fn with_model_reasoning_content_passback(mut self, required: bool) -> Self {
        self.requires_reasoning_content_on_assistant_messages = required;
        self
    }
}
/// Provider 默认 HTTP/SSE 传输。
pub const PROVIDER_WIRE_TRANSPORT_HTTP: &str = "http";
/// Provider WebSocket 传输。
pub const PROVIDER_WIRE_TRANSPORT_WEBSOCKET: &str = "websocket";

/// 渠道默认优先级（再导出 domain 常量，供配置与测试统一引用）。
pub use axon_domain::DEFAULT_CHANNEL_PRIORITY;
/// 渠道默认权重（再导出 domain 常量）。
pub use axon_domain::DEFAULT_CHANNEL_WEIGHT;

/// 配置来源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSourceKind {
    /// 内置默认值。
    BuiltInDefault,
    /// 系统级配置。
    System,
    /// 用户级配置。
    User,
    /// workspace 级配置。
    Workspace,
    /// 环境变量覆盖。
    Environment,
    /// 启动参数覆盖。
    CliArgument,
}

/// 单个配置来源记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLoadEntry {
    /// 来源类型。
    pub source: ConfigSourceKind,
    /// 配置键。
    pub key: &'static str,
    /// 是否包含 secret 原文。
    pub contains_secret_plaintext: bool,
}

/// 配置加载报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigLoadReport {
    /// 按优先级记录的来源。
    pub entries: Vec<ConfigLoadEntry>,
}

impl ConfigLoadReport {
    /// 记录一个普通配置来源。
    pub fn record(&mut self, source: ConfigSourceKind, key: &'static str) {
        self.entries.push(ConfigLoadEntry {
            source,
            key,
            contains_secret_plaintext: false,
        });
    }

    /// 确保报告未包含 secret 原文。
    pub fn ensure_no_secret_plaintext(&self) -> AxonResult<()> {
        if self
            .entries
            .iter()
            .any(|entry| entry.contains_secret_plaintext)
        {
            return Err(AxonError::runtime_invalid_state(
                "ConfigLoadReport 不能记录 secret 原文",
            ));
        }
        Ok(())
    }
}

/// 配置加载结果。
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    /// 合并后的配置。
    pub config: AxonConfig,
    /// 加载报告。
    pub report: ConfigLoadReport,
}

/// 配置加载器。
#[derive(Debug, Default, Clone, Copy)]
pub struct ConfigLoader;

impl ConfigLoader {
    /// 加载内置默认配置。
    pub fn load_defaults() -> AxonResult<LoadedConfig> {
        let mut config = AxonConfig::default();
        normalize_setup_defaults(&mut config);
        normalize_derived_config(&mut config);
        let mut report = ConfigLoadReport::default();
        record_default_keys(&mut report);
        config.validate()?;
        report.ensure_no_secret_plaintext()?;
        Ok(LoadedConfig { config, report })
    }

    /// 返回当前平台的用户级 Axon 根目录。
    pub fn default_user_axon_root_path() -> AxonResult<PathBuf> {
        default_user_axon_root_path()
    }

    /// 返回当前平台的用户配置文件路径。
    pub fn default_user_config_path() -> AxonResult<PathBuf> {
        default_user_config_path()
    }

    /// 从指定路径加载用户配置；文件不存在时返回内置默认配置。
    pub fn load_from_path(path: &Path) -> AxonResult<LoadedConfig> {
        let mut report = ConfigLoadReport::default();
        record_default_keys(&mut report);
        let mut config = if path.exists() {
            let text = fs::read_to_string(path).map_err(|e| {
                let path = path.display();
                let msg = format!("配置文件读取失败: {e:?}, 路径: {path}");
                eprintln!("ERROR: {msg}");
                AxonError::Config(axon_domain::ConfigError::ConfigFileNotFound(
                    "无法读取配置文件".to_owned(),
                ))
            })?;
            let config: AxonConfig = toml::from_str(&text).map_err(|e| {
                let path = path.display();
                let msg = format!("配置文件 TOML 解析失败: {e:?}, 路径: {path}");
                eprintln!("ERROR: {msg}");
                AxonError::Config(axon_domain::ConfigError::ParseError(
                    "配置文件 TOML 无效".to_owned(),
                ))
            })?;
            record_default_keys_as_user(&mut report);
            config
        } else {
            AxonConfig::default()
        };
        normalize_setup_defaults(&mut config);
        normalize_derived_config(&mut config);
        config.validate().map_err(|e| {
            let providers = config.providers.len();
            let models = config.models.len();
            let path = path.display();
            let msg = format!(
                "配置校验失败: {e:?}, providers: {providers}, models: {models}, 路径: {path}"
            );
            eprintln!("ERROR: {msg}");
            e
        })?;
        report.ensure_no_secret_plaintext()?;
        Ok(LoadedConfig { config, report })
    }

    /// 从 TOML 字符串加载并校验配置（纯内存，不落盘）。
    pub fn load_from_str(text: &str) -> AxonResult<LoadedConfig> {
        let mut report = ConfigLoadReport::default();
        record_default_keys(&mut report);
        let mut config = if text.trim().is_empty() {
            AxonConfig::default()
        } else {
            let config: AxonConfig = toml::from_str(text).map_err(|e| {
                AxonError::Config(axon_domain::ConfigError::ParseError(format!(
                    "配置文件 TOML 无效: {e}"
                )))
            })?;
            record_default_keys_as_user(&mut report);
            config
        };
        normalize_setup_defaults(&mut config);
        normalize_derived_config(&mut config);
        config.validate()?;
        report.ensure_no_secret_plaintext()?;
        Ok(LoadedConfig { config, report })
    }
}

fn normalize_derived_config(config: &mut AxonConfig) {
    if config.agent.log_level == LOG_LEVEL_WARNING_ALIAS {
        config.agent.log_level = DEFAULT_AGENT_LOG_LEVEL.to_owned();
    }
    config.vision = VisionConfig::default();
    config.memory.embedding.dimension = resolved_memory_embedding_dimension(config);
}

fn normalize_setup_defaults(config: &mut AxonConfig) {
    for provider in &mut config.providers {
        if provider.base_url.trim().is_empty() {
            let default_url = match provider.protocol.as_str() {
                PROVIDER_PROTOCOL_OPENAI_CHAT => Some(DEFAULT_BASE_URL_OPENAI),
                other => default_base_url_for_protocol(other),
            };
            if let Some(url) = default_url {
                provider.base_url = url.to_owned();
            }
        }
    }
    for model in &mut config.models {
        if !model.supports_embedding && model.context_window == 0 {
            model.context_window = DEFAULT_MODEL_CONTEXT_WINDOW;
        }
    }
}

fn resolved_memory_embedding_dimension(config: &AxonConfig) -> usize {
    if !config.memory.embedding.enabled {
        return 0;
    }
    config
        .models
        .iter()
        .find(|model| {
            model.supports_embedding
                && (model.name == config.memory.embedding.model
                    || model.id == config.memory.embedding.model)
        })
        .map_or(0, |model| model.embedding_dimension)
}

/// Axon 总配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AxonConfig {
    /// Agent 配置。
    pub agent: AgentConfig,
    /// TUI 交互配置。
    pub tui: TuiConfig,
    /// 渠道负载均衡配置。
    pub channel_balancer: ChannelBalancerConfig,
    /// 远控配置。
    pub remote: RemoteConfig,
    /// 视觉配置。
    #[serde(skip)]
    pub vision: VisionConfig,
    /// 记忆配置。
    pub memory: MemoryConfig,
    /// Skills 启用状态配置。
    pub skills: SkillsConfig,
    /// Provider 目录。
    pub providers: Vec<ProviderConfig>,
    /// 模型目录。
    pub models: Vec<ModelConfig>,
}

/// 模型 provider 配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// provider 名称。
    pub name: String,
    /// 是否参与运行时模型路由。
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,
    /// 协议类型：openai 或 anthropic。
    pub protocol: String,
    /// API base URL。
    pub base_url: String,
    /// API key 环境变量名。
    pub api_key_env: Option<String>,
    /// 直接配置的 API key；优先级高于 api_key_env。
    pub api_key: Option<String>,
    /// provider streaming 底层传输链路：默认 http，可配置 websocket。
    pub wire_transport: String,
    /// WebSocket URL；为空时 runtime 从 base_url 推导。
    pub websocket_url: Option<String>,
    /// 是否以 zstd 压缩 HTTP 请求体；仅应对明确支持该能力的 provider 启用。
    pub enable_request_compression: bool,
    /// 渠道优先级，数值越大越优先；同模型 id 下高优先级组整体先于低优先级组。
    pub priority: i32,
    /// 渠道权重，同优先级组内加权负载均衡的相对配额；运行时按 max(weight, 1) 兜底。
    pub weight: u32,
}

impl ProviderConfig {
    /// 创建 provider 配置。
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        protocol: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let protocol = protocol.into();
        Self {
            name: name.into(),
            enabled: true,
            protocol,
            base_url: base_url.into(),
            api_key_env: None,
            api_key: None,
            wire_transport: PROVIDER_WIRE_TRANSPORT_HTTP.to_owned(),
            websocket_url: None,
            enable_request_compression: false,
            priority: DEFAULT_CHANNEL_PRIORITY,
            weight: DEFAULT_CHANNEL_WEIGHT,
        }
    }

    /// 根据 provider 协议推导运行时能力。
    #[must_use]
    pub fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::for_protocol(&self.protocol)
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            protocol: PROVIDER_PROTOCOL_OPENAI.to_owned(),
            base_url: String::new(),
            api_key_env: None,
            api_key: None,
            wire_transport: PROVIDER_WIRE_TRANSPORT_HTTP.to_owned(),
            websocket_url: None,
            enable_request_compression: false,
            priority: DEFAULT_CHANNEL_PRIORITY,
            weight: DEFAULT_CHANNEL_WEIGHT,
        }
    }
}

/// Provider 默认参与运行时模型路由。
#[must_use]
pub const fn default_provider_enabled() -> bool {
    true
}

/// 模型配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// 模型名称（负载均衡路由键）；
    /// 同一 name 可对应多个 provider 渠道，运行时按 priority/weight 选择。
    pub name: String,
    /// 模型 ID（实际发送给上游渠道的参数）。
    pub id: String,
    /// provider 名称。
    pub provider: String,
    /// 上下文窗口。
    pub context_window: u64,
    /// 模型最大输出 token；省略时使用统一默认值。
    pub max_output_tokens: Option<u64>,
    /// 是否支持视觉。
    #[serde(default = "default_model_supports_vision")]
    pub supports_vision: bool,
    /// 是否支持 embedding。
    pub supports_embedding: bool,
    /// embedding 维度；仅 supports_embedding=true 时有效。
    pub embedding_dimension: usize,
    /// 是否要求每条历史 assistant 消息回传 `reasoning_content`。
    ///
    /// 仅对思考模式下强制回传推理正文的 OpenAI-compatible 网关开启；
    /// 未保存推理正文的历史消息会补空串，避免上游以 400 拒绝整条请求。
    pub requires_reasoning_content_on_assistant_messages: bool,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            id: String::new(),
            provider: String::new(),
            context_window: DEFAULT_MODEL_CONTEXT_WINDOW,
            max_output_tokens: Some(DEFAULT_MODEL_MAX_OUTPUT_TOKENS),
            supports_vision: true,
            supports_embedding: false,
            embedding_dimension: 0,
            requires_reasoning_content_on_assistant_messages: false,
        }
    }
}

impl ModelConfig {
    /// 模型自身声明的历史推理回传兼容要求。
    #[must_use]
    pub const fn requires_reasoning_content_passback(&self) -> bool {
        self.requires_reasoning_content_on_assistant_messages
    }
}

/// 模型默认按支持视觉处理，避免未声明能力时丢弃用户图片附件。
#[must_use]
pub fn default_model_supports_vision() -> bool {
    true
}

/// 序列化为 TOML 文本。
pub fn to_toml_string(config: &AxonConfig) -> AxonResult<String> {
    config.validate()?;
    let mut value = to_toml_value(config)?;
    let default = to_toml_value(&AxonConfig::default())?;
    prune_default_values(&mut value, &default);
    prune_provider_catalog_defaults(&mut value)?;
    prune_catalog_defaults(&mut value, "models", &ModelConfig::default())?;
    prune_provider_protocol_defaults(&mut value);
    remove_empty_tables(&mut value);
    toml::to_string_pretty(&value).map_err(|_| AxonError::runtime_failure("配置序列化失败"))
}

fn prune_provider_protocol_defaults(root: &mut toml::Value) {
    let Some(items) = root
        .get_mut("providers")
        .and_then(toml::Value::as_array_mut)
    else {
        return;
    };
    for item in items {
        let Some(table) = item.as_table_mut() else {
            continue;
        };
        let protocol = table
            .get("protocol")
            .and_then(toml::Value::as_str)
            .unwrap_or(PROVIDER_PROTOCOL_OPENAI);
        let default_url = match protocol {
            PROVIDER_PROTOCOL_OPENAI_CHAT => Some(DEFAULT_BASE_URL_OPENAI),
            other => default_base_url_for_protocol(other),
        };
        if let (Some(default_url), Some(base_url)) = (
            default_url,
            table.get("base_url").and_then(toml::Value::as_str),
        ) && base_url == default_url
        {
            table.remove("base_url");
        }
    }
}

fn to_toml_value<T: Serialize>(value: &T) -> AxonResult<toml::Value> {
    toml::Value::try_from(value).map_err(|_| AxonError::runtime_failure("配置序列化失败"))
}

fn prune_catalog_defaults<T: Serialize>(
    root: &mut toml::Value,
    key: &str,
    default_entry: &T,
) -> AxonResult<()> {
    let default = to_toml_value(default_entry)?;
    let Some(items) = root.get_mut(key).and_then(toml::Value::as_array_mut) else {
        return Ok(());
    };
    for item in items {
        prune_default_values(item, &default);
    }
    Ok(())
}

fn prune_provider_catalog_defaults(root: &mut toml::Value) -> AxonResult<()> {
    let default = to_toml_value(&ProviderConfig::default())?;
    let Some(items) = root
        .get_mut("providers")
        .and_then(toml::Value::as_array_mut)
    else {
        return Ok(());
    };
    for item in items {
        prune_default_values(item, &default);
    }
    Ok(())
}

fn prune_default_values(value: &mut toml::Value, default: &toml::Value) {
    let (Some(table), Some(default_table)) = (value.as_table_mut(), default.as_table()) else {
        return;
    };
    let keys = table.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let Some(current) = table.get_mut(&key) else {
            continue;
        };
        if let Some(default_value) = default_table.get(&key) {
            prune_default_values(current, default_value);
            if current == default_value || current.as_table().is_some_and(toml::map::Map::is_empty)
            {
                table.remove(&key);
            }
        }
    }
}

fn remove_empty_tables(value: &mut toml::Value) -> bool {
    let Some(table) = value.as_table_mut() else {
        return false;
    };
    let keys = table.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if table.get_mut(&key).is_some_and(remove_empty_tables) {
            table.remove(&key);
        }
    }
    table.is_empty()
}

/// 添加 provider。
pub fn add_provider(config: &mut AxonConfig, provider: ProviderConfig) -> AxonResult<()> {
    if config
        .providers
        .iter()
        .any(|existing| existing.name == provider.name)
    {
        return Err(AxonError::config_invalid("provider.name 不能重复"));
    }
    config.providers.push(provider);
    config.validate()
}

/// 移除 provider。
pub fn remove_provider(config: &mut AxonConfig, name: &str) -> AxonResult<()> {
    if config.models.iter().any(|model| model.provider == name) {
        return Err(AxonError::config_invalid("provider 仍被模型引用，不能移除"));
    }
    let original_len = config.providers.len();
    config.providers.retain(|provider| provider.name != name);
    if config.providers.len() == original_len {
        return Err(AxonError::config_invalid("provider 不存在"));
    }
    config.validate()
}

impl AxonConfig {
    /// 查找参与运行时模型路由的 Provider。
    #[must_use]
    pub fn enabled_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers
            .iter()
            .find(|provider| provider.enabled && provider.name == name)
    }

    /// 判断模型是否属于启用的 Provider。
    #[must_use]
    pub fn model_is_enabled(&self, model: &ModelConfig) -> bool {
        self.enabled_provider(&model.provider).is_some()
    }

    /// 校验安全不变量。
    pub fn validate(&self) -> AxonResult<()> {
        validators::validate_config(self)
    }
}

fn record_default_keys(report: &mut ConfigLoadReport) {
    report.record(ConfigSourceKind::BuiltInDefault, "agent");
    report.record(ConfigSourceKind::BuiltInDefault, "tui");
    report.record(ConfigSourceKind::BuiltInDefault, "channel_balancer");
    report.record(ConfigSourceKind::BuiltInDefault, "remote");
    report.record(ConfigSourceKind::BuiltInDefault, "vision");
    report.record(ConfigSourceKind::BuiltInDefault, "memory");
}

fn record_default_keys_as_user(report: &mut ConfigLoadReport) {
    report.record(ConfigSourceKind::User, "agent");
    report.record(ConfigSourceKind::User, "tui");
    report.record(ConfigSourceKind::User, "channel_balancer");
    report.record(ConfigSourceKind::User, "remote");
    report.record(ConfigSourceKind::User, "vision");
    report.record(ConfigSourceKind::User, "memory");
}

/// 运行态数据目录名。
pub const AXON_RUNTIME_DATA_DIR: &str = "data";
/// 外部事件 provider 内容资产目录名（`~/.axon/events`）。
pub const AXON_EVENT_PROVIDERS_DIR: &str = "events";
/// 运行态日志目录名。
pub const AXON_RUNTIME_LOG_DIR: &str = "logs";

#[cfg(test)]
#[path = "tests/provider_catalog_tests.rs"]
mod provider_catalog_tests;
#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
