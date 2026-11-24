use std::fmt;

use axon_domain::{AxonError, AxonResult};
use serde::Serialize;

use crate::{AxonConfig, keys_specs::CONFIG_KEY_SPECS};

/// 配置值类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValueKind {
    /// 布尔值。
    Bool,
    /// 无符号整数。
    Unsigned,
    /// 字符串。
    String,
    /// 字符串列表。
    StringList,
}

/// 配置键定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConfigKeySpec {
    /// 点分配置键。
    pub key: &'static str,
    /// 值类型。
    pub kind: ConfigValueKind,
    /// 是否允许通过 `axon config set` 修改。
    pub editable: bool,
    /// 中文说明。
    pub description: &'static str,
    /// setup 模块名。
    pub setup_module: &'static str,
    /// setup 默认值说明。
    pub default_value: &'static str,
    /// 可选值说明；开放字符串使用空切片。
    pub options: &'static [&'static str],
    /// 联动说明。
    pub depends_on: &'static [&'static str],
}

/// 配置值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ConfigValue {
    /// 布尔值。
    Bool(bool),
    /// 无符号整数。
    Unsigned(u64),
    /// 字符串。
    String(String),
    /// 字符串列表。
    StringList(Vec<String>),
}

impl fmt::Display for ConfigValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(formatter, "{value}"),
            Self::Unsigned(value) => write!(formatter, "{value}"),
            Self::String(value) => write!(formatter, "{value}"),
            Self::StringList(values) => write!(formatter, "{}", values.join(",")),
        }
    }
}

/// 返回所有配置键定义。
#[must_use]
pub fn config_key_specs() -> &'static [ConfigKeySpec] {
    CONFIG_KEY_SPECS
}

/// 读取点分配置值。
#[must_use]
pub fn get_config_value(config: &AxonConfig, key: &str) -> Option<ConfigValue> {
    get_non_terminal_config_value(config, key)
}

fn get_non_terminal_config_value(config: &AxonConfig, key: &str) -> Option<ConfigValue> {
    match key {
        "agent.default_model" => Some(ConfigValue::String(config.agent.default_model.clone())),
        "agent.fast_model" => Some(ConfigValue::String(config.agent.fast_model.clone())),
        "agent.reasoning_effort" => {
            Some(ConfigValue::String(config.agent.reasoning_effort.clone()))
        }
        "agent.log_level" => Some(ConfigValue::String(config.agent.log_level.clone())),
        "agent.max_subagent_threads" => Some(ConfigValue::Unsigned(
            u64::try_from(config.agent.max_subagent_threads).unwrap_or(u64::MAX),
        )),
        "agent.max_subagent_active_turns" => Some(ConfigValue::Unsigned(
            u64::try_from(config.agent.max_subagent_active_turns).unwrap_or(u64::MAX),
        )),
        "channel_balancer.failure_threshold" => Some(ConfigValue::Unsigned(u64::from(
            config.channel_balancer.failure_threshold,
        ))),
        "channel_balancer.cooldown_ms" => {
            Some(ConfigValue::Unsigned(config.channel_balancer.cooldown_ms))
        }
        "channel_balancer.affinity_ttl_ms" => Some(ConfigValue::Unsigned(
            config.channel_balancer.affinity_ttl_ms,
        )),
        "channel_balancer.telemetry_persistence_enabled" => Some(ConfigValue::Bool(
            config.channel_balancer.telemetry_persistence_enabled,
        )),
        "memory.embedding.enabled" => Some(ConfigValue::Bool(config.memory.embedding.enabled)),
        "memory.embedding.model" => {
            Some(ConfigValue::String(config.memory.embedding.model.clone()))
        }
        "remote.gateway.api_token_env"
        | "remote.gateway.http_bind"
        | "remote.gateway.websocket_bind"
        | "remote.gateway.auth_keys"
        | "remote.gateway.legacy_token_enabled" => get_remote_config_value(config, key),
        _ => None,
    }
}

fn get_remote_config_value(config: &AxonConfig, key: &str) -> Option<ConfigValue> {
    match key {
        "remote.gateway.api_token_env" => Some(ConfigValue::String(
            config.remote.gateway.api_token_env.clone(),
        )),
        "remote.gateway.http_bind" => {
            Some(ConfigValue::String(config.remote.gateway.http_bind.clone()))
        }
        "remote.gateway.websocket_bind" => Some(ConfigValue::String(
            config.remote.gateway.websocket_bind.clone(),
        )),
        "remote.gateway.auth_keys" => {
            Some(ConfigValue::String(config.remote.gateway.auth_keys.clone()))
        }
        "remote.gateway.legacy_token_enabled" => Some(ConfigValue::Bool(
            config.remote.gateway.legacy_token_enabled,
        )),
        _ => None,
    }
}

