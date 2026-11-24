use axon_domain::ToolExecutionPermission;
use axon_protocol::wire::{WireLifecyclePhase, WireLifecycleStatus};

const EXECUTION_PERMISSION_ALLOWED: &str = "allowed";
const EXECUTION_PERMISSION_FORBIDDEN: &str = "forbidden";

pub(super) fn lifecycle_correlation(kind: &str, attempt_index: Option<usize>) -> String {
    attempt_index.map_or_else(
        || kind.to_owned(),
        |attempt| format!("lifecycle:{kind}:{attempt}"),
    )
}

pub(super) fn tool_correlation(provider_call_id: &str) -> String {
    format!("tool:{provider_call_id}")
}

pub(super) fn streaming_summary(provider: &str, model: &str) -> String {
    let provider = provider.trim();
    let model = model.trim();
    match (provider.is_empty(), model.is_empty()) {
        (false, false) => format!("{provider}/{model} 生成回复中"),
        (true, false) => format!("{model} 生成回复中"),
        (false, true) => format!("{provider} 生成回复中"),
        (true, true) => "生成回复中".to_owned(),
    }
}

pub(super) fn lifecycle_phase(value: &str) -> WireLifecyclePhase {
    match value.trim().to_ascii_lowercase().as_str() {
        "command" | "shell" => WireLifecyclePhase::UserShell,
        "review" => WireLifecyclePhase::Review,
        "compact" | "compaction" => WireLifecyclePhase::Compact,
        "governance" => WireLifecyclePhase::Governance,
        _ => WireLifecyclePhase::RegularTurn,
    }
}

pub(super) fn wire_lifecycle_status(value: &str) -> WireLifecycleStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "started" => WireLifecycleStatus::Started,
        "running" => WireLifecycleStatus::Running,
        "waiting" => WireLifecycleStatus::Waiting,
        "completed" | "succeeded" | "success" | "passed" => WireLifecycleStatus::Completed,
        "cancelled" | "canceled" | "stopped" => WireLifecycleStatus::Cancelled,
        "aborted" => WireLifecycleStatus::Aborted,
        "blocked" => WireLifecycleStatus::Blocked,
        _ => WireLifecycleStatus::Failed,
    }
}

pub(super) fn execution_permission_name(permission: &ToolExecutionPermission) -> String {
    match permission {
        ToolExecutionPermission::Allowed => EXECUTION_PERMISSION_ALLOWED.to_owned(),
        ToolExecutionPermission::Forbidden { .. } => EXECUTION_PERMISSION_FORBIDDEN.to_owned(),
    }
}

pub(super) fn current_time_ms() -> i64 {
    i64::try_from(crate::run_commands::run_execution::now_ms()).unwrap_or(i64::MAX)
}
