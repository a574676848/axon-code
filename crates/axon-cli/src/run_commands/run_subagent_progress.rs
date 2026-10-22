//! 子代理内部进度到父运行活动事件的投影。

use std::sync::Arc;

use axon_agent_runtime::AGENT_RUN_ERROR_TYPE_INTERRUPTED;
use axon_protocol::wire::{WireEventKind, WireLifecycleStatus, WireProviderStatusPhase};
use axon_session_runtime::{SubagentSessionSinkFactory, TurnEventSink};

use super::{
    RunRequest,
    run_execution::{RunExecutionError, RunResponse},
    run_progress::{RunLifecycleStatusView, RunProgressSink, RunProgressWireEvent},
    run_session_store::runtime_failure_from_execution_error,
    run_subagent_lifecycle::{
        SUBAGENT_LIFECYCLE_KIND, SUBAGENT_LIFECYCLE_TITLE, SubagentLifecyclePublisher,
    },
};

/// 子代理回合终态的状态取值：与主会话 `turn_completed` 载荷同一套口径。
const TURN_STATUS_COMPLETED: &str = "completed";
const TURN_STATUS_FAILED: &str = "failed";
const TURN_STATUS_INTERRUPTED: &str = "interrupted";
/// 中断的展示正文；与主会话收口文案一致，避免把「我让它停的」说成「它没做成」。
const SUBAGENT_INTERRUPTED_RESPONSE: &str = "本轮已由用户中断。";

pub(super) type SubagentProviderTurnRunner =
    for<'a> fn(
        &RunRequest,
        Option<&RunProgressSink<'a>>,
    ) -> Result<RunResponse, Box<RunExecutionError>>;

const PHASE_CONNECTING: &str = "provider_connecting";
const PHASE_RUNNING: &str = "running";
const PHASE_GENERATING: &str = "provider_generating";
const PHASE_RETRYING: &str = "provider_retrying";
const PHASE_PROVIDER_FAILED: &str = "provider_failed";
const PHASE_FAILOVER: &str = "provider_failover";
const PHASE_TOOL_RUNNING: &str = "tool_running";
const PHASE_TOOL_COMPLETED: &str = "tool_completed";
const PROVIDER_MODEL_SEPARATOR: &str = "/";

pub(super) fn run_subagent_provider_turn(
    request: &RunRequest,
    task_path: &axon_agent_runtime::AgentPath,
    started_at_ms: u64,
    model: &str,
    publisher: &SubagentLifecyclePublisher,
    runner: SubagentProviderTurnRunner,
    session_sinks: Option<Arc<dyn SubagentSessionSinkFactory>>,
) -> Result<RunResponse, Box<RunExecutionError>> {
    let mut projection = SubagentProgressProjection::new(
        task_path.as_str().to_owned(),
        started_at_ms,
        model.to_owned(),
    );
    // 子代理是独立会话：原始 wire 事件先落进**它自己**的会话事件流，再做父轮可见性投影。
    // 界面因此能用与普通会话完全相同的一条管线渲染它，而不是另造一套摘要视图。
    //
    // 落库失败只降级为告警：父轮的进度可见性不该因为宿主端存储异常而中断。
    let child_sink =
        session_sinks.and_then(|factory| match factory.sink_for(&request.session_id) {
            Ok(sink) => Some(sink),
            Err(error) => {
                axon_logger::warning(&format!(
                    "子代理会话 sink 不可用，逐字稿将只保留父轮投影: session_id={}, error={error}",
                    request.session_id
                ));
                None
            }
        });
    // 子代理的任务提示就是这条会话的用户消息：与主会话一样，它必须进自己的事件流，
    // 否则界面的归约器打不开回合，整段逐字稿都无处安放。
    if let Some(sink) = &child_sink {
        match subagent_user_message_event(request, started_at_ms) {
            Ok(event) => {
                if let Err(error) = TurnEventSink::publish(sink.as_ref(), &event) {
                    axon_logger::warning(&format!(
                        "子代理用户消息追加失败: session_id={}, error={error}",
                        request.session_id
                    ));
                }
            }
            Err(error) => axon_logger::warning(&format!(
                "子代理用户消息构造失败: session_id={}, error={error}",
                request.session_id
            )),
        }
    }
    let mut emit = |event: RunProgressWireEvent| {
        if let Some(sink) = &child_sink
            && let Ok(runtime_event) =
                axon_session_runtime::wire_event_to_runtime_event(&event.event)
            && let Err(error) = TurnEventSink::publish(sink.as_ref(), &runtime_event)
        {
            axon_logger::warning(&format!(
                "子代理会话事件追加失败: session_id={}, error={error}",
                request.session_id
            ));
        }
        if let Some(view) = projection.project(event) {
            let _ = publisher.publish(view);
        }
    };
    let progress = match crate::runtime_paths::runtime_axon_data_path_static(&request.config_path) {
        Ok(root) => RunProgressSink::with_session_id(&mut emit, request.session_id.clone())
            .with_tool_result_archive_root(root),
        Err(_) => RunProgressSink::with_session_id(&mut emit, request.session_id.clone()),
    };
    let outcome = runner(request, Some(&progress));
    // 回合终态同样属于这条会话：主会话的轮次分割线与指标都来自它，
    // 子代理要「与普通会话完全一致」，就必须和普通会话一样留下这一条。
    if let Some(sink) = &child_sink {
        let terminal = SubagentTurnTerminal::from_outcome(request, &outcome);
        match subagent_turn_terminal_event(request, &terminal) {
            Ok(event) => {
                if let Err(error) = TurnEventSink::publish(sink.as_ref(), &event) {
                    axon_logger::warning(&format!(
                        "子代理回合终态追加失败: session_id={}, error={error}",
                        request.session_id
                    ));
                }
            }
            Err(error) => axon_logger::warning(&format!(
                "子代理回合终态构造失败: session_id={}, error={error}",
                request.session_id
            )),
        }
    }
    outcome
}

