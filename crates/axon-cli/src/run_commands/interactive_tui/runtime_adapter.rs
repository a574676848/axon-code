//! CLI 到 Agent TUI runtime adapter 的桥接。

use std::path::PathBuf;

use axon_tui_runtime::{
    AgentTuiActiveTurn, AgentTuiCancelToken, AgentTuiCatalogLoader, AgentTuiCompletionInbox,
    AgentTuiLaunchRequest, AgentTuiMcpAuthAction, AgentTuiMcpAuthLoader, AgentTuiMcpAuthRequest,
    AgentTuiRunOutput, AgentTuiRuntimeAdapter, AgentTuiRuntimeEvent, AgentTuiRuntimePayload,
    AgentTuiSettingsChange, AgentTuiSteerResult, AgentTuiSubmit, AgentTuiSubmitFailure,
    RuntimeAdapterCapabilities, RuntimeAdapterContract, RuntimeError, RuntimeEventEmitter,
    RuntimeTransportTiming, TuiError, axon_cli_runtime_event,
};

use super::settings_persistence::persist_tui_settings_change_if_needed;
use super::{
    run_output_error_message, run_output_renderer::format_tui_run_output, tui_control_commands,
    tui_submit_mapper::interaction_from_tui_submit,
};
use crate::CliRunResult;
use crate::run_commands::{daemon_interaction_client, submit_prepared_daemon_interaction};

#[derive(Clone)]
pub(super) struct CliAgentTuiRuntimeAdapter {
    pub(super) config_path: Option<PathBuf>,
    pub(super) reply_destination: String,
}

impl AgentTuiRuntimeAdapter for CliAgentTuiRuntimeAdapter {
    fn contract(&self) -> RuntimeAdapterContract {
        let current = RuntimeAdapterContract::current();
        RuntimeAdapterContract::new(
            current.schema_version,
            RuntimeAdapterCapabilities {
                steer: true,
                ..RuntimeAdapterCapabilities::AXON_CLI
            },
        )
    }

    fn start_catalog_loader(
        &self,
        request: &AgentTuiLaunchRequest,
        force_mcp_refresh: bool,
    ) -> Option<AgentTuiCatalogLoader> {
        let config_path = self.config_path.clone()?;
        let cwd = Some(request.cwd.clone());
        Some(AgentTuiCatalogLoader::spawn(
            "axon-tui-catalog-loader",
            move || {
                if force_mcp_refresh {
                    request_daemon_mcp_refresh(cwd.as_deref(), &config_path);
                }
                super::load_tui_catalog_snapshot(&config_path, cwd.as_deref(), force_mcp_refresh)
            },
        ))
    }

    fn start_mcp_auth(
        &self,
        request: &AgentTuiLaunchRequest,
        auth: &AgentTuiMcpAuthRequest,
    ) -> Option<AgentTuiMcpAuthLoader> {
        let config_path = self.config_path.clone()?;
        let cwd = Some(request.cwd.clone());
        let action = auth.action;
        let server_id = auth.server_id.clone();
        let task_server_id = server_id.clone();
        Some(AgentTuiMcpAuthLoader::spawn(action, server_id, move || {
            let root = crate::runtime_paths::runtime_axon_root_path(&config_path)
                .map_err(|error| format!("Axon 运行态根目录解析失败：{error}"))?;
            match action {
                AgentTuiMcpAuthAction::Login => crate::mcp_commands::perform_browser_mcp_login(
                    &root,
                    cwd.as_deref(),
                    &task_server_id,
                ),
                AgentTuiMcpAuthAction::Logout => {
                    crate::mcp_commands::perform_mcp_logout(&root, cwd.as_deref(), &task_server_id)
                }
            }
        }))
    }

    /// 启动 TUI 完成通知收件箱：注册 presence 并持续轮询 Daemon 收件箱。
    ///
    /// 注册失败时返回 `None`，完成通知仍会投递到其他可用渠道，不会静默丢失。
    fn start_completion_inbox(
        &self,
        request: &AgentTuiLaunchRequest,
    ) -> Option<AgentTuiCompletionInbox> {
        crate::run_commands::daemon_completion_inbox_client::start_completion_inbox(
            self.config_path.as_deref(),
            &request.cwd,
            &self.reply_destination,
        )
    }

