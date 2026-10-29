use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_AFFINITY_TTL_MS, DEFAULT_AGENT_AUTO_COMPACTION_THRESHOLD,
    DEFAULT_AGENT_IDLE_COMPACTION_DEBOUNCE_MS, DEFAULT_AGENT_LOG_LEVEL,
    DEFAULT_AGENT_MAX_SUBAGENT_ACTIVE_TURNS, DEFAULT_AGENT_MAX_SUBAGENT_DEPTH,
    DEFAULT_AGENT_MAX_SUBAGENT_THREADS, DEFAULT_AGENT_REASONING_EFFORT,
    DEFAULT_BREAKER_COOLDOWN_MS, DEFAULT_BREAKER_FAILURE_THRESHOLD,
    DEFAULT_CHANNEL_METRICS_PERSISTENCE_ENABLED, DEFAULT_GATEWAY_API_TOKEN_ENV,
    DEFAULT_GATEWAY_HTTP_BIND, DEFAULT_GATEWAY_WEBSOCKET_BIND,
    HIGH_THROUGHPUT_REMOTE_MAX_EVENTS_PER_CURSOR, HIGH_THROUGHPUT_REMOTE_MAX_PAYLOAD_BYTES,
    STANDARD_REMOTE_MAX_EVENTS_PER_CURSOR, STANDARD_REMOTE_MAX_PAYLOAD_BYTES,
};

/// Agent 配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// 默认主模型。
    pub default_model: String,
    /// 快速委托模型。
    pub fast_model: String,
    /// 主 Agent 默认推理强度，仅作用于 default_model。
    pub reasoning_effort: String,
    /// 每个 run session 允许的最大子代理线程数。
    pub max_subagent_threads: usize,
    /// 每个 run session 允许同时运行的子代理 turn 数。
    pub max_subagent_active_turns: usize,
    /// 子代理最大嵌套深度，root 深度为 0。
    pub max_subagent_depth: u32,
    /// 自动压缩触发阈值（context_window 的比例，0.0-1.0）。
    pub auto_compaction_threshold: f64,
    /// 闲暇压缩 debounce 窗口，单位毫秒。
    pub idle_compaction_debounce_ms: u64,
    /// 日志级别：debug、info、warning、error。
    pub log_level: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// TUI 配置段；三段式 TUI 不暴露终端原生历史兼容配置。
pub struct TuiConfig {}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            default_model: String::new(),
            fast_model: String::new(),
            reasoning_effort: DEFAULT_AGENT_REASONING_EFFORT.to_owned(),
            max_subagent_threads: DEFAULT_AGENT_MAX_SUBAGENT_THREADS,
            max_subagent_active_turns: DEFAULT_AGENT_MAX_SUBAGENT_ACTIVE_TURNS,
            max_subagent_depth: DEFAULT_AGENT_MAX_SUBAGENT_DEPTH,
            auto_compaction_threshold: DEFAULT_AGENT_AUTO_COMPACTION_THRESHOLD,
            idle_compaction_debounce_ms: DEFAULT_AGENT_IDLE_COMPACTION_DEBOUNCE_MS,
            log_level: DEFAULT_AGENT_LOG_LEVEL.to_owned(),
        }
    }
}

/// Skills 启用状态配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// 被用户禁用的 skill 名称。
    pub disabled: BTreeSet<String>,
}

/// 远控运行模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteMode {
    /// 常规远控模式。
    #[default]
    Standard,
    /// 高频事件流模式。
    HighThroughput,
}

impl RemoteMode {
    /// 返回配置文件中的稳定字符串。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::HighThroughput => "high-throughput",
        }
    }

    /// 从配置字符串解析远控模式。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "standard" => Some(Self::Standard),
            "high-throughput" => Some(Self::HighThroughput),
            _ => None,
        }
    }
}

/// 远控配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    /// 统一 Gateway 配置；远控目标态始终启用。
    pub gateway: RemoteGatewayConfig,
    /// 远控运行模式。
    pub mode: RemoteMode,
    /// Feishu WebSocket adapter 配置。
    pub feishu_websocket: RemoteFeishuWebSocketConfig,
}

/// 统一 Gateway 配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteGatewayConfig {
    /// API token 对应的环境变量名；token 原文只存在于用户级 env 文件。
    pub api_token_env: String,
    /// HTTP Webhook 监听地址。
    pub http_bind: String,
    /// 通用 WebSocket 监听地址。
    pub websocket_bind: String,
}