/// 子代理回合终态的状态取值；中断与失败必须分开成两种事实。
fn subagent_terminal_status(error_type: &str) -> &'static str {
    if error_type == AGENT_RUN_ERROR_TYPE_INTERRUPTED {
        TURN_STATUS_INTERRUPTED
    } else {
        TURN_STATUS_FAILED
    }
}

/// 子代理回合终态：状态、展示正文与结构化失败回执。
struct SubagentTurnTerminal {
    status: &'static str,
    response: String,
    failure: Option<axon_domain::RuntimeFailure>,
}

impl SubagentTurnTerminal {
    /// 从运行结果推导终态。
    ///
    /// 中断与失败是两种事实：被父轮或用户中止的子代理只记 `interrupted`，不写失败回执；
    /// 只有真正执行失败才带结构化 `failure`，界面据此分别显示「已中断」与失败卡。
    fn from_outcome(
        request: &RunRequest,
        outcome: &Result<RunResponse, Box<RunExecutionError>>,
    ) -> Self {
        match outcome {
            Ok(response) => Self {
                status: TURN_STATUS_COMPLETED,
                response: response.content.clone(),
                failure: None,
            },
            Err(error) if error.error_type == AGENT_RUN_ERROR_TYPE_INTERRUPTED => Self {
                status: subagent_terminal_status(error.error_type),
                response: SUBAGENT_INTERRUPTED_RESPONSE.to_owned(),
                failure: None,
            },
            Err(error) => Self {
                status: subagent_terminal_status(error.error_type),
                response: error.to_string(),
                failure: Some(runtime_failure_from_execution_error(
                    &request.session_id,
                    error,
                    "",
                    super::run_execution::now_ms(),
                )),
            },
        }
    }
}

/// 子代理会话的回合终态事件。
///
/// 与主会话同一种事件类型（`turn_completed`）与载荷口径：界面据此收口回合、
/// 生成轮次分割线并落定耗时，不需要为子代理另写一套收口逻辑。
fn subagent_turn_terminal_event(
    request: &RunRequest,
    terminal: &SubagentTurnTerminal,
) -> Result<axon_session_runtime::RuntimeEvent, String> {
    let mut payload = serde_json::json!({
        "sessionId": request.session_id,
        "response": terminal.response,
        "status": terminal.status,
        // 渠道归因：界面据此在失败卡上指出是哪条渠道与模型失败。
        "provider": request.provider.name,
        "model": request.model.id,
    });
    // 结构化失败只在这里出现一次：正文留一句人话，字段留给界面画卡。
    if let Some(failure) = &terminal.failure {
        payload["failure"] = serde_json::to_value(failure)
            .map_err(|error| format!("子代理失败回执序列化失败：{error}"))?;
    }
    Ok(axon_session_runtime::RuntimeEvent {
        event_type: axon_session_runtime::model::TURN_COMPLETED_EVENT.to_owned(),
        payload_json: serde_json::to_string(&payload)
            .map_err(|error| format!("子代理回合终态序列化失败：{error}"))?,
    })
}

