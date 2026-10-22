//! 持久 Session Action 的应用运行时。
//!
//! 本 crate 只协调 Action、checkpoint、recovery 与 Agent 执行端口，
//! 不依赖 CLI、Feishu、IPC 或操作系统服务。

pub mod application;
pub mod model;
pub mod ports;

pub use application::{
    AgentTurnExecutor, ContinuationResult, ExecutionWorker, ExecutionWorkerError,
    JarvisRuntimeProfile, JarvisRuntimeSelector, ProviderToolEnvironment, ProviderTurnRequest,
    ProviderTurnResult, ProviderTurnRunner, SessionRecovery, SnapshotChunker,
    wire_event_to_runtime_event,
};
pub use model::{
    ActionExecutionRequest, NextExecutionRequest, RuntimeEvent, SessionExecutionContext,
    TurnExecutionError, TurnExecutionOutcome, TurnFailureKind,
};
pub use ports::{
    AtomicTurnPerformanceRecorder, EventPublisher, RuntimeClock, SubagentSessionSinkFactory,
    TurnEventSink, TurnExecutor, TurnPerformanceRecorder, TurnPerformanceSnapshot,
    TurnPerformanceStage, TurnPerformanceStageTiming, configure_first_token_telemetry,
    first_token_telemetry_enabled,
};
