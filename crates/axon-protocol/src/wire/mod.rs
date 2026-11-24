//! Axon 跨端交互 wire 协议。
//!
//! 本模块只定义跨 runtime、CLI、TUI、remote 的稳定 serde schema。
//! 端侧 view model 可以从这些类型投射，但协议层不承载 UI 或 runtime 逻辑。

mod event;
mod memory_surface;
mod provider_status;
mod snapshot;
mod token_budget_event;

pub use crate::diff::FileChange;
pub use event::{
    WIRE_LIFECYCLE_KIND_PROVIDER_RETRY, WIRE_LIFECYCLE_KIND_STREAMING_COMPLETED,
    WIRE_LIFECYCLE_KIND_STREAMING_STARTED, WIRE_SCHEMA_VERSION, WireAssistantMessage,
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireFileChangeEvent, WireGovernanceStatus,
    WireLifecyclePhase, WireLifecycleStatus, WireMemoryStatus, WireMemorySurfaceEntry,
    WirePlanStep, WirePlanUpdate, WireReasoningDelta, WireReasoningEnd, WireReasoningMessage,
    WireReasoningStart, WireReviewStatus, WireRuntimeError, WireSchemaVersion, WireSessionRef,
    WireSkillStatus, WireToolCall, WireToolResult, WireTurnLifecycle, WireTurnSummary,
    WireUsageUpdate, WireUserMessage, WireUserQuestion, WireVerifyStatus,
    is_provider_retry_lifecycle,
};
pub use memory_surface::memory_surface_text_is_user_visible;
pub use provider_status::{WireProviderStatus, WireProviderStatusPhase, WireProviderTransport};
pub use snapshot::{
    WireFocusedInteraction, WireInputMode, WireInputPanelState, WireRenderTimelineItem,
    WireRenderTimelineItemKind, WireResumeSnapshot, WireScrollAnchor, WireStatusBarState,
    WireViewportSnapshot,
};
pub use token_budget_event::{WireContextRollover, WireTokenBudgetReminder};
