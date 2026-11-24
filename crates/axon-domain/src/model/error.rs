//! 领域层统一错误。

/// Axon 领域层结果类型。
pub type AxonResult<T> = Result<T, AxonError>;

/// 配置错误专用类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// 配置违反安全不变量。
    InvalidConfiguration(String),
    /// 配置文件不存在或无法读取。
    ConfigFileNotFound(String),
    /// 配置解析失败。
    ParseError(String),
    /// 必需的配置项缺失。
    MissingRequired(&'static str),
}

/// Memory 运行错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// Memory 持久化失败。
    Persistence(String),
}

/// 非配置类运行错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// 调用输入不满足领域约束。
    InvalidInput(String),
    /// 运行时状态违反不变量。
    InvalidState(String),
    /// 非 Memory 数据持久化失败。
    Persistence(String),
    /// 外部或内部协议数据无效。
    Protocol(String),
    /// 其他运行时失败。
    Failure(String),
}

/// Provider 工具调用不完整原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderToolCallIssue {
    /// 缺少调用 ID。
    MissingId,
    /// 缺少工具名称。
    MissingName,
    /// 参数为空。
    EmptyArguments,
    /// 参数不是完整 JSON。
    MalformedArguments,
    /// 空对象不满足工具 schema。
    IllegalEmptyObject,
}

impl ProviderToolCallIssue {
    /// 返回可持久化的稳定字段编码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingId => "missing_id",
            Self::MissingName => "missing_name",
            Self::EmptyArguments => "empty_arguments",
            Self::MalformedArguments => "malformed_arguments",
            Self::IllegalEmptyObject => "illegal_empty_object",
        }
    }
}

/// Provider 响应协议失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInvalidResponse {
    /// Provider 协议编码。
    pub provider_protocol: String,
    /// 已脱敏的错误消息。
    pub message: String,
    /// 稳定错误码。
    pub code: &'static str,
    /// 稳定失败类别。
    pub failure_kind: &'static str,
    /// 不完整协议字段。
    pub missing_field: Option<&'static str>,
    /// 不完整工具调用在响应中的索引。
    pub call_index: Option<usize>,
}

/// Provider 传输错误专用类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// Provider 传输失败。
    Transport {
        /// 错误消息。
        message: String,
        /// HTTP 状态码。
        http_status: Option<u16>,
        /// 触发失败的 Provider 终态事件类型（如 `response.failed`）；非事件类失败为 None。
        provider_event_type: Option<String>,
        /// Provider 或边缘网关返回的请求 ID；上游未回传时为 None。
        upstream_request_id: Option<String>,
    },
    /// 模型目录中找不到可用模型。
    NoModelRoute(String),
    /// Provider 响应格式无效。
    InvalidResponse(ProviderInvalidResponse),
    /// 请求超时。
    Timeout {
        /// 超时时长（毫秒）。
        timeout_ms: u64,
    },
    /// 速率限制。
    RateLimited {
        /// 重试建议（秒）。
        retry_after_secs: Option<u64>,
    },
}

/// 工具调用错误专用类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// 工具定义无效。
    InvalidDefinition(String),
    /// 工具参数无效。
    InvalidArguments(String),
    /// 工具执行失败。
    ExecutionFailed(String),
    /// 工具执行超时。
    Timeout {
        /// 超时时长（毫秒）。
        timeout_ms: u64,
    },
}

/// Cell 状态转换错误专用类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellError {
    /// Cell 状态转换无效。
    InvalidTransition {
        /// 当前状态。
        from: String,
        /// 目标状态。
        to: String,
        /// 原因。
        reason: String,
    },
    /// Cell 不存在。
    NotFound(String),
}

/// 不依赖外部框架的强类型错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxonError {
    /// 输入为空或只有空白字符。
    EmptyInput,
    /// 输入超过领域对象允许的长度。
    InputTooLong {
        /// 最大允许长度。
        limit: usize,
        /// 实际输入长度。
        actual: usize,
    },
    /// 远控动作被权限策略拒绝。
    RemoteDenied {
        /// 被拒绝的远控动作。
        action: crate::RemoteAction,
        /// 拒绝原因。
        reason: crate::RemoteDenialReason,
    },
    /// 当前平台尚未实现该能力。
    UnsupportedPlatform {
        /// 当前平台。
        platform: crate::PlatformKind,
        /// 未支持的能力。
        capability: &'static str,
    },
    /// 配置错误。
    Config(ConfigError),
    /// Memory 运行错误。
    Memory(MemoryError),
    /// 非配置类运行错误。
    Runtime(RuntimeError),
    /// Provider 传输错误。
    Provider(ProviderError),
    /// 工具调用错误。
    Tool(ToolError),
    /// Cell 状态转换错误。
    Cell(CellError),
    /// MCP 定义或协议数据无效。
    InvalidMcpDefinition(&'static str),
}

