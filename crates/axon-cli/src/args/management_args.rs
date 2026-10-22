use super::*;

/// `axon models` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ModelsCommand {
    /// provider 过滤器。
    #[arg(long)]
    pub provider: Option<String>,
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// `axon providers` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ProvidersCommand {
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    /// provider 子命令。
    #[command(subcommand)]
    pub command: Option<ProviderCommand>,
}

/// provider 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum ProviderCommand {
    /// 添加 provider。
    Add(ProviderAddCommand),
    /// 移除 provider。
    Remove(ProviderNameCommand),
    /// 测试 provider。
    Test(ProviderNameCommand),
}

/// provider add 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ProviderAddCommand {
    /// provider 名称。
    #[arg(long)]
    pub name: String,
    /// provider 协议。
    #[arg(long, value_enum)]
    pub protocol: ProviderProtocol,
    /// base URL。
    #[arg(long)]
    pub base_url: String,
    /// API key 环境变量名。
    #[arg(long)]
    pub api_key_env: Option<String>,
    /// provider streaming 底层传输链路。
    #[arg(long, value_enum, default_value_t = ProviderWireTransport::Http)]
    pub wire_transport: ProviderWireTransport,
    /// WebSocket URL；省略时从 base URL 推导。
    #[arg(long)]
    pub websocket_url: Option<String>,
}

/// provider 名称参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ProviderNameCommand {
    /// provider 名称。
    pub name: String,
}

/// provider 协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderProtocol {
    /// OpenAI 协议（默认 Responses API）。
    #[value(name = "openai")]
    OpenAi,
    /// OpenAI Chat Completions 协议。
    #[value(name = "openai-chatcompletions")]
    OpenAiChatCompletions,
    /// Anthropic Messages 协议。
    #[value(name = "anthropic")]
    Anthropic,
    /// 阿里云百炼多模态 embedding 协议。
    #[value(name = "dashscope")]
    DashScope,
    /// 火山方舟多模态 embedding 协议。
    #[value(name = "volcengine-ark-multimodal")]
    VolcengineArkMultimodal,
}

impl ProviderProtocol {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::OpenAi => PROVIDER_PROTOCOL_OPENAI,
            Self::OpenAiChatCompletions => PROVIDER_PROTOCOL_OPENAI_CHAT,
            Self::Anthropic => PROVIDER_PROTOCOL_ANTHROPIC,
            Self::DashScope => PROVIDER_PROTOCOL_DASHSCOPE,
            Self::VolcengineArkMultimodal => PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL,
        }
    }
}

/// provider streaming 底层传输链路。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum ProviderWireTransport {
    /// HTTP/SSE streaming。
    #[default]
    #[value(name = "http")]
    Http,
    /// WebSocket streaming。
    #[value(name = "websocket")]
    WebSocket,
}

impl ProviderWireTransport {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::Http => PROVIDER_WIRE_TRANSPORT_HTTP,
            Self::WebSocket => PROVIDER_WIRE_TRANSPORT_WEBSOCKET,
        }
    }
}

/// `axon config` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ConfigCommand {
    /// config 子命令。
    #[command(subcommand)]
    pub command: Option<ConfigSubcommand>,
}

/// config 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum ConfigSubcommand {
    /// 展示配置。
    Show,
    /// 打开配置文件。
    Edit,
    /// 设置单个配置值。
    Set(ConfigSetCommand),
    /// 获取单个配置值。
    Get(ConfigGetCommand),
    /// 校验配置。
    Validate,
    /// 列出配置键。
    Keys,
    /// 输出配置文件路径。
    Path,
    /// 输出相邻 env 文件路径。
    EnvPath,
}

/// `axon config set` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ConfigSetCommand {
    /// 点分配置键。
    pub key: String,
    /// 配置值。
    pub value: String,
}

/// `axon config get` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ConfigGetCommand {
    /// 点分配置键。
    pub key: String,
}
