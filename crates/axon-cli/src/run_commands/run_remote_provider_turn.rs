//! Daemon Provider turn 的准备、执行与结果投影。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axon_agent_runtime::RunCancelToken;
use axon_daemon::PRIMARY_TURN_ACTION_KIND;
use axon_domain::{InteractionId, SYSTEM_SOURCE_CHANNEL};
use axon_protocol::wire::WireEventEnvelope;
use axon_remote_feishu::RemoteFeishuProcessingOutcome;
use axon_session_runtime::{
    ProviderTurnRequest, ProviderTurnResult, ProviderTurnRunner, TurnEventSink, TurnExecutionError,
};

use super::super::run_agent_identity::run_agent_identity;
use super::super::run_steer::{
    ActivePrimaryTurnRegistry, RunSteerBridge, RunSteerInbox, register_deferred_message_session,
};
use super::super::run_stream_provider_progress;
use super::super::run_subagent_executor::{SUBAGENT_RESULT_MARKER_TEXT, strip_result_path};
use super::super::{
    CliRunResult, RunCommand, RunStartupSnapshot, StartupSnapshotCache, execute_standalone_subagent,
};
use super::{
    FeishuTurnContext, complete_feishu_processing, resolve_feishu_turn_context, transport_files,
};
use crate::OutputFormat;

/// Provider turn 失败明细的最大字符数，避免超长 stderr 污染日志与账本。
const PROVIDER_FAILURE_DETAIL_MAX_CHARS: usize = 600;
/// Provider turn 失败且无任何输出时的占位说明。
const PROVIDER_FAILURE_EMPTY_DETAIL: &str = "无输出";

/// CLI 组合根提供的 Provider runner；Session Runtime 不依赖该类型。
#[derive(Clone)]
pub(crate) struct CliProviderTurnRunner {
    pub(crate) config_path: PathBuf,
    pub(crate) active_primary_turns: ActivePrimaryTurnRegistry,
    /// 进程级启动快照缓存；Daemon 长驻场景据此跳过每轮的全量快照构建。
    pub(crate) startup_snapshots: StartupSnapshotCache,
    /// 主 turn 专属的用户提问端口；子代理与后台执行不装配（保持 `None`）。
    pub(crate) user_question_port: Option<Arc<dyn axon_agent_runtime::UserQuestionPort>>,
}

impl ProviderTurnRunner for CliProviderTurnRunner {
    fn run(&self, request: ProviderTurnRequest) -> Result<ProviderTurnResult, TurnExecutionError> {
        self.run_inner(request, None)
    }

    fn run_streaming(
        &self,
        request: ProviderTurnRequest,
        sink: Option<&dyn TurnEventSink>,
    ) -> Result<ProviderTurnResult, TurnExecutionError> {
        self.run_inner(request, sink)
    }

    fn run_local_action(
        &self,
        _request: ProviderTurnRequest,
    ) -> Result<ProviderTurnResult, TurnExecutionError> {
        Err(TurnExecutionError::permanent("本地 Action 类型已废弃"))
    }
}

