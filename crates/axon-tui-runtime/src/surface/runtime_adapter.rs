//! Runtime adapter 契约事实源。

use std::sync::Arc;

use super::error_model::{IntoTuiError, TuiError};

/// 当前 TUI runtime adapter 契约版本。
pub const RUNTIME_ADAPTER_SCHEMA_VERSION: RuntimeAdapterSchemaVersion =
    RuntimeAdapterSchemaVersion::new(1, 0);

/// Runtime adapter schema version。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeAdapterSchemaVersion {
    /// 破坏性契约版本。
    pub major: u16,
    /// 能力版本。
    pub minor: u16,
}

impl RuntimeAdapterSchemaVersion {
    /// 创建 schema version。
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// 当前 host 是否可执行该 adapter。
    #[must_use]
    pub const fn matches_expected(self, expected: Self) -> bool {
        self.major == expected.major && self.minor == expected.minor
    }

    /// 格式化为稳定显示文本。
    #[must_use]
    pub fn render(self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

/// Runtime adapter 能力声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeAdapterCapabilities {
    /// 支持运行中输入注入。
    pub steer: bool,
    /// 支持流式输出。
    pub streaming: bool,
    /// 支持并行工具调用。
    pub parallel_tools: bool,
    /// 支持 assistant delta 流。
    pub streaming_assistant: bool,
    /// 支持 plan delta 流。
    pub streaming_plan: bool,
}

impl RuntimeAdapterCapabilities {
    /// 当前 Axon CLI runtime 的默认能力。
    pub const AXON_CLI: Self = Self {
        steer: false,
        streaming: true,
        parallel_tools: true,
        streaming_assistant: true,
        streaming_plan: true,
    };
}

impl Default for RuntimeAdapterCapabilities {
    fn default() -> Self {
        Self::AXON_CLI
    }
}

/// Runtime adapter 契约声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeAdapterContract {
    /// adapter 提供的 schema version。
    pub schema_version: RuntimeAdapterSchemaVersion,
    /// adapter 能力集合。
    pub capabilities: RuntimeAdapterCapabilities,
}

impl RuntimeAdapterContract {
    /// 创建契约声明。
    #[must_use]
    pub const fn new(
        schema_version: RuntimeAdapterSchemaVersion,
        capabilities: RuntimeAdapterCapabilities,
    ) -> Self {
        Self {
            schema_version,
            capabilities,
        }
    }

    /// 当前 host 的默认契约。
    #[must_use]
    pub const fn current() -> Self {
        Self::new(
            RUNTIME_ADAPTER_SCHEMA_VERSION,
            RuntimeAdapterCapabilities::AXON_CLI,
        )
    }

    /// 校验 adapter schema。
    pub fn validate(self) -> Result<Self, RuntimeError> {
        if self
            .schema_version
            .matches_expected(RUNTIME_ADAPTER_SCHEMA_VERSION)
        {
            return Ok(self);
        }
        Err(RuntimeError::schema_mismatch(
            RUNTIME_ADAPTER_SCHEMA_VERSION,
            self.schema_version,
        ))
    }
}

impl Default for RuntimeAdapterContract {
    fn default() -> Self {
        Self::current()
    }
}

/// Runtime adapter 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    error: Box<TuiError>,
}

impl RuntimeError {
    /// 创建运行时错误。
    #[must_use]
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            error: Box::new(TuiError::runtime(message)),
        }
    }

    /// 创建 steer 拒绝错误。
    #[must_use]
    pub fn steer_rejected(message: impl Into<String>) -> Self {
        Self {
            error: Box::new(TuiError::steer_rejected(message)),
        }
    }

    /// 创建 schema mismatch 错误。
    #[must_use]
    pub fn schema_mismatch(
        expected: RuntimeAdapterSchemaVersion,
        actual: RuntimeAdapterSchemaVersion,
    ) -> Self {
        let expected = expected.render();
        let actual = actual.render();
        Self {
            error: Box::new(TuiError::schema_mismatch("", expected, actual)),
        }
    }

    /// 创建 worker panic 错误。
    #[must_use]
    pub fn worker_panic(panic_summary: impl Into<String>) -> Self {
        Self {
            error: Box::new(TuiError::worker_panic(panic_summary)),
        }
    }

    /// 创建 worker channel 断开错误。
    #[must_use]
    pub fn worker_channel_disconnected() -> Self {
        Self {
            error: Box::new(TuiError::worker_channel_disconnected()),
        }
    }

    /// 用户可见消息。
    #[must_use]
    pub fn message(&self) -> &str {
        &self.error.message
    }

    /// 稳定错误类型。
    #[must_use]
    pub fn error_type(&self) -> RuntimeErrorType {
        RuntimeErrorType::from_tui_error(&self.error)
    }

    /// 可选错误码。
    #[must_use]
    pub fn error_code(&self) -> Option<&str> {
        self.error.error_code.as_deref()
    }

    /// 可选恢复建议。
    #[must_use]
    pub fn next_step(&self) -> Option<&str> {
        self.error.next_step.as_deref()
    }

    /// 转为统一 TUI 错误模型。
    #[must_use]
    pub fn to_tui_error(&self) -> TuiError {
        (*self.error).clone()
    }
}

