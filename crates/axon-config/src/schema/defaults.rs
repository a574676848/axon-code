/// 默认断路器连续失败阈值。
pub const DEFAULT_BREAKER_FAILURE_THRESHOLD: u32 = 3;
/// 默认断路器冷却窗口，单位毫秒。
pub const DEFAULT_BREAKER_COOLDOWN_MS: u64 = 30_000;
/// 默认内存亲和 TTL，单位毫秒。
pub const DEFAULT_AFFINITY_TTL_MS: u64 = 300_000;
/// 主 Agent 默认模型槽位；完整配置校验要求用户显式配置。
pub const DEFAULT_AGENT_MODEL_SLOT: &str = "";
/// 主 Agent 默认 reasoning effort。
pub const DEFAULT_AGENT_REASONING_EFFORT: &str = "medium";
/// OpenAI 与 Anthropic API reasoning effort 官方值并集。
pub const REASONING_EFFORT_VALUES: &[&str] =
    &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
/// OpenAI Responses / Chat Completions reasoning effort 官方值。
pub const OPENAI_REASONING_EFFORT_VALUES: &[&str] =
    &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
/// Anthropic Messages output_config.effort 官方值。
pub const ANTHROPIC_REASONING_EFFORT_VALUES: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// CLI 与 TUI reasoning effort 参数提示。
pub const REASONING_EFFORT_ARGUMENT_HINT: &str = "none|minimal|low|medium|high|xhigh|max";
/// 非 embedding 模型的运行时默认上下文窗口。
pub const DEFAULT_MODEL_CONTEXT_WINDOW: u64 = 128_000;
/// 模型默认最大输出 token。
pub const DEFAULT_MODEL_MAX_OUTPUT_TOKENS: u64 = 128_000;

/// 规范化 reasoning effort；只接受官方原始值，不保留旧前缀兼容。
#[must_use]
pub fn normalize_reasoning_effort(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
/// 默认每个 run session 允许的最大子代理线程数。
pub const DEFAULT_AGENT_MAX_SUBAGENT_THREADS: usize = 16;
/// 默认每个 run session 允许同时运行的子代理 turn 数。
pub const DEFAULT_AGENT_MAX_SUBAGENT_ACTIVE_TURNS: usize = 16;
/// 默认子代理最大嵌套深度，root 深度为 0。
pub const DEFAULT_AGENT_MAX_SUBAGENT_DEPTH: u32 = 3;
/// Agent 自动压缩触发阈值（context_window 的比例），默认 0.8 (80%)。
pub const DEFAULT_AGENT_AUTO_COMPACTION_THRESHOLD: f64 = 0.8;
/// Agent 闲暇压缩 debounce 窗口，单位毫秒。
pub const DEFAULT_AGENT_IDLE_COMPACTION_DEBOUNCE_MS: u64 = 2_000;
/// Agent 默认日志级别；详细日志必须由配置显式开启。
pub const DEFAULT_AGENT_LOG_LEVEL: &str = "warn";
/// 日志级别的规范候选值，兼容别名不作为独立选项展示。
pub const LOG_LEVEL_VALUES: &[&str] = &["trace", "debug", "info", "warn", "error"];
/// 旧配置与外部调用支持的警告级别别名。
pub const LOG_LEVEL_WARNING_ALIAS: &str = "warning";
/// 上下文压缩阈值分子，目标阈值为模型窗口的 80%。
pub const COMPRESSION_CONTEXT_WINDOW_NUMERATOR: u64 = 8;
/// 上下文压缩阈值分母，目标阈值为模型窗口的 80%。
pub const COMPRESSION_CONTEXT_WINDOW_DENOMINATOR: u64 = 10;

/// axon_cli 内置工具默认允许的子命令；当前未暴露该工具，保持空白名单。
#[must_use]
pub fn default_axon_cli_allowed_subcommands() -> Vec<String> {
    Vec::new()
}
/// 标准远控 cursor 最大事件数。
pub const STANDARD_REMOTE_MAX_EVENTS_PER_CURSOR: usize = 200;
/// 标准远控最大 payload 字节数。
pub const STANDARD_REMOTE_MAX_PAYLOAD_BYTES: usize = 65_536;
/// 高吞吐远控 cursor 最大事件数。
pub const HIGH_THROUGHPUT_REMOTE_MAX_EVENTS_PER_CURSOR: usize = 1_000;
/// 高吞吐远控最大 payload 字节数。
pub const HIGH_THROUGHPUT_REMOTE_MAX_PAYLOAD_BYTES: usize = 262_144;
/// 用户级 Axon 根目录名。
pub const USER_AXON_DIR: &str = ".axon";
/// Windows 用户配置目录名。
pub const USER_CONFIG_VENDOR_DIR: &str = "Axon";
/// Axon 配置子目录名。
pub const USER_CONFIG_DIR: &str = "config";
/// Axon 配置文件名。
pub const USER_CONFIG_FILE: &str = "config.toml";
/// TUI resize/replay 自动模式的兜底行数预算，对齐 Codex 对未知终端的保守默认值。
pub const DEFAULT_TUI_TERMINAL_RESIZE_REFLOW_FALLBACK_MAX_ROWS: usize = 1_000;
/// 用户级运行态数据目录名。
pub const USER_RUNTIME_DATA_DIR: &str = "data";
/// 用户级运行态数据子目录：runtime（存放 shell_history 等运行态日志）。
pub const USER_RUNTIME_SUBDIR: &str = "runtime";

/// OpenAI 官方默认 base URL。
pub const DEFAULT_BASE_URL_OPENAI: &str = "https://api.openai.com/v1";
/// Anthropic 官方默认 base URL。
pub const DEFAULT_BASE_URL_ANTHROPIC: &str = "https://api.anthropic.com/v1";
/// 阿里云百炼多模态 embedding 默认 base URL。
pub const DEFAULT_BASE_URL_DASHSCOPE: &str = "https://dashscope.aliyuncs.com/api/v1";
/// 火山方舟多模态 embedding 默认 base URL。
pub const DEFAULT_BASE_URL_VOLCENGINE_ARK_MULTIMODAL: &str =
    "https://ark.cn-beijing.volces.com/api/v3";

/// 根据 protocol 返回默认 base_url。
#[must_use]
pub fn default_base_url_for_protocol(protocol: &str) -> Option<&'static str> {
    match protocol {
        crate::PROVIDER_PROTOCOL_OPENAI => Some(DEFAULT_BASE_URL_OPENAI),
        crate::PROVIDER_PROTOCOL_ANTHROPIC => Some(DEFAULT_BASE_URL_ANTHROPIC),
        crate::PROVIDER_PROTOCOL_DASHSCOPE => Some(DEFAULT_BASE_URL_DASHSCOPE),
        crate::PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL => {
            Some(DEFAULT_BASE_URL_VOLCENGINE_ARK_MULTIMODAL)
        }
        _ => None,
    }
}
/// 统一 Gateway API token 默认环境变量名。
pub const DEFAULT_GATEWAY_API_TOKEN_ENV: &str = "AXON_GATEWAY_API_TOKEN";
/// Gateway HTTP Webhook 默认监听地址。
pub const DEFAULT_GATEWAY_HTTP_BIND: &str = "127.0.0.1:3143";
/// Gateway WebSocket 默认监听地址。
pub const DEFAULT_GATEWAY_WEBSOCKET_BIND: &str = "127.0.0.1:3144";
/// 默认不持久化渠道遥测样本。
pub const DEFAULT_CHANNEL_METRICS_PERSISTENCE_ENABLED: bool = false;