impl CliProviderTurnRunner {
    /// 单次 Provider turn 的共用实现；注入 `sink` 时在运行过程中逐条实时投影运行事件。
    fn run_inner(
        &self,
        request: ProviderTurnRequest,
        sink: Option<&dyn TurnEventSink>,
    ) -> Result<ProviderTurnResult, TurnExecutionError> {
        let started_at_ms = crate::run_commands::run_execution::now_ms();
        if request.action_kind != PRIMARY_TURN_ACTION_KIND {
            let task_path = axon_agent_runtime::AgentPath::parse(format!(
                "/root/task-{}",
                sanitize_agent_path_segment(&request.session_id)
            ))
            .map_err(|error| TurnExecutionError::permanent(format!("{error:?}")))?;
            let output = execute_standalone_subagent(
                &self.config_path,
                Path::new(&request.cwd),
                &request.session_id,
                task_path,
                &request.prompt,
                started_at_ms,
            )
            .map_err(TurnExecutionError::retryable)?;
            let transcript_ref = output
                .summary
                .split_once(SUBAGENT_RESULT_MARKER_TEXT)
                .and_then(|(_, path)| path.lines().next())
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_owned);
            let token_usage = output.token_usage.map(|usage| {
                usage
                    .total_input_tokens
                    .saturating_add(usage.output_tokens)
                    .saturating_add(usage.reasoning_output_tokens)
            });
            return Ok(ProviderTurnResult {
                response: strip_result_path(&output.summary),
                wire_events: Vec::new(),
                continuation: None,
                transcript_ref,
                token_usage,
                time_usage_ms: None,
                agent_tree_summary: None,
                tool_receipt_sequence: None,
            });
        }
        axon_logger::debug(&format!(
            "Daemon Provider runner 开始: session_id={}, run_session_id={}, prompt_bytes={}",
            request.session_id,
            request.run_session_id,
            request.prompt.len()
        ));
        // 实时投影是可降级能力：配置、凭据或会话映射异常只能退化为持久 outbox 出站，
        // 绝不允许拒绝整轮用户 turn，否则飞书侧会完全收不到回复。
        let feishu = match resolve_feishu_turn_context(&self.config_path, &request) {
            Ok(context) => context,
            Err(error) => {
                axon_logger::warning(&format!(
                    "Feishu 实时投影不可用，本轮仅由持久 outbox 出站: {error}"
                ));
                None
            }
        };
        // Provider 请求发出前的失败不会产生外部副作用，配置或输入错误直接结论为确定性失败。
        let command = provider_run_command(&self.config_path, &request)
            .map_err(TurnExecutionError::permanent)?;
        // 快照是纯加速能力：缓存不可用或命中失败时，主路径仍会现场构建，语义不变。
        let startup_snapshot = self
            .startup_snapshots
            .acquire(&self.config_path, Path::new(&request.cwd));
        let run_session_id = command.session.clone().unwrap_or_default();
        // 该边界内的失败只会发生在注册与准备阶段，尚未发出 Provider 请求，可安全重试。
        let run_result = execute_provider_command(
            &self.config_path,
            &request,
            &command,
            startup_snapshot,
            sink,
            &self.active_primary_turns,
            self.user_question_port.clone(),
        )
        .map_err(TurnExecutionError::retryable)?;
        // Provider turn 已经执行，之后的收尾失败无法证明外部结果，必须进入 uncertain。
        let run_result =
            finalize_provider_run(feishu.as_ref(), run_result, &command, started_at_ms)
                .map_err(TurnExecutionError::external_outcome_unknown)?;
        build_provider_turn_result(&self.config_path, &run_session_id, &run_result)
            .map_err(TurnExecutionError::external_outcome_unknown)
    }
}

fn provider_run_command(
    config_path: &Path,
    request: &ProviderTurnRequest,
) -> Result<RunCommand, String> {
    let fixed = request
        .transport_context
        .as_ref()
        .and_then(|context| context.get("destination"))
        .and_then(serde_json::Value::as_str)
        .map(|destination| crate::cron_commands::cron_agent_config(config_path, destination))
        .transpose()?
        .flatten();
    Ok(RunCommand {
        prompt: request.prompt.clone(),
        model: fixed.as_ref().map(|(model, _)| model.clone()),
        reasoning_effort: fixed.map(|(_, effort)| effort),
        format: Some(OutputFormat::Json),
        continue_last: false,
        session: Some(request.run_session_id.clone()),
        file: transport_files(request.transport_context.as_ref()),
        timeout: None,
        tool: Vec::new(),
    })
}