impl IntoTuiError for RuntimeError {
    fn into_tui_error(self) -> TuiError {
        *self.error
    }
}

impl From<TuiError> for RuntimeError {
    fn from(error: TuiError) -> Self {
        Self {
            error: Box::new(error),
        }
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for RuntimeError {}

/// Runtime adapter 错误类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorType {
    /// adapter schema 与 host 不匹配。
    SchemaMismatch,
    /// 运行时执行失败。
    Runtime,
    /// 当前 turn 拒绝 steer。
    SteerRejected,
}

impl RuntimeErrorType {
    fn from_tui_error(error: &TuiError) -> Self {
        match error.error_code.as_deref() {
            Some("runtime_adapter_schema_mismatch") => Self::SchemaMismatch,
            Some("steer_rejected") => Self::SteerRejected,
            _ => Self::Runtime,
        }
    }

    /// 转为现有 TUI error_type。
    #[must_use]
    pub const fn as_tui_error_type(self) -> &'static str {
        match self {
            Self::SchemaMismatch => "configuration",
            Self::Runtime | Self::SteerRejected => "runtime",
        }
    }
}

/// 可跨线程复制的 Runtime 事件发送器。
pub struct RuntimeEventSender<Event> {
    send: Arc<dyn Fn(Event) -> Result<(), RuntimeError> + Send + Sync>,
}

impl<Event> Clone for RuntimeEventSender<Event> {
    fn clone(&self) -> Self {
        Self {
            send: Arc::clone(&self.send),
        }
    }
}

impl<Event> RuntimeEventSender<Event> {
    /// 创建线程安全事件发送器。
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(Event) -> Result<(), RuntimeError> + Send + Sync + 'static,
    {
        Self {
            send: Arc::new(send),
        }
    }

    /// 发送一个事件。
    pub fn send(&self, event: Event) -> Result<(), RuntimeError> {
        (self.send)(event)
    }
}

/// Runtime 事件发射器。
pub struct RuntimeEventEmitter<'a, Event> {
    emit: &'a mut dyn FnMut(Event),
    try_emit: Option<RuntimeEventSender<Event>>,
}

impl<'a, Event> RuntimeEventEmitter<'a, Event> {
    /// 创建同步发射器。
    pub fn new(emit: &'a mut dyn FnMut(Event)) -> Self {
        Self {
            emit,
            try_emit: None,
        }
    }

    /// 创建带线程安全非阻塞通道的发射器。
    pub fn with_try_emit(
        emit: &'a mut dyn FnMut(Event),
        try_emit: RuntimeEventSender<Event>,
    ) -> Self {
        Self {
            emit,
            try_emit: Some(try_emit),
        }
    }

    /// 获取可在线程间共享的事件 sender。
    #[must_use]
    pub fn thread_safe_sender(&self) -> Option<RuntimeEventSender<Event>> {
        self.try_emit.clone()
    }

    /// 同步发射 runtime event。
    pub fn emit(&mut self, event: Event) {
        (self.emit)(event);
    }

    /// 尝试非阻塞发射 runtime event。
    pub fn try_emit(&mut self, event: Event) -> Result<(), RuntimeError> {
        let Some(sender) = &self.try_emit else {
            self.emit(event);
            return Ok(());
        };
        sender.send(event)
    }
}