/// 设置点分配置值。
pub fn set_config_value(
    config: &mut AxonConfig,
    key: &str,
    raw_value: &str,
) -> AxonResult<ConfigValue> {
    ensure_editable_key(key)?;
    let mut candidate = config.clone();
    set_core_config_value(&mut candidate, key, raw_value)?;
    candidate.validate()?;
    let value = get_config_value(&candidate, key).ok_or(AxonError::config_invalid("未知配置键"))?;
    *config = candidate;
    Ok(value)
}

fn set_core_config_value(config: &mut AxonConfig, key: &str, raw_value: &str) -> AxonResult<()> {
    match key {
        "agent.default_model" => {
            config.agent.default_model = parse_string(raw_value)?;
        }
        "agent.fast_model" => {
            config.agent.fast_model = parse_string(raw_value)?;
        }
        "agent.reasoning_effort" => {
            config.agent.reasoning_effort = parse_string(raw_value)?;
        }
        "agent.log_level" => {
            config.agent.log_level = parse_log_level(raw_value)?;
        }
        "agent.max_subagent_threads" => {
            config.agent.max_subagent_threads = parse_usize(raw_value)?;
        }
        "agent.max_subagent_active_turns" => {
            config.agent.max_subagent_active_turns = parse_usize(raw_value)?;
        }
        "channel_balancer.failure_threshold" => {
            config.channel_balancer.failure_threshold = parse_u32(raw_value)?;
        }
        "channel_balancer.cooldown_ms" => {
            config.channel_balancer.cooldown_ms = parse_u64(raw_value)?;
        }
        "channel_balancer.affinity_ttl_ms" => {
            config.channel_balancer.affinity_ttl_ms = parse_u64(raw_value)?;
        }
        "channel_balancer.telemetry_persistence_enabled" => {
            config.channel_balancer.telemetry_persistence_enabled = parse_bool(raw_value)?;
        }
        "memory.embedding.enabled" => {
            config.memory.embedding.enabled = parse_bool(raw_value)?;
        }
        "memory.embedding.model" => {
            config.memory.embedding.model = parse_string(raw_value)?;
        }
        key if key.starts_with("remote.") => set_remote_config_value(config, key, raw_value)?,
        _ => return Err(AxonError::config_invalid("未知配置键")),
    }
    Ok(())
}

fn set_remote_config_value(config: &mut AxonConfig, key: &str, raw_value: &str) -> AxonResult<()> {
    match key {
        "remote.gateway.api_token_env" => {
            config.remote.gateway.api_token_env = parse_string(raw_value)?;
        }
        "remote.gateway.http_bind" => {
            config.remote.gateway.http_bind = parse_string(raw_value)?;
        }
        "remote.gateway.websocket_bind" => {
            config.remote.gateway.websocket_bind = parse_string(raw_value)?;
        }
        "remote.gateway.auth_keys" => {
            // 空串是合法取值：表示走运行时根下的默认路径，因此不走 parse_string 的非空校验。
            config.remote.gateway.auth_keys = raw_value.trim().to_owned();
        }
        "remote.gateway.legacy_token_enabled" => {
            config.remote.gateway.legacy_token_enabled = parse_bool(raw_value)?;
        }
        _ => return Err(AxonError::config_invalid("未知配置键")),
    }
    Ok(())
}

fn ensure_editable_key(key: &str) -> AxonResult<()> {
    match config_key_specs().iter().find(|spec| spec.key == key) {
        Some(spec) if spec.editable => Ok(()),
        Some(_) => Err(AxonError::config_invalid("配置键不可通过 CLI 修改")),
        None => Err(AxonError::config_invalid("未知配置键")),
    }
}

fn parse_string(raw_value: &str) -> AxonResult<String> {
    let value = raw_value.trim().to_owned();
    if value.is_empty() {
        Err(AxonError::config_invalid("配置值不能为空"))
    } else {
        Ok(value)
    }
}

fn parse_usize(raw_value: &str) -> AxonResult<usize> {
    raw_value
        .trim()
        .parse::<usize>()
        .map_err(|_| AxonError::config_invalid("无符号整数配置值无效"))
}

fn parse_u32(raw_value: &str) -> AxonResult<u32> {
    raw_value
        .trim()
        .parse::<u32>()
        .map_err(|_| AxonError::config_invalid("无符号整数配置值无效"))
}

fn parse_u64(raw_value: &str) -> AxonResult<u64> {
    raw_value
        .trim()
        .parse::<u64>()
        .map_err(|_| AxonError::config_invalid("无符号整数配置值无效"))
}

fn parse_log_level(raw_value: &str) -> AxonResult<String> {
    let value = raw_value.trim().to_ascii_lowercase();
    if value == crate::LOG_LEVEL_WARNING_ALIAS {
        Ok(crate::DEFAULT_AGENT_LOG_LEVEL.to_owned())
    } else if crate::LOG_LEVEL_VALUES.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(AxonError::config_invalid(
            "日志级别必须为 trace、debug、info、warn 或 error（兼容 warning）",
        ))
    }
}

fn parse_bool(raw_value: &str) -> AxonResult<bool> {
    match raw_value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(AxonError::config_invalid("布尔配置值必须为 true 或 false")),
    }
}