type SubagentProgressSignature = (String, String, String, Option<u64>, Option<u16>);

/// 子代理会话的用户消息事件。
///
/// 与主会话同一种帧：`UserMessage` 载荷承载本轮的任务提示。子代理的会话流
/// 因此和普通会话一样，从一条用户消息开始，界面不需要任何特殊起点逻辑。
fn subagent_user_message_event(
    request: &RunRequest,
    now_ms: u64,
) -> Result<axon_session_runtime::RuntimeEvent, String> {
    use axon_protocol::wire::{
        WireCorrelationId, WireEventEnvelope, WireEventKind, WireSchemaVersion, WireSessionRef,
        WireUserMessage,
    };
    let session_id = request.session_id.clone();
    let envelope = WireEventEnvelope {
        schema_version: WireSchemaVersion::default(),
        event_id: WireCorrelationId(format!("{session_id}:user")),
        correlation_id: WireCorrelationId("user".to_owned()),
        session: WireSessionRef {
            session_id: session_id.clone(),
            run_id: None,
            parent_session_id: request.parent_session_id.clone(),
        },
        sequence: 0,
        recorded_at_ms: i64::try_from(now_ms).unwrap_or(i64::MAX),
        kind: WireEventKind::UserMessage(WireUserMessage {
            part_id: format!("user:{session_id}"),
            message_id: format!("user-message:{session_id}"),
            content: request.prompt.clone(),
            local_files: Vec::new(),
        }),
    };
    axon_session_runtime::wire_event_to_runtime_event(&envelope)
}
struct SubagentProgressProjection {
    task_path: String,
    started_at_ms: u64,
    model: String,
    provider: Option<String>,
    total_tokens: Option<u64>,
    cache_hit_rate_basis_points: Option<u16>,
    last_phase: Option<String>,
    last_detail: Option<String>,
    last_emitted: Option<SubagentProgressSignature>,
}

impl SubagentProgressProjection {
    fn new(task_path: String, started_at_ms: u64, model: String) -> Self {
        Self {
            task_path,
            started_at_ms,
            model,
            provider: None,
            total_tokens: None,
            cache_hit_rate_basis_points: None,
            last_phase: None,
            last_detail: None,
            last_emitted: None,
        }
    }

    fn project(&mut self, event: RunProgressWireEvent) -> Option<RunLifecycleStatusView> {
        match event.event.kind {
            WireEventKind::ProviderStatus(status) => {
                let same_channel = self.provider.as_deref() == Some(status.provider.as_str())
                    && (status.model.trim().is_empty() || self.model == status.model);
                let keep_running = status.phase == WireProviderStatusPhase::Connecting
                    && same_channel
                    && self.last_phase.as_deref().is_some_and(|phase| {
                        !matches!(
                            phase,
                            PHASE_CONNECTING
                                | PHASE_RETRYING
                                | PHASE_PROVIDER_FAILED
                                | PHASE_FAILOVER
                        )
                    });
                self.provider = Some(status.provider);
                if !status.model.trim().is_empty() {
                    self.model = status.model;
                }
                let (phase, detail) = if keep_running {
                    (PHASE_RUNNING, "正在运行")
                } else {
                    provider_phase(status.phase)
                };
                self.activity(phase, detail)
            }
            WireEventKind::UsageUpdate(usage) => {
                let total_tokens = usage.total_tokens();
                if total_tokens > 0 {
                    self.total_tokens = Some(
                        self.total_tokens
                            .map_or(total_tokens, |current| current.max(total_tokens)),
                    );
                    self.cache_hit_rate_basis_points = Some(usage.cache_hit_rate_basis_points());
                }
                self.repeat_current_activity()
            }
            WireEventKind::AssistantMessage(message) if !message.content.is_empty() => {
                self.activity(PHASE_GENERATING, "正在生成回复")
            }
            WireEventKind::ReasoningDelta(delta) if !delta.delta.is_empty() => {
                self.activity(PHASE_GENERATING, "正在思考")
            }
            WireEventKind::ToolCall(call) => {
                let detail = format!("正在执行工具 {}", call.tool_name);
                self.activity(PHASE_TOOL_RUNNING, &detail)
            }
            WireEventKind::ToolResult(result) if result.status != WireLifecycleStatus::Running => {
                let tool_name = result.tool_name.unwrap_or_else(|| "工具".to_owned());
                let detail = if matches!(
                    result.status,
                    WireLifecycleStatus::Failed
                        | WireLifecycleStatus::Aborted
                        | WireLifecycleStatus::Cancelled
                        | WireLifecycleStatus::Blocked
                ) {
                    format!("{tool_name} 未完成，继续处理")
                } else {
                    format!("{tool_name} 已完成，继续处理")
                };
                self.activity(PHASE_TOOL_COMPLETED, &detail)
            }
            _ => None,
        }
    }

