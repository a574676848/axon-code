use super::{Args, OutputFormat, PathBuf, Subcommand, ValueEnum};

/// 新增供应商时使用的协议；通常由供应商名称自动选择。
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SetupProviderProtocol {
    /// OpenAI Responses。
    Openai,
    /// OpenAI Chat Completions。
    OpenaiChatcompletions,
    /// Anthropic Messages。
    Anthropic,
    /// Dashscope。
    Dashscope,
    /// Volcengine Ark。
    VolcengineArkMultimodal,
}

impl SetupProviderProtocol {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::OpenaiChatcompletions => "openai-chatcompletions",
            Self::Anthropic => "anthropic",
            Self::Dashscope => "dashscope",
            Self::VolcengineArkMultimodal => "volcengine-ark-multimodal",
        }
    }
}

/// 仅在供应商明确要求时才需要改用 WebSocket。
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SetupWireTransport {
    /// 默认 HTTP/SSE。
    Http,
    /// WebSocket。
    Websocket,
}

impl SetupWireTransport {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Websocket => "websocket",
        }
    }
}

/// `axon setup` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SetupCommand {
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    /// Provider 名称。
    #[arg(long)]
    pub provider_name: Option<String>,
    /// 新增自定义供应商时的协议；已知供应商会自动推导。
    #[arg(long, value_enum)]
    pub protocol: Option<SetupProviderProtocol>,
    /// 自定义供应商 API 地址；已知供应商会自动推导。
    #[arg(long)]
    pub base_url: Option<String>,
    /// 供应商明确要求 WebSocket 时填写。
    #[arg(long, value_enum)]
    pub wire_transport: Option<SetupWireTransport>,
    /// 自定义 WebSocket 地址。
    #[arg(long)]
    pub websocket_url: Option<String>,
    /// 清除 WebSocket 地址。
    #[arg(long, conflicts_with = "websocket_url")]
    pub clear_websocket_url: bool,
    /// 配置完成后写入的 agent 模型槽。
    #[arg(long)]
    pub model: Option<String>,
    /// 模型显示名称；省略时使用模型 ID。
    #[arg(long)]
    pub model_name: Option<String>,
    /// 主 Axon 的推理强度；留空使用模型默认值。
    #[arg(long)]
    pub reasoning_effort: Option<String>,
    /// 清除推理强度设置。
    #[arg(long, conflicts_with = "reasoning_effort")]
    pub clear_reasoning_effort: bool,
    /// 模型上下文窗口（高级设置）。
    #[arg(long)]
    pub context_window: Option<u64>,
    /// 模型最大输出 token（高级设置）。
    #[arg(long)]
    pub max_output_tokens: Option<u64>,
    /// 清除最大输出 token 设置。
    #[arg(long, conflicts_with = "max_output_tokens")]
    pub clear_max_output_tokens: bool,
    /// 模型是否支持视觉输入（高级设置）。
    #[arg(long)]
    pub supports_vision: Option<bool>,
    /// 模型是否支持 embedding（高级设置）。
    #[arg(long)]
    pub supports_embedding: Option<bool>,
    /// embedding 维度（高级设置）。
    #[arg(long)]
    pub embedding_dimension: Option<usize>,
    /// 非交互模式。
    #[arg(long)]
    pub non_interactive: bool,
    /// 仅配置缺失项；已有 provider/model/secret 保持不变。
    #[arg(long)]
    pub quick: bool,
    /// agent 生成的 setup JSON 配置。
    #[arg(long)]
    pub agent_config: Option<PathBuf>,
    /// 覆盖供应商密钥环境变量名；通常自动推导。
    #[arg(long)]
    pub api_key_env: Option<String>,
    /// 写入相邻 `.env` 的 API key。
    #[arg(long)]
    pub api_key: Option<String>,
    /// 清除 API key。
    #[arg(long, conflicts_with = "api_key")]
    pub clear_api_key: bool,
    /// 重置指定的 provider 名称及其所有模型。
    #[arg(long)]
    pub reset_provider: Option<String>,
    /// 重置 Agent 三个模型槽位及 reasoning effort。
    #[arg(long)]
    pub reset_agent_models: bool,
    /// 跳过 setup 后本地验收。
    #[arg(long)]
    pub no_validate: bool,
}

/// `axon auth` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct AuthCommand {
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    /// auth 子命令。
    #[command(subcommand)]
    pub command: Option<AuthSubcommand>,
}

/// auth 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum AuthSubcommand {
    /// 展示本机凭据状态。
    Status(AuthStatusCommand),
}

/// `axon auth status` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct AuthStatusCommand {
    /// provider 名称；省略时展示全部 provider。
    #[arg(long)]
    pub provider: Option<String>,
}
