//! Agent TUI 运行错误到结构化 error cell 的投影。

use super::run_output::{AgentTuiChannelAttempt, AgentTuiRunError};
use crate::surface::{
    cell_event_data::AgentRenderErrorCell,
    error_messages::{ErrorMessageInput, project_error_message},
    error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity},
};

pub(crate) fn run_error_cell(error: &AgentTuiRunError) -> AgentRenderErrorCell {
    let projection = project_error_message(&ErrorMessageInput {
        raw_message: error.message.trim(),
        error_type: error.error_type.as_deref(),
        http_status: error.http_status,
        error_code: error.error_code.as_deref(),
        error_param: error.error_param.as_deref(),
    });
    AgentRenderErrorCell {
        id: error.id.clone(),
        title: projection.title,
        message: projection.message,
        details: structured_error_details(error),
        next_step: error.next_step.clone().or(projection.next_step),
        severity: projection.severity,
        action: projection.action,
        cell_kind: projection.cell_kind,
    }
}

pub(crate) fn local_error_cell(
    id: impl Into<String>,
    title: impl Into<String>,
    message: impl Into<String>,
    next_step: Option<String>,
) -> AgentRenderErrorCell {
    AgentRenderErrorCell {
        id: id.into(),
        title: title.into(),
        message: message.into(),
        details: Vec::new(),
        next_step,
        severity: TuiErrorSeverity::Recoverable,
        action: TuiErrorAction::None,
        cell_kind: TuiErrorCellKind::Ui,
    }
}

fn structured_error_details(error: &AgentTuiRunError) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(status) = error.http_status {
        details.push(format!("HTTP {status}"));
    }
    if let Some(code) = error
        .error_code
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        details.push(format!("代码 {code}"));
    }
    if let Some(param) = error
        .error_param
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        details.push(format!("参数 {param}"));
    }
    if let Some(event_id) = error
        .caused_by_runtime_event
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        details.push(format!("事件 {event_id}"));
    }
    details.extend(error.details.iter().filter_map(|detail| {
        let detail = detail.trim();
        (!detail.is_empty()).then(|| detail.to_owned())
    }));
    details.extend(error.channel_attempts.iter().map(channel_attempt_detail));
    details.dedup();
    details
}

fn channel_attempt_detail(attempt: &AgentTuiChannelAttempt) -> String {
    let mut parts = vec![format!("{}/{}", attempt.provider, attempt.model)];
    if let Some(status) = attempt.http_status {
        parts.push(format!("HTTP {status}"));
    }
    parts.push(channel_attempt_outcome_label(&attempt.outcome).to_owned());
    parts.join(" · ")
}

fn channel_attempt_outcome_label(outcome: &str) -> &str {
    match outcome {
        "success" => "成功",
        "transient_failure" => "瞬时失败",
        "permanent_failure" => "永久失败",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_error_cell_uses_dictionary_copy_for_provider_auth() {
        let error = AgentTuiRunError::structured(
            "Incorrect API key provided: sk-secret",
            Some("provider_transport".to_owned()),
            Some(401),
            Some("invalid_api_key".to_owned()),
            None,
        );

        let cell = run_error_cell(&error);

        assert_eq!(cell.title, "认证失败");
        assert_eq!(cell.message, "模型服务拒绝了当前凭据。");
        assert_eq!(cell.action, TuiErrorAction::Reconfigure);
        assert!(cell.details.iter().any(|detail| detail == "HTTP 401"));
        assert!(!cell.message.contains("sk-secret"));
    }

    #[test]
    fn run_error_cell_maps_mcp_crash_code_to_retry() {
        let error = AgentTuiRunError::structured(
            "mcp server closed: transport closed",
            Some("runtime".to_owned()),
            None,
            Some("mcp_crash".to_owned()),
            None,
        );

        let cell = run_error_cell(&error);

        assert_eq!(cell.title, "MCP 服务异常");
        assert_eq!(cell.action, TuiErrorAction::Retry);
        assert_eq!(cell.cell_kind, TuiErrorCellKind::Runtime);
    }

    #[test]
    fn run_error_cell_preserves_memory_persistence_message() {
        let error = AgentTuiRunError::structured(
            "Memory SQLite 记录写入失败",
            Some("memory_persistence".to_owned()),
            None,
            None,
            None,
        );

        let cell = run_error_cell(&error);

        assert_eq!(cell.title, "Memory 持久化失败");
        assert_eq!(cell.message, "Memory SQLite 记录写入失败");
        assert_eq!(cell.action, TuiErrorAction::Retry);
    }

    #[test]
    fn run_error_cell_maps_snapshot_permission_to_inline_snapshot_action() {
        let error = AgentTuiRunError::structured(
            "access is denied",
            Some("snapshot".to_owned()),
            None,
            Some("snapshot_permission".to_owned()),
            None,
        );

        let cell = run_error_cell(&error);

        assert_eq!(cell.title, "会话快照无法访问");
        assert_eq!(cell.action, TuiErrorAction::Reconfigure);
        assert_eq!(cell.cell_kind, TuiErrorCellKind::Snapshot);
        assert!(cell.next_step.is_some());
    }

    #[test]
    fn run_error_cell_keeps_channel_attempts_as_structured_details() {
        let error = AgentTuiRunError::structured(
            "all providers failed",
            Some("provider_transport".to_owned()),
            Some(502),
            None,
            None,
        )
        .with_channel_attempts(vec![AgentTuiChannelAttempt {
            provider: "openai-a".to_owned(),
            model: "gpt-5".to_owned(),
            http_status: Some(429),
            outcome: "transient_failure".to_owned(),
        }]);

        let cell = run_error_cell(&error);

        assert!(
            cell.details
                .iter()
                .any(|detail| detail == "openai-a/gpt-5 · HTTP 429 · 瞬时失败")
        );
    }

    #[test]
    fn unknown_error_does_not_render_raw_debug_as_message() {
        let error = AgentTuiRunError::message("provider.rs:88 token=secret debug dump");

        let cell = run_error_cell(&error);

        assert_eq!(cell.title, "运行失败");
        assert!(!cell.message.contains("provider.rs"));
        assert!(!cell.message.contains("secret"));
    }
}