    fn repeat_current_activity(&mut self) -> Option<RunLifecycleStatusView> {
        let phase = self
            .last_phase
            .clone()
            .unwrap_or_else(|| PHASE_GENERATING.to_owned());
        let detail = self
            .last_detail
            .clone()
            .unwrap_or_else(|| "正在处理".to_owned());
        self.activity(&phase, &detail)
    }

    fn activity(&mut self, phase: &str, detail: &str) -> Option<RunLifecycleStatusView> {
        let route = self.provider_model_route();
        let signature = (
            phase.to_owned(),
            detail.to_owned(),
            route.clone(),
            self.total_tokens,
            self.cache_hit_rate_basis_points,
        );
        if self.last_emitted.as_ref() == Some(&signature) {
            return None;
        }
        self.last_phase = Some(phase.to_owned());
        self.last_detail = Some(detail.to_owned());
        self.last_emitted = Some(signature);
        let summary = format!("{} {detail}", self.task_path);
        let mut view = RunLifecycleStatusView::with_id(
            subagent_lifecycle_part_id(&self.task_path),
            SUBAGENT_LIFECYCLE_TITLE,
            SUBAGENT_LIFECYCLE_KIND,
            summary,
            phase,
            None,
        )
        .with_started_at_ms(self.started_at_ms)
        .with_tool_name(route);
        if let Some(total_tokens) = self.total_tokens {
            view = view.with_total_tokens(total_tokens);
        }
        if let Some(value) = self.cache_hit_rate_basis_points {
            view = view.with_cache_hit_rate_basis_points(value);
        }
        Some(view)
    }

    fn provider_model_route(&self) -> String {
        self.provider
            .as_deref()
            .filter(|provider| !provider.trim().is_empty())
            .map_or_else(
                || self.model.clone(),
                |provider| format!("{provider}{PROVIDER_MODEL_SEPARATOR}{}", self.model),
            )
    }
}

fn provider_phase(phase: WireProviderStatusPhase) -> (&'static str, &'static str) {
    match phase {
        WireProviderStatusPhase::Connecting => (PHASE_CONNECTING, "正在连接供应商"),
        WireProviderStatusPhase::Connected => (PHASE_RUNNING, "正在运行"),
        WireProviderStatusPhase::Generating => (PHASE_GENERATING, "正在生成回复"),
        WireProviderStatusPhase::Retrying => (PHASE_RETRYING, "正在重试供应商请求"),
        WireProviderStatusPhase::Failed => (PHASE_PROVIDER_FAILED, "供应商请求失败"),
        WireProviderStatusPhase::Failover => (PHASE_FAILOVER, "正在切换供应商"),
    }
}

fn subagent_lifecycle_part_id(task_path: &str) -> String {
    let suffix = task_path.trim_start_matches('/').replace('/', ":");
    format!("lifecycle:{SUBAGENT_LIFECYCLE_KIND}:{suffix}")
}

#[cfg(test)]
#[path = "run_subagent_progress_tests.rs"]
mod tests;