impl Default for RemoteGatewayConfig {
    fn default() -> Self {
        Self {
            api_token_env: DEFAULT_GATEWAY_API_TOKEN_ENV.to_owned(),
            http_bind: DEFAULT_GATEWAY_HTTP_BIND.to_owned(),
            websocket_bind: DEFAULT_GATEWAY_WEBSOCKET_BIND.to_owned(),
        }
    }
}

impl RemoteConfig {
    /// 当前模式下的 cursor 最大事件数。
    #[must_use]
    pub const fn max_events_per_cursor(&self) -> usize {
        match self.mode {
            RemoteMode::Standard => STANDARD_REMOTE_MAX_EVENTS_PER_CURSOR,
            RemoteMode::HighThroughput => HIGH_THROUGHPUT_REMOTE_MAX_EVENTS_PER_CURSOR,
        }
    }

    /// 当前模式下的 payload 最大字节数。
    #[must_use]
    pub const fn max_payload_bytes(&self) -> usize {
        match self.mode {
            RemoteMode::Standard => STANDARD_REMOTE_MAX_PAYLOAD_BYTES,
            RemoteMode::HighThroughput => HIGH_THROUGHPUT_REMOTE_MAX_PAYLOAD_BYTES,
        }
    }
}

/// 远控 Feishu WebSocket adapter 配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteFeishuWebSocketConfig {
    /// 是否启用 Feishu WebSocket adapter。
    pub enabled: bool,
    /// Feishu app_id。
    pub app_id: String,
    /// Feishu app_secret 环境变量名。
    pub app_secret_env: String,
    /// Feishu OpenAPI 基址；为空时使用官方默认地址。
    pub api_base_url: String,
    /// 可选 WebSocket URL；为空时使用官方默认地址。
    pub websocket_url: String,
    /// 是否允许发送远控通知。
    pub enable_notification_sending: bool,
    /// 消息通知 chat_id；为空时不向飞书投递后台完成通知。
    pub notification_chat_id: String,
    /// 允许访问远控的 Feishu open_id 列表。
    pub allowed_sender_open_ids: Vec<String>,
    /// 禁止访问远控的 Feishu open_id 列表，优先级高于 allowlist。
    pub blocked_sender_open_ids: Vec<String>,
    /// 群聊消息是否要求显式 @ 机器人。
    pub require_mention: bool,
    /// 按 chat_id 生效的群聊治理策略。
    pub chat_policies: Vec<RemoteFeishuChatPolicyConfig>,
}

/// Feishu 单群治理策略配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteFeishuChatPolicyConfig {
    /// Feishu chat_id。
    pub chat_id: String,
    /// 当前 chat 的 sender allowlist；非空时覆盖全局 allowlist。
    pub allowed_sender_open_ids: Vec<String>,
    /// 当前 chat 的 sender blacklist；与全局 blacklist 叠加生效。
    pub blocked_sender_open_ids: Vec<String>,
    /// 当前 chat 是否要求 @ 机器人；为空时沿用全局配置。
    pub require_mention: Option<bool>,
}

/// 视觉配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisionConfig {
    /// 图片进入视觉模型前是否要求本机确认。
    #[serde(skip)]
    pub require_local_confirmation: bool,
    /// 支持的 MIME。
    #[serde(skip)]
    pub supported_mime_types: Vec<String>,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            require_local_confirmation: false,
            supported_mime_types: vec![
                "image/png".to_owned(),
                "image/jpeg".to_owned(),
                "image/webp".to_owned(),
            ],
        }
    }
}

/// Memory 配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// embedding provider 引用配置。
    pub embedding: MemoryEmbeddingConfig,
}

/// 渠道负载均衡配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelBalancerConfig {
    /// 连续失败几次熔断。
    pub failure_threshold: u32,
    /// 熔断冷却窗口。
    pub cooldown_ms: u64,
    /// 内存亲和 TTL。
    pub affinity_ttl_ms: u64,
    /// 是否将渠道遥测样本持久化到运行态 data 目录。
    pub telemetry_persistence_enabled: bool,
}

impl Default for ChannelBalancerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: DEFAULT_BREAKER_FAILURE_THRESHOLD,
            cooldown_ms: DEFAULT_BREAKER_COOLDOWN_MS,
            affinity_ttl_ms: DEFAULT_AFFINITY_TTL_MS,
            telemetry_persistence_enabled: DEFAULT_CHANNEL_METRICS_PERSISTENCE_ENABLED,
        }
    }
}

/// Memory embedding 配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryEmbeddingConfig {
    /// 是否启用 embedding。
    pub enabled: bool,
    /// embedding 模型名称，引用顶层 model catalog 的 name 或 id。
    pub model: String,
    /// embedding 维度。
    #[serde(skip)]
    pub dimension: usize,
}