fn execute_provider_command(
    config_path: &Path,
    request: &ProviderTurnRequest,
    command: &RunCommand,
    startup_snapshot: Option<Arc<RunStartupSnapshot>>,
    sink: Option<&dyn TurnEventSink>,
    active_primary_turns: &ActivePrimaryTurnRegistry,
    user_question_port: Option<Arc<dyn axon_agent_runtime::UserQuestionPort>>,
) -> Result<CliRunResult, String> {
    let agent_identity = run_agent_identity(request);
    let steer_bridge = RunSteerBridge::default();
    let steer_inbox = steer_bridge.open();
    // 本轮执行的取消令牌：注册进活跃主 turn，网关中断即置位它。
    let cancel_token = RunCancelToken::default();
    let _deferred_registration =
        register_deferred_message_session(&request.session_id, steer_inbox.clone());
    let _completion_notice_subscription =
        super::super::run_completion_notice_subscription::subscribe_run_completion_notices(
            &request.session_id,
        );
    let _session_reclaim_subscription =
        super::super::run_completion_notice_subscription::subscribe_run_shell_session_reclaims(
            &request.session_id,
        );
    if let Err(error) = super::super::run_continuation_assembly::assemble_pending_continuations(
        Some(config_path),
        &request.session_id,
        crate::run_commands::run_execution::now_ms(),
    ) {
        axon_logger::warning(&format!(
            "子代理终态装配失败，事实保留待下次装配: session={}, error={error}",
            request.session_id
        ));
    }
    let _active_registration = active_turn_registration(
        request,
        active_primary_turns,
        steer_inbox.clone(),
        cancel_token.clone(),
    )?;
    Ok(run_stream_provider_progress::run_with_progress(
        config_path,
        request,
        command,
        startup_snapshot,
        sink,
        steer_inbox,
        agent_identity,
        cancel_token,
        user_question_port,
    ))
}

fn active_turn_registration(
    request: &ProviderTurnRequest,
    active_primary_turns: &ActivePrimaryTurnRegistry,
    steer_inbox: RunSteerInbox,
    cancel_token: RunCancelToken,
) -> Result<Option<super::super::run_steer::ActivePrimaryTurnRegistration>, String> {
    if request.action_kind != PRIMARY_TURN_ACTION_KIND {
        return Ok(None);
    }
    let Some(context) = request.transport_context.as_ref() else {
        return Ok(None);
    };
    let channel = context
        .get("channel")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let interaction_id = context
        .get("interactionId")
        .and_then(serde_json::Value::as_str)
        .map(InteractionId::new)
        .transpose()
        .map_err(|error| error.to_string())?;
    let primary_lane_id = context
        .get("primaryLaneId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let (Some(primary_lane_id), Some(interaction_id)) = (primary_lane_id, interaction_id) else {
        return Ok(None);
    };
    if channel == Some(SYSTEM_SOURCE_CHANNEL) {
        return Ok(None);
    }
    let affinity_key = ActivePrimaryTurnRegistry::affinity_key(primary_lane_id);
    active_primary_turns
        .register(&affinity_key, interaction_id, steer_inbox, cancel_token)
        .map(Some)
}

fn finalize_provider_run(
    feishu: Option<&FeishuTurnContext>,
    run_result: CliRunResult,
    command: &RunCommand,
    started_at_ms: u64,
) -> Result<CliRunResult, String> {
    if let Some(context) = feishu {
        let outcome = if run_result.exit_code == 0 {
            RemoteFeishuProcessingOutcome::Success
        } else {
            RemoteFeishuProcessingOutcome::Failure
        };
        complete_feishu_processing(context, outcome);
    }
    axon_logger::debug(&format!(
        "Daemon Provider runner 返回: run_session_id={}, exit_code={}, elapsed_ms={}",
        command.session.clone().unwrap_or_default(),
        run_result.exit_code,
        crate::run_commands::run_execution::now_ms().saturating_sub(started_at_ms)
    ));
    if run_result.exit_code != 0 {
        return Err(format!(
            "Provider turn 执行失败，退出码 {}：{}",
            run_result.exit_code,
            provider_failure_detail(&run_result)
        ));
    }
    Ok(run_result)
}

/// 失败回执必须带上可诊断细节，否则 Daemon 侧只能看到裸退出码。
///
/// 结构化载荷（CLI 的 `run_failed` JSON）先按诊断字段重建成**完整可解析**的紧凑 JSON，
/// 再交给截断逻辑；直接截断整段会把 JSON 切成半截，下游解析失败就只能把原始载荷
/// 甩给用户看（历史上正是如此）。
fn provider_failure_detail(run_result: &CliRunResult) -> String {
    let detail = if run_result.stderr.trim().is_empty() {
        run_result.stdout.as_str()
    } else {
        run_result.stderr.as_str()
    };
    let compact = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return PROVIDER_FAILURE_EMPTY_DETAIL.to_owned();
    }
    if let Some(structured) = compact_structured_failure(&compact) {
        return structured;
    }
    if compact.chars().count() <= PROVIDER_FAILURE_DETAIL_MAX_CHARS {
        return compact;
    }
    let mut truncated = compact
        .chars()
        .take(PROVIDER_FAILURE_DETAIL_MAX_CHARS)
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