    fn start_session_prewarm(&self, request: &AgentTuiLaunchRequest) {
        if let Some(config_path) = self.config_path.clone() {
            let cwd = request.cwd.clone();
            std::thread::spawn(move || {
                axon_rustls_provider::ensure_rustls_crypto_provider();
                if let Ok(loaded) = axon_config::ConfigLoader::load_from_path(&config_path) {
                    let _ = axon_memory_read::prewarm_memory_embedding(
                        &loaded.config,
                        std::time::Duration::from_millis(500),
                    );
                }
                let _ =
                    crate::run_commands::run_request_payload::RunWorkspaceSnapshotPrewarm::spawn(
                        config_path,
                        cwd,
                        None,
                    );
            });
        }
    }

    fn load_initial_history(&self, request: &AgentTuiLaunchRequest) -> Option<AgentTuiRunOutput> {
        let history =
            crate::run_commands::daemon_interaction_client::load_latest_completed_turn_for_workspace(
                &request.cwd,
                self.config_path.as_deref(),
                super::TUI_CHANNEL,
            )
            .ok()??;
        Some(super::run_output_renderer::format_tui_initial_history(
            history.session_id,
            history.response,
            &history.wire_events,
        ))
    }

    fn load_active_turn(&self, request: &AgentTuiLaunchRequest) -> Option<AgentTuiActiveTurn> {
        daemon_interaction_client::load_active_interaction_for_workspace(
            &request.cwd,
            self.config_path.as_deref(),
            super::TUI_CHANNEL,
        )
        .ok()
        .flatten()
        .map(|interaction| {
            AgentTuiActiveTurn::with_reply_destination(
                interaction.content,
                interaction.reply_route.destination(),
            )
        })
    }