impl AxonError {
    /// 创建 Provider 工具调用不完整错误。
    pub fn provider_tool_call_incomplete(
        provider_protocol: impl Into<String>,
        call_index: usize,
        issue: ProviderToolCallIssue,
    ) -> Self {
        let missing_field = issue.as_str();
        Self::Provider(ProviderError::InvalidResponse(ProviderInvalidResponse {
            provider_protocol: provider_protocol.into(),
            message: format!(
                "provider tool-call incomplete: call_index={call_index}, missing_field={missing_field}"
            ),
            code: crate::RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE,
            failure_kind: crate::RUNTIME_FAILURE_KIND_INCOMPLETE,
            missing_field: Some(missing_field),
            call_index: Some(call_index),
        }))
    }

    /// 创建通用 Provider 响应协议错误。
    pub fn provider_invalid_response(
        provider_protocol: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Provider(ProviderError::InvalidResponse(ProviderInvalidResponse {
            provider_protocol: provider_protocol.into(),
            message: message.into(),
            code: crate::RUNTIME_FAILURE_CODE_PROVIDER_INVALID_RESPONSE,
            failure_kind: crate::RUNTIME_FAILURE_KIND_INVALID_RESPONSE,
            missing_field: None,
            call_index: None,
        }))
    }

    /// 创建 Memory 持久化错误。
    pub fn memory_persistence(message: impl Into<String>) -> Self {
        Self::Memory(MemoryError::Persistence(message.into()))
    }

    /// 创建配置不变量错误。
    pub fn config_invalid(message: impl Into<String>) -> Self {
        Self::Config(ConfigError::InvalidConfiguration(message.into()))
    }

    /// 创建输入无效错误。
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::Runtime(RuntimeError::InvalidInput(message.into()))
    }

    /// 创建运行时状态错误。
    pub fn runtime_invalid_state(message: impl Into<String>) -> Self {
        Self::Runtime(RuntimeError::InvalidState(message.into()))
    }

    /// 创建非 Memory 持久化错误。
    pub fn runtime_persistence(message: impl Into<String>) -> Self {
        Self::Runtime(RuntimeError::Persistence(message.into()))
    }

    /// 创建协议错误。
    pub fn runtime_protocol(message: impl Into<String>) -> Self {
        Self::Runtime(RuntimeError::Protocol(message.into()))
    }

    /// 创建通用运行时失败。
    pub fn runtime_failure(message: impl Into<String>) -> Self {
        Self::Runtime(RuntimeError::Failure(message.into()))
    }

    /// 创建工具参数无效错误。
    pub fn tool_invalid_arguments(message: impl Into<String>) -> Self {
        Self::Tool(ToolError::InvalidArguments(message.into()))
    }

    /// 创建工具执行失败错误。
    pub fn tool_execution_failed(message: impl Into<String>) -> Self {
        Self::Tool(ToolError::ExecutionFailed(message.into()))
    }

    /// 创建 provider transport 错误。
    pub fn provider_transport(message: impl Into<String>, http_status: Option<u16>) -> Self {
        Self::Provider(ProviderError::Transport {
            message: message.into(),
            http_status,
            provider_event_type: None,
            upstream_request_id: None,
        })
    }

    /// 创建带 Provider 归因信息的 transport 错误。
    ///
    /// 归因字段由 transport 层从 Provider 终态事件与响应头取得，供失败记录与界面展示；
    /// 取不到时传 None，不阻塞失败链路。
    pub fn provider_transport_with_attribution(
        message: impl Into<String>,
        http_status: Option<u16>,
        provider_event_type: Option<String>,
        upstream_request_id: Option<String>,
    ) -> Self {
        Self::Provider(ProviderError::Transport {
            message: message.into(),
            http_status,
            provider_event_type,
            upstream_request_id,
        })
    }

    /// 创建无 HTTP 状态的 Provider 运行失败。
    pub fn provider_failure(message: impl Into<String>) -> Self {
        Self::provider_transport(message, None)
    }

    /// 创建模型路由错误。
    pub fn provider_no_model_route(message: impl Into<String>) -> Self {
        Self::Provider(ProviderError::NoModelRoute(message.into()))
    }

    /// 创建工具定义错误。
    pub fn tool_invalid_definition(message: impl Into<String>) -> Self {
        Self::Tool(ToolError::InvalidDefinition(message.into()))
    }

    /// 创建 cell 状态转换错误。
    pub fn cell_invalid_transition(message: impl Into<String>) -> Self {
        Self::Cell(CellError::InvalidTransition {
            from: String::new(),
            to: String::new(),
            reason: message.into(),
        })
    }
}

// 实现 From trait 以支持错误转换
impl From<ConfigError> for AxonError {
    fn from(err: ConfigError) -> Self {
        AxonError::Config(err)
    }
}

impl From<MemoryError> for AxonError {
    fn from(err: MemoryError) -> Self {
        AxonError::Memory(err)
    }
}

impl From<ProviderError> for AxonError {
    fn from(err: ProviderError) -> Self {
        AxonError::Provider(err)
    }
}

impl From<ToolError> for AxonError {
    fn from(err: ToolError) -> Self {
        AxonError::Tool(err)
    }
}

impl From<CellError> for AxonError {
    fn from(err: CellError) -> Self {
        AxonError::Cell(err)
    }
}