/// 把 `run_failed` 载荷重建成只含诊断字段的紧凑 JSON；不是结构化载荷时返回 None。
fn compact_structured_failure(detail: &str) -> Option<String> {
    let start = detail.find('{')?;
    let prefix = detail[..start].trim();
    let value = serde_json::from_str::<serde_json::Value>(&detail[start..]).ok()?;
    if value.get("event").and_then(serde_json::Value::as_str) != Some("run_failed") {
        return None;
    }
    let failure = value.get("failure").cloned().unwrap_or_default();
    let mut compact_failure = serde_json::Map::new();
    for field in [
        "category",
        "code",
        "providerProtocol",
        "httpStatus",
        "retryable",
        "replaySafe",
        "attemptCount",
        "providerEventType",
        "upstreamRequestId",
    ] {
        if let Some(field_value) = failure.get(field) {
            compact_failure.insert(field.to_owned(), field_value.clone());
        }
    }
    // 只有这一项可能很长：它是给人看的结论，单独收敛到既有上限。
    compact_failure.insert(
        "redactedMessage".to_owned(),
        serde_json::Value::String(bounded_text(
            failure
                .get("redactedMessage")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        )),
    );
    let mut compact_value = serde_json::Map::new();
    compact_value.insert(
        "event".to_owned(),
        serde_json::Value::String("run_failed".to_owned()),
    );
    for field in ["provider", "model", "httpStatus"] {
        if let Some(field_value) = value.get(field) {
            compact_value.insert(field.to_owned(), field_value.clone());
        }
    }
    // 告警属于本轮诊断事实，压载时必须原样保留，否则客户端永远看不到。
    if let Some(warnings) = value.get("warnings") {
        compact_value.insert("warnings".to_owned(), warnings.clone());
    }
    compact_value.insert(
        "failure".to_owned(),
        serde_json::Value::Object(compact_failure),
    );
    let payload = serde_json::to_string(&serde_json::Value::Object(compact_value)).ok()?;
    Some(if prefix.is_empty() {
        payload
    } else {
        format!("{prefix} {payload}")
    })
}

/// 单字段长度收敛：超出按字符截断并补省略号。
fn bounded_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= PROVIDER_FAILURE_DETAIL_MAX_CHARS {
        return trimmed.to_owned();
    }
    let mut truncated = trimmed
        .chars()
        .take(PROVIDER_FAILURE_DETAIL_MAX_CHARS)
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn build_provider_turn_result(
    config_path: &Path,
    run_session_id: &str,
    run_result: &CliRunResult,
) -> Result<ProviderTurnResult, String> {
    let token_usage =
        match super::super::run_rollout_store::load_session_usage(config_path, run_session_id) {
            Ok(usage) => usage,
            Err(error) => {
                axon_logger::warning(&format!(
                    "读取已持久化 token usage 失败: session_id={run_session_id}, error={error}"
                ));
                None
            }
        };
    let payload = serde_json::from_str::<serde_json::Value>(&run_result.stdout)
        .map_err(|error| format!("解析 Provider JSON 输出失败：{error}"))?;
    let wire_events = extract_wire_events(&payload)?;
    let cumulative_response = payload
        .get("content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Ok(ProviderTurnResult {
        response: cumulative_response.to_owned(),
        wire_events,
        continuation: None,
        transcript_ref: payload
            .get("transcriptRef")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        token_usage,
        time_usage_ms: payload
            .get("timeUsageMs")
            .and_then(serde_json::Value::as_u64),
        agent_tree_summary: payload
            .get("agentTreeSummary")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        tool_receipt_sequence: payload
            .get("toolReceiptSequence")
            .and_then(serde_json::Value::as_u64),
    })
}

fn extract_wire_events(payload: &serde_json::Value) -> Result<Vec<WireEventEnvelope>, String> {
    payload
        .get("wireEvents")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map(|events| events.unwrap_or_default())
        .map_err(|error| format!("解析 Provider wireEvents 失败：{error}"))
}

fn sanitize_agent_path_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "task".to_owned()
    } else {
        sanitized
    }
}

#[cfg(test)]
#[path = "run_remote_provider_turn_tests.rs"]
mod tests;