    fn resume_active_turn(
        &self,
        active_turn: &AgentTuiActiveTurn,
        request: &AgentTuiLaunchRequest,
        cancel_token: &AgentTuiCancelToken,
        emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<Option<AgentTuiRunOutput>, AgentTuiSubmitFailure> {
        let destination = active_turn
            .reply_destination
            .as_deref()
            .unwrap_or(self.reply_destination.as_str());
        let interaction = daemon_interaction_client::load_active_interaction(
            &request.cwd,
            self.config_path.as_deref(),
            super::TUI_CHANNEL,
            destination,
        )
        .map_err(RuntimeError::runtime)
        .map_err(AgentTuiSubmitFailure::without_output)?
        .ok_or_else(|| {
            AgentTuiSubmitFailure::without_output(RuntimeError::runtime(
                "Daemon 活动 Interaction 已结束",
            ))
        })?;
        let mut observe = |observation| match observation {
            daemon_interaction_client::DaemonTurnObservation::Wire(event) => {
                emit.emit(axon_cli_runtime_event(AgentTuiRuntimePayload::WireEvent {
                    event: Box::new(event),
                }));
            }
            daemon_interaction_client::DaemonTurnObservation::SubscriptionPhase(phase) => {
                emit.emit(axon_cli_runtime_event(
                    AgentTuiRuntimePayload::SubscriptionPhase(tui_subscription_phase(phase)),
                ));
            }
            daemon_interaction_client::DaemonTurnObservation::RuntimeObservation(frame) => {
                emit.emit(axon_cli_runtime_event(AgentTuiRuntimePayload::Observation(
                    frame,
                )));
            }
        };
        let projection = daemon_interaction_client::resume_daemon_interaction(
            interaction,
            self.config_path.as_deref(),
            &|| cancel_token.is_cancelled(),
            Some(&mut observe),
        );
        let result = daemon_projection_result(projection.map(|projection| {
            serde_json::json!({
                "sessionId": projection.session_id,
                "wireEvents": projection.wire_events,
                "content": projection.response,
            })
        }));
        let output = format_tui_run_output(&result);
        let _ = active_turn;
        if result.exit_code == 0 {
            Ok(Some(output))
        } else {
            // 恢复失败同样必须交出已获得的 durable 终态投影，并按失败收口。
            Err(AgentTuiSubmitFailure::with_output(
                runtime_error_from_run_output(&output),
                output,
            ))
        }
    }

    fn submit(
        &self,
        submission: &AgentTuiSubmit,
        cancel_token: &AgentTuiCancelToken,
        emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiRunOutput, AgentTuiSubmitFailure> {
        if let Some(config_path) = self.config_path.as_deref() {
            let change = AgentTuiSettingsChange::from_overrides(&submission.overrides);
            persist_tui_settings_change_if_needed(config_path, &change)
                .map_err(RuntimeError::runtime)
                .map_err(AgentTuiSubmitFailure::without_output)?;
        }
        if let Some(session_id) = tui_control_commands::active_tui_session_id(submission) {
            crate::run_commands::run_compaction::cancel_idle_compaction(&session_id);
        }
        {
            let mut emit_event = |event| emit.emit(event);
            if let Some(result) = tui_control_commands::run_tui_control_command(
                submission,
                self.config_path.as_deref(),
                &mut emit_event,
            ) {
                return result;
            }
        }
        let prepared = interaction_from_tui_submit(
            submission,
            &submission.cwd,
            axon_time::unix_now_ms(),
            &self.reply_destination,
        )
        .map_err(RuntimeError::runtime)
        .map_err(AgentTuiSubmitFailure::without_output)?
        .ok_or_else(|| {
            AgentTuiSubmitFailure::without_output(RuntimeError::runtime(
                "TUI Interaction 内容不能为空",
            ))
        })?;
        let interaction = prepared.interaction;
        let telemetry_enabled = axon_session_runtime::first_token_telemetry_enabled();
        let ipc_submit_started_at_ms = telemetry_enabled.then(axon_time::unix_now_ms);
        let (session_id, primary_interaction_id) = submit_prepared_daemon_interaction(
            &interaction,
            &prepared.workspace,
            self.config_path.as_deref(),
        )
        .map_err(RuntimeError::runtime)
        .map_err(AgentTuiSubmitFailure::without_output)?;
        let ipc_submit_finished_at_ms = telemetry_enabled.then(axon_time::unix_now_ms);
        let observed_interaction = axon_domain::InteractionRecord {
            interaction_id: primary_interaction_id.clone(),
            ..interaction
        };
        let mut transport_timing = ipc_submit_started_at_ms.zip(ipc_submit_finished_at_ms).map(
            |(started_at_ms, finished_at_ms)| RuntimeTransportTiming {
                interaction_id: primary_interaction_id.as_str().to_owned(),
                ipc_submit_started_at_ms: started_at_ms,
                ipc_submit_finished_at_ms: finished_at_ms,
                interaction_appended_at_ms: finished_at_ms,
                first_ipc_event_received_at_ms: 0,
            },
        );
        let mut observe = |observation| match observation {
            daemon_interaction_client::DaemonTurnObservation::Wire(event) => {
                let mut runtime_event = axon_cli_runtime_event(AgentTuiRuntimePayload::WireEvent {
                    event: Box::new(event),
                });
                if let Some(mut timing) = transport_timing.take() {
                    timing.first_ipc_event_received_at_ms = axon_time::unix_now_ms();
                    runtime_event = runtime_event.with_transport_timing(timing);
                }
                emit.emit(runtime_event);
            }
            daemon_interaction_client::DaemonTurnObservation::SubscriptionPhase(phase) => {
                emit.emit(axon_cli_runtime_event(
                    AgentTuiRuntimePayload::SubscriptionPhase(tui_subscription_phase(phase)),
                ));
            }
            daemon_interaction_client::DaemonTurnObservation::RuntimeObservation(frame) => {
                emit.emit(axon_cli_runtime_event(AgentTuiRuntimePayload::Observation(
                    frame,
                )));
            }
        };
        let projection = daemon_interaction_client::resume_daemon_interaction(
            observed_interaction,
            self.config_path.as_deref(),
            &|| cancel_token.is_cancelled(),
            Some(&mut observe),
        );
        let result = daemon_projection_result(projection.map(|projection| {
            serde_json::json!({
                "sessionId": session_id.as_str(),
                "wireEvents": projection.wire_events,
                "content": projection.response,
            })
        }));
        let output = format_tui_run_output(&result);
        if result.exit_code == 0 {
            Ok(output)
        } else {
            // 失败必须连同已投影的 durable 终态一起交出：实时邮箱淘汰的展示增量
            // 只能由这份终态重放补齐，只上报错误会让正文留下缺口。
            Err(AgentTuiSubmitFailure::with_output(
                runtime_error_from_run_output(&output),
                output,
            ))
        }
    }

    fn steer(
        &self,
        submission: &AgentTuiSubmit,
        _emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiSteerResult, RuntimeError> {
        // 待决问答作答不经 Interaction 提交：它走统一答案入口，由服务端结算并投递，
        // TUI 只负责把作答原样送达。
        if let Some(answer) = submission.question_answer.as_ref() {
            return super::question_answer_submission::submit_question_answer(
                answer,
                self.config_path.as_deref(),
            )
            .map(|_| AgentTuiSteerResult::Accepted)
            .map_err(RuntimeError::runtime);
        }
        let prepared = interaction_from_tui_submit(
            submission,
            &submission.cwd,
            axon_time::unix_now_ms(),
            &self.reply_destination,
        )
        .map_err(RuntimeError::runtime)?
        .ok_or_else(|| RuntimeError::runtime("TUI steer 内容不能为空"))?;
        submit_prepared_daemon_interaction(
            &prepared.interaction,
            &prepared.workspace,
            self.config_path.as_deref(),
        )
        .map(|_| AgentTuiSteerResult::Accepted)
        .map_err(RuntimeError::runtime)
    }
}

/// 请求 Daemon 侧实时重探 MCP 能力：清除健康负缓存并重建该 Workspace 的启动快照。
///
/// `/mcp` 的本地目录刷新不因 Daemon 不可用而失败，因此这里只告警。
fn request_daemon_mcp_refresh(cwd: Option<&std::path::Path>, config_path: &std::path::Path) {
    let Some(cwd) = cwd else {
        return;
    };
    if let Err(error) =
        daemon_interaction_client::refresh_daemon_mcp_capabilities(cwd, Some(config_path))
    {
        axon_logger::warning(&format!("MCP 实时刷新未提交到 Daemon：{error}"));
    }
}

impl CliAgentTuiRuntimeAdapter {}

fn tui_subscription_phase(
    phase: daemon_interaction_client::DaemonSubscriptionPhase,
) -> axon_tui_runtime::DaemonSubscriptionPhase {
    match phase {
        daemon_interaction_client::DaemonSubscriptionPhase::Connected => {
            axon_tui_runtime::DaemonSubscriptionPhase::Connected
        }
        daemon_interaction_client::DaemonSubscriptionPhase::Reconnecting => {
            axon_tui_runtime::DaemonSubscriptionPhase::Reconnecting
        }
        daemon_interaction_client::DaemonSubscriptionPhase::Resyncing => {
            axon_tui_runtime::DaemonSubscriptionPhase::Resyncing
        }
    }
}

/// 把 Daemon 回合结果规范化为 TUI 内部 JSON 运行结果。
///
/// 失败报告与成功报告共用同一 `wireEvents` 契约：实时邮箱只淘汰可重建的展示增量，
/// 被淘汰的增量必须由这份终态按 durable 顺序完整重放补齐。
fn daemon_projection_result(
    result: Result<serde_json::Value, daemon_interaction_client::DaemonTurnFailure>,
) -> CliRunResult {
    match result {
        Ok(value) => CliRunResult {
            exit_code: 0,
            stdout: value.to_string(),
            stderr: String::new(),
        },
        Err(failure) => CliRunResult {
            exit_code: 1,
            stdout: failure.to_report_json(),
            stderr: String::new(),
        },
    }
}

const RUNTIME_OUTPUT_FALLBACK_CODE: &str = "runtime_output_missing_error_line";
const RUNTIME_ERROR_NEXT_STEP: &str =
    "运行时已保留错误上下文，可继续选择其他可用能力或检查结构化详情。";

fn runtime_error_from_run_output(output: &AgentTuiRunOutput) -> RuntimeError {
    output
        .lines()
        .iter()
        .find_map(|line| match line {
            axon_tui_runtime::AgentTuiRunLine::Error(error) => Some(RuntimeError::from(TuiError {
                severity: error.severity,
                action: error.action,
                cell_kind: error.cell_kind,
                message: error.message.clone(),
                error_type: error
                    .error_type
                    .clone()
                    .unwrap_or_else(|| "runtime".to_owned()),
                error_code: error.error_code.clone(),
                next_step: error.next_step.clone(),
                details: runtime_error_details(error),
            })),
            _ => None,
        })
        .unwrap_or_else(|| {
            let message = run_output_error_message(output);
            RuntimeError::from(TuiError {
                severity: axon_tui_runtime::TuiErrorSeverity::UserFacing,
                action: axon_tui_runtime::TuiErrorAction::InspectDetails,
                cell_kind: axon_tui_runtime::TuiErrorCellKind::Runtime,
                message: message.clone(),
                error_type: "runtime".to_owned(),
                error_code: Some(RUNTIME_OUTPUT_FALLBACK_CODE.to_owned()),
                next_step: Some(RUNTIME_ERROR_NEXT_STEP.to_owned()),
                details: vec![format!("原因 {message}")],
            })
        })
}

fn runtime_error_details(error: &axon_tui_runtime::AgentTuiRunError) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(status) = error.http_status {
        details.push(format!("HTTP {status}"));
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
    details.extend(error.channel_attempts.iter().map(|attempt| {
        let mut parts = vec![format!("{}/{}", attempt.provider, attempt.model)];
        if let Some(status) = attempt.http_status {
            parts.push(format!("HTTP {status}"));
        }
        parts.push(attempt.outcome.clone());
        parts.join(" · ")
    }));
    details.dedup();
    details
}

#[cfg(test)]
mod tests {
    use axon_tui_runtime::{AgentTuiRunError, AgentTuiRunLine, AgentTuiRunOutput};

    use super::runtime_error_from_run_output;

    #[test]
    fn runtime_error_from_run_output_preserves_structured_failure_fields() {
        let mut output = AgentTuiRunOutput::empty();
        output.push(AgentTuiRunLine::Error(
            AgentTuiRunError::structured_with_id(
                "error:provider",
                "模型服务暂时不可用",
                Some("provider_transport".to_owned()),
                Some(503),
                Some("service_unavailable".to_owned()),
                None,
            )
            .with_caused_by_runtime_event("runtime-event-42"),
        ));

        let error = runtime_error_from_run_output(&output);
        let tui_error = error.to_tui_error();

        assert_eq!(tui_error.message, "模型服务暂时不可用");
        assert_eq!(tui_error.error_type, "provider_transport");
        assert_eq!(tui_error.error_code.as_deref(), Some("service_unavailable"));
        assert!(tui_error.details.iter().any(|detail| detail == "HTTP 503"));
        assert!(
            tui_error
                .details
                .iter()
                .any(|detail| detail == "事件 runtime-event-42")
        );
    }

    #[test]
    fn runtime_error_from_empty_run_output_keeps_structured_fallback_reason() {
        let output = AgentTuiRunOutput::empty();

        let error = runtime_error_from_run_output(&output);
        let tui_error = error.to_tui_error();

        assert_eq!(tui_error.error_type, "runtime");
        assert_eq!(
            tui_error.error_code.as_deref(),
            Some("runtime_output_missing_error_line")
        );
        assert!(
            tui_error
                .details
                .iter()
                .any(|detail| detail.contains("原因"))
        );
    }
}
