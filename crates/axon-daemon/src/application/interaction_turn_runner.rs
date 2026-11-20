//! Daemon Interaction 到 Primary Turn 与 Work Ledger 的执行边界。

use super::super::egress_bridge::CommittedEventEgress;
use super::super::event_stream_bridge::EventStreamBridge;
use super::super::primary_turn_service::{
    PrimaryTurnRequest, PrimaryTurnRunner, PrimaryTurnRunnerError,
};
use super::completion_delivery::{completion_notification_id, completion_steer_execution_id};
use super::daemon_host_projection::queued_action;
#[path = "child_session_event_sink.rs"]
pub(super) mod child_session_event_sink;
#[path = "interaction_turn_event_sink.rs"]
mod interaction_turn_event_sink;
#[path = "interaction_turn_generation_watch.rs"]
mod interaction_turn_generation_watch;
#[path = "interaction_turn_terminal.rs"]
mod interaction_turn_terminal;
use axon_crypto::sha256_hex;
use axon_domain::{
    DaemonSessionId, ExecutionRecord, ExecutionStatus, GoalStatus, InteractionRecord,
    SessionActionId,
};
use axon_session::{DaemonHostRecord, RuntimeStore, SessionRuntimeRecord, interaction_session_id};
/// Daemon Primary Turn 使用的稳定 Action 类型。
///
/// 取值定义在会话运行时一侧（`axon_session_runtime::PRIMARY_TURN_ACTION_KIND`），
/// 这里只做转发：主 turn 判据不允许在两处各写一份字面量。
pub use axon_session_runtime::PRIMARY_TURN_ACTION_KIND;
use axon_session_runtime::model::{RuntimeEvent, TURN_COMPLETED_EVENT};
use axon_session_runtime::{
    ActionExecutionRequest, AtomicTurnPerformanceRecorder, EventPublisher, RuntimeClock,
    SessionExecutionContext, SubagentSessionSinkFactory, TurnEventSink, TurnExecutor,
    TurnPerformanceRecorder, TurnPerformanceStage,
};
use child_session_event_sink::ChildSessionSinkFactory;
use interaction_turn_event_sink::RuntimeTurnEventSink;
use interaction_turn_terminal::{FailedTurnTerminalInput, failed_turn_terminal_events};
use interaction_turn_terminal::{TurnFailureKind, classify_turn_failure};
use std::sync::Arc;
const PRIMARY_ACTION_ID_PREFIX: &str = "interaction-action-";
/// 用户消息事件的关联标识。
const USER_CORRELATION_ID: &str = "user";
/// 目标执行失败后的退避间隔。
const GOAL_EXECUTION_RETRY_BACKOFF_MS: u64 = 30_000;
/// 目标执行的最大尝试轮数。
const GOAL_EXECUTION_MAX_ATTEMPTS: u32 = 3;
/// 目标更新轮次的来源前缀：与目标工具写入侧同源。
const GOAL_UPDATE_SOURCE_PREFIX: &str = "goal-update:";
/// 目标阶段推进轮次的来源前缀：与阶段续跑写入侧同源。
const GOAL_STAGE_SOURCE_PREFIX: &str = "goal-stage:";
#[derive(Clone)]
pub(super) struct InteractionTurnRunner<S, E, P, C> {
    pub(super) store: S,
    pub(super) executor: E,
    pub(super) publisher: P,
    pub(super) stream_bridge: Arc<EventStreamBridge>,
    pub(super) host: DaemonHostRecord,
    pub(super) clock: C,
    pub(super) now_ms: u64,
}

impl<S, E, P, C> PrimaryTurnRunner for InteractionTurnRunner<S, E, P, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    E: TurnExecutor + Clone + Send + Sync + 'static,
    P: EventPublisher + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
{
    fn run(&self, request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError> {
        let lease_token = request.lease_token.clone();
        // 首字与速率是产品指标，不是诊断开关：无条件采集，
        // 否则用户必须把日志级别开到 trace 才能在界面上看到这两个数。
        let performance = {
            let recorder = Arc::new(AtomicTurnPerformanceRecorder::new());
            recorder.record(TurnPerformanceStage::WorkerClaimed);
            Some(recorder)
        };
        let (interaction, session_id, execution_request) = self.execution_request(request)?;
        // 目标执行轮在账本上的那一行随本轮前进：入站即认领，收口即结算。
        // 一轮只对应一条 Interaction，唯一性由入站主键保证，账本不再需要自己的租约。
        let goal_execution = self.goal_execution_record(&interaction);
        if let Some(execution) = goal_execution.as_ref() {
            let _ = self
                .store
                .begin_execution(&execution.execution_id, self.clock.now_ms());
        }
        let transport_context = interaction_transport_context(&interaction);
        // 运行过程中的逐条投影与收口时的批量投影共用同一条落库/发布路径，
        // 使订阅者在 Provider 连接、工具调用与回复生成的当下即可见。
        // 子代理是独立会话：这里把宿主侧的 sink 工厂交给本轮，
        // 运行侧再把它转交给每个子代理，使子代理的逐字稿落进它自己的会话事件流。
        let subagent_session_sinks: Arc<dyn SubagentSessionSinkFactory> =
            Arc::new(ChildSessionSinkFactory {
                store: self.store.clone(),
                host: self.host.clone(),
                clock: self.clock.clone(),
                stream_bridge: Arc::clone(&self.stream_bridge),
            });
        let sink = RuntimeTurnEventSink::new(
            &self.store,
            CommittedEventEgress::new(&self.publisher, self.stream_bridge.as_ref()),
            &self.host,
            &session_id,
            &interaction.interaction_id,
            &lease_token,
            &self.clock,
            performance.clone(),
            subagent_session_sinks,
        );
        let action_id = execution_request.action.action_id.to_string();
        let action_kind = execution_request.action.kind.clone();
        // 用户输入是本轮的第一个事实：随事件流一起落库，订阅端与回放端才有同一条
        // 事实源，不必再回头查历史索引。附件随消息走，避免正文与素材分成两条路径。
        publish_user_message(&sink, &session_id, &interaction, self.clock.now_ms())?;
        let started_at = std::time::Instant::now();
        let outcome = match self
            .executor
            .execute_streaming(execution_request, Some(&sink))
        {
            Ok(outcome) => {
                let _ = sink.flush();
                outcome
            }
            Err(error) => {
                let _ = sink.flush();
                let message = error.into_message();
                let (kind, response, report) = classify_turn_failure(&message);
                let events = failed_turn_terminal_events(FailedTurnTerminalInput {
                    action_id: &action_id,
                    action_kind: &action_kind,
                    session_id: session_id.as_str(),
                    kind,
                    response: &response,
                    failure: report,
                    transport_context: transport_context.clone(),
                    recorded_at_ms: self.clock.now_ms(),
                    // 失败回执里可能带着 CLI 的降级告警，原样透传给客户端。
                    warnings: &failure_warnings(&message),
                })
                .map_err(runtime_error)?;
                self.persist_and_publish(
                    &session_id,
                    &interaction.interaction_id,
                    &lease_token,
                    events,
                    performance.as_deref(),
                    None,
                    None,
                )?;
                if kind == TurnFailureKind::Interrupted {
                    self.settle_goal_execution(goal_execution.as_ref(), ExecutionStatus::Cancelled);
                    return Err(PrimaryTurnRunnerError::Interrupted(response));
                }
                self.retry_goal_execution(goal_execution.as_ref());
                return Err(PrimaryTurnRunnerError::Provider(message));
            }
        };
        // 收尾观测帧：把这一轮的生成速率与用量补齐。
        // 客户端只在轮次终态之前接受观测帧，因此必须早于下面的 persist_and_publish。
        let total_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let generation = publish_turn_performance(
            &sink,
            &interaction.interaction_id,
            total_ms,
            self.clock.now_ms(),
        );
        let summary = completion_summary_from_events(&outcome.events);
        let next_stage = outcome.next_execution;
        // 终态与它派生的指标同批落盘：`session_metrics` 由 `turn_metrics` 累加，
        // 缺了它 header 的会话累计耗时与 token 读数整段消失。
        let turn_metrics = sink.turn_metrics_event();
        self.persist_and_publish(
            &session_id,
            &interaction.interaction_id,
            &lease_token,
            outcome.events,
            performance.as_deref(),
            generation,
            turn_metrics,
        )?;
        // 目标轮声明了下一阶段时不终结目标：下一阶段就是一条新的入站事实。
        let continued = next_stage.is_some_and(|next| {
            self.continue_goal_execution(&interaction, &session_id, goal_execution.as_ref(), next)
        });
        if !continued {
            self.complete_goal_execution(goal_execution.as_ref());
        }
        self.settle_completion_delivery(&interaction, summary.as_deref());
        Ok(())
    }
}

impl<S, E, P, C> InteractionTurnRunner<S, E, P, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    P: EventPublisher,
    C: RuntimeClock,
{
    /// 本轮对应的执行账本行：只有目标执行轮才有。
    ///
    /// 按标识直取，不按会话回落：`work_items` 同一张在途表里还有入站轮次的行，
    /// 按会话取最近一条可能把本轮自己的 turn 行当成账本行，进而改坏它的处理状态。
    fn goal_execution_record(&self, interaction: &InteractionRecord) -> Option<ExecutionRecord> {
        if interaction.source.client() != Some(axon_session_runtime::ORIGIN_CHANNEL_GOAL) {
            return None;
        }
        let interaction_id = interaction.interaction_id.as_str();
        for candidate in [
            interaction_id.to_owned(),
            retry_base_execution_id(interaction_id),
        ] {
            if let Ok(execution_id) = axon_domain::ExecutionId::new(candidate)
                && let Ok(Some(record)) = self.store.get_execution(&execution_id)
            {
                return Some(record);
            }
        }
        // 更新与阶段推进是同一目标的后继轮次：它们没有自己的账本行，按目标取最近一条。
        // 这些轮次收口时也要能把目标从进行中推到已完成，缺了这一步目标会一直停在执行态。
        let goal_id = derived_goal_id(interaction.source.source_message_id())?;
        self.store
            .list_executions_for_goal(&goal_id, 1)
            .ok()?
            .into_iter()
            .next()
    }

    /// 目标轮声明了下一阶段：把下一阶段要求提交成一条新 Interaction，本阶段不终结目标。
    ///
    /// 多阶段目标与首次执行走同一条入站事实通道，因此阶段推进不需要任何后台调度：
    /// 下一轮由主 turn 领取，正文、指标与终态与首阶段完全同源。
    fn continue_goal_execution(
        &self,
        interaction: &InteractionRecord,
        session_id: &DaemonSessionId,
        execution: Option<&ExecutionRecord>,
        next: axon_session_runtime::NextExecutionRequest,
    ) -> bool {
        let Some(execution) = execution else {
            return false;
        };
        let prompt = serde_json::from_str::<serde_json::Value>(&next.payload_json)
            .ok()
            .and_then(|payload| {
                payload
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|prompt| !prompt.trim().is_empty());
        let Some(prompt) = prompt else {
            return false;
        };
        let stage = next.stage.unwrap_or_else(|| "next".to_owned());
        let source = axon_domain::InteractionSource::new(
            interaction.source.channel(),
            format!("goal-stage:{}:{stage}", execution.goal_id.as_str()),
        )
        .and_then(|source| source.with_client(axon_session_runtime::ORIGIN_CHANNEL_GOAL));
        let Ok(source) = source else {
            return false;
        };
        let Ok(route) =
            axon_domain::ReplyRoute::new(interaction.source.channel(), session_id.as_str())
        else {
            return false;
        };
        let Ok(record) = InteractionRecord::new(
            session_id.clone(),
            axon_session::new_interaction_id(),
            source,
            interaction.scope.clone(),
            interaction.audience.clone(),
            route,
            prompt,
            self.clock.now_ms(),
        ) else {
            return false;
        };
        self.store.append(&record).is_ok()
    }

    /// 目标执行成功：账本结算，目标推进到已完成。
    ///
    /// 完成通知由 supervisor 的补偿扫描按「已终结且未通知」派生，因此这里不直接发通知：
    /// 一条路径只写一次事实，崩溃窗口也由同一处扫描补齐。
    fn complete_goal_execution(&self, execution: Option<&ExecutionRecord>) {
        let Some(execution) = execution else {
            return;
        };
        let now_ms = self.clock.now_ms();
        self.settle_goal_execution(Some(execution), ExecutionStatus::Completed);
        let Ok(Some(mut goal)) = self.store.get_goal(&execution.goal_id) else {
            return;
        };
        if goal.status.is_terminal() {
            return;
        }
        let expected_revision = goal.revision;
        if goal.transition(GoalStatus::Completed, now_ms).is_err() {
            return;
        }
        if let Err(error) = self.store.update_goal(&goal, expected_revision) {
            axon_logger::warning(&format!(
                "目标状态结算失败: goal={}, error={error}",
                goal.goal_id.as_str()
            ));
        }
    }

    /// 把目标执行轮的终态写回账本。
    ///
    /// 账本写失败只影响看板读数，不得改变本轮已经产生的正文与终态，因此这里只告警。
    fn settle_goal_execution(&self, execution: Option<&ExecutionRecord>, status: ExecutionStatus) {
        let Some(execution) = execution else {
            return;
        };
        if let Err(error) =
            self.store
                .finish_execution(&execution.execution_id, status, self.clock.now_ms())
        {
            axon_logger::warning(&format!(
                "目标执行账本结算失败: execution={}, error={error}",
                execution.execution_id.as_str()
            ));
        }
    }

    /// 目标执行失败：未达上限时排一次退避重投，达到上限时落 failed。
    ///
    /// 重试不重新领取旧轮，而是到期后再提交一条新 Interaction，走同一套领取与执行路径。
    fn retry_goal_execution(&self, execution: Option<&ExecutionRecord>) {
        let Some(execution) = execution else {
            return;
        };
        let now_ms = self.clock.now_ms();
        if let Err(error) = self.store.retry_execution(
            &execution.execution_id,
            now_ms.saturating_add(GOAL_EXECUTION_RETRY_BACKOFF_MS),
            GOAL_EXECUTION_MAX_ATTEMPTS,
            now_ms,
        ) {
            axon_logger::warning(&format!(
                "目标执行重试排期失败: execution={}, error={error}",
                execution.execution_id.as_str()
            ));
        }
    }

    fn execution_request(
        &self,
        request: PrimaryTurnRequest,
    ) -> Result<(InteractionRecord, DaemonSessionId, ActionExecutionRequest), PrimaryTurnRunnerError>
    {
        let interaction = request.interaction;
        let interaction_id = interaction.interaction_id.as_str();
        let session_id = interaction_session_id(&interaction);
        let session_id =
            DaemonSessionId::new(session_id).map_err(|error| runtime_error(error.to_string()))?;
        let runtime = self.open_runtime(&interaction, &session_id)?;
        let action_id = SessionActionId::new(stable_interaction_id(
            PRIMARY_ACTION_ID_PREFIX,
            interaction_id,
        ))
        .map_err(runtime_error)?;
        let execution_request = ActionExecutionRequest {
            action: queued_action(
                action_id,
                session_id.clone(),
                PRIMARY_TURN_ACTION_KIND.to_owned(),
                interaction_payload(&interaction),
            ),
            context: SessionExecutionContext {
                run_session_id: runtime.current_run_id,
                cwd: runtime.cwd,
                context_version: runtime.context_version,
            },
            runtime_profile: request.runtime_profile,
            // 本轮的用户消息（含附件）已在上面写入事件流，执行器不再重复写。
            record_turn_input: false,
        };
        Ok((interaction, session_id, execution_request))
    }

    fn open_runtime(
        &self,
        interaction: &InteractionRecord,
        session_id: &DaemonSessionId,
    ) -> Result<SessionRuntimeRecord, PrimaryTurnRunnerError> {
        let workspace_id = interaction
            .scope
            .workspace_id()
            .ok_or_else(|| runtime_error("Primary Turn 必须绑定 Workspace"))?;
        let workspace = self
            .store
            .get_workspace(workspace_id)
            .map_err(runtime_error)?
            .ok_or_else(|| runtime_error("Interaction 对应 Workspace 未注册"))?;
        let mut runtime = self
            .store
            .create_session(session_id, &workspace.workspace_root, self.now_ms)
            .map_err(runtime_error)?;
        // 会话 cwd 只是恢复提示，不是身份的一部分：历史行可能在建行时留空（网关固定会话
        // 就不绑定项目工作区）。Provider 需要一个可访问的工作目录，留空会整轮失败，
        // 因此这一轮按该 Interaction 所属 Workspace 的根目录补齐。
        runtime.cwd = effective_cwd(&runtime.cwd, &workspace.workspace_root);
        Ok(runtime)
    }
}

/// 本轮实际使用的工作目录：会话行有值就用它，留空回落到所属 Workspace 的根目录。
fn effective_cwd(session_cwd: &str, workspace_root: &str) -> String {
    if session_cwd.trim().is_empty() {
        workspace_root.to_owned()
    } else {
        session_cwd.to_owned()
    }
}

/// 把本轮的用户输入写成事件流的第一条事实。
///
/// 事件流此前只承载运行过程，用户气泡只能回查历史索引；这条事件让「用户说了什么、
/// 带了哪些附件」与助手输出落在同一条时间线上，回放因此不再需要第二条数据源。
fn publish_user_message(
    sink: &RuntimeTurnEventSink<'_, impl RuntimeStore, impl EventPublisher, impl RuntimeClock>,
    session_id: &DaemonSessionId,
    interaction: &InteractionRecord,
    now_ms: u64,
) -> Result<(), PrimaryTurnRunnerError> {
    let interaction_id = interaction.interaction_id.as_str();
    let envelope = axon_protocol::wire::WireEventEnvelope {
        schema_version: axon_protocol::wire::WireSchemaVersion::default(),
        event_id: axon_protocol::wire::WireCorrelationId(format!("{interaction_id}:user")),
        correlation_id: axon_protocol::wire::WireCorrelationId(USER_CORRELATION_ID.to_owned()),
        session: axon_protocol::wire::WireSessionRef {
            session_id: session_id.as_str().to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 0,
        recorded_at_ms: i64::try_from(now_ms).unwrap_or(i64::MAX),
        kind: axon_protocol::wire::WireEventKind::UserMessage(
            axon_protocol::wire::WireUserMessage {
                part_id: format!("user:{interaction_id}"),
                message_id: format!("user-message:{interaction_id}"),
                content: interaction.content.clone(),
                local_files: interaction.attachments.clone(),
            },
        ),
    };
    let event = axon_session_runtime::wire_event_to_runtime_event(&envelope)
        .map_err(PrimaryTurnRunnerError::Runtime)?;
    TurnEventSink::publish(sink, &event).map_err(PrimaryTurnRunnerError::Runtime)
}

impl<S, E, P, C> InteractionTurnRunner<S, E, P, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    P: EventPublisher,
    C: RuntimeClock,
{
    fn persist_and_publish(
        &self,
        session_id: &DaemonSessionId,
        interaction_id: &axon_domain::InteractionId,
        lease_token: &str,
        events: Vec<RuntimeEvent>,
        performance: Option<&AtomicTurnPerformanceRecorder>,
        generation: Option<TurnGenerationMetrics>,
        turn_metrics: Option<(String, String)>,
    ) -> Result<(), PrimaryTurnRunnerError> {
        let performance = performance.map(AtomicTurnPerformanceRecorder::snapshot);
        let payloads = events
            .into_iter()
            .map(|event| {
                let event_performance = if event.event_type == TURN_COMPLETED_EVENT {
                    performance.as_ref()
                } else {
                    None
                };
                // 生成指标同样只挂收尾事件：它描述的是这一轮，而不是某个中间步骤。
                let event_generation = if event.event_type == TURN_COMPLETED_EVENT {
                    generation.clone()
                } else {
                    None
                };
                annotate_runtime_payload(
                    &event.payload_json,
                    interaction_id,
                    event_performance,
                    event_generation,
                )
                .map(|payload_json| (event.event_type, payload_json))
                .map_err(runtime_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        // 指标只落库、不投递实时通道：它与终态共享同一次事务与一次 fsync，
        // 但订阅者看到的帧序列里不该多出一条派生读数。
        let (payloads, publishable) = terminal_batch(payloads, turn_metrics);
        let persisted = self
            .store
            .append_interaction_events(
                session_id,
                &self.host,
                interaction_id,
                lease_token,
                &payloads,
                self.clock.now_ms(),
            )
            .map_err(runtime_error)?;
        let egress = CommittedEventEgress::new(&self.publisher, self.stream_bridge.as_ref());
        for event in persisted.iter().take(publishable) {
            egress
                .publish_for_interaction(interaction_id, event, &event.cursor())
                .map_err(PrimaryTurnRunnerError::Renderer)?;
        }
        Ok(())
    }
}

impl<S, E, P, C> InteractionTurnRunner<S, E, P, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
{
    /// 主 turn 汇总回复产出后，结算完成通知的渠道投递。
    ///
    /// 一次完成通知只在这里结算一次：主 turn 的汇总回复是各渠道唯一交付内容，
    /// 回写汇总后把全部渠道推进到 `dispatched`，由各渠道消费者（TUI 轮询收件箱、
    /// 远端持久投递器）自行确认送达。这里不做任何送达确认：即便远端是当时唯一
    /// 可用渠道，也必须等渠道把正文写入自己的 durable 出站队列后才允许确认。
    fn settle_completion_delivery(&self, interaction: &InteractionRecord, summary: Option<&str>) {
        let Some(execution_id) =
            completion_steer_execution_id(interaction.source.source_message_id())
        else {
            return;
        };
        let notification_id = completion_notification_id(&execution_id);
        let Ok(Some(notification)) = self.store.completion_notification(&notification_id) else {
            return;
        };
        if !notification.state.is_deliverable() {
            return;
        }
        if let Some(summary) = summary
            && self
                .store
                .record_completion_summary(&notification_id, summary, self.now_ms)
                .is_err()
        {
            return;
        }
        let _ = self
            .store
            .dispatch_completion_deliveries(&notification_id, self.now_ms);
    }
}

/// 根据 Interaction 所属复合车道派生内部运行 Session ID。
///
/// Session 是复合车道的内部运行窗口：同一 workspace、trust scope 和
/// channel/destination 组合的相邻输入必须收敛到同一个 Session 才能延续 Provider
/// history；不同组合之间不得共享运行窗口。逐条 Interaction 只决定幂等与投递，
/// 不决定 Session 身份。
/// 读取主 turn 的会话标识。
///
/// 会话标识是入站事实的一列，随 Interaction 一起持久化；本函数只读取，不派生，也不再
/// 依赖 Workspace 是否绑定。
pub fn primary_session_id_for_interaction(
    interaction: &InteractionRecord,
) -> Result<DaemonSessionId, String> {
    let session_id = interaction_session_id(interaction);
    DaemonSessionId::new(session_id).map_err(|error| error.to_string())
}

pub(super) fn stable_interaction_id(prefix: &str, interaction_id: &str) -> String {
    format!("{prefix}{}", sha256_hex(interaction_id.as_bytes()))
}

/// 从后继轮次的来源键解析目标标识：`goal-update:{goal_id}:{n}` 与 `goal-stage:{goal_id}:{stage}`。
fn derived_goal_id(source_message_id: &str) -> Option<axon_domain::GoalId> {
    for prefix in [GOAL_UPDATE_SOURCE_PREFIX, GOAL_STAGE_SOURCE_PREFIX] {
        if let Some(rest) = source_message_id.strip_prefix(prefix) {
            let goal_id = rest.split(':').next()?;
            return axon_domain::GoalId::new(goal_id.to_owned()).ok();
        }
    }
    None
}

/// 重试轮标识回落到账本标识：`{execution_id}-r{attempt}` 去掉重试后缀。
///
/// 后缀只在目标执行重投时写入，形状由本模块自己决定，因此解析它是安全的；
/// 其余标识原样返回，交给精确查找判空。
fn retry_base_execution_id(interaction_id: &str) -> String {
    match interaction_id.rsplit_once("-r") {
        Some((head, suffix))
            if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) =>
        {
            head.to_owned()
        }
        _ => interaction_id.to_owned(),
    }
}

#[path = "interaction_turn_payloads.rs"]
mod interaction_turn_payloads;

use interaction_turn_payloads::{
    TurnGenerationMetrics, annotate_runtime_payload, completion_summary_from_events,
    interaction_payload, interaction_transport_context, publish_turn_performance, runtime_error,
};
#[cfg(test)]
#[path = "interaction_turn_runner_performance_tests.rs"]
mod performance_tests;

/// 终态批次的载荷布局：事件按序在前，派生的 `turn_metrics` 追加在后。
///
/// 返回值第二项是「可投递前缀」长度：`turn_metrics` 只落库，不投递实时通道。
fn terminal_batch(
    payloads: Vec<(String, String)>,
    metrics: Option<(String, String)>,
) -> (Vec<(String, String)>, usize) {
    let publishable = payloads.len();
    let mut payloads = payloads;
    if let Some(metrics) = metrics {
        payloads.push(metrics);
    }
    (payloads, publishable)
}

/// 从失败回执里取出本轮告警；取不到就是空。
fn failure_warnings(message: &str) -> Vec<String> {
    let Some(start) = message.find("{") else {
        return Vec::new();
    };
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&message[start..]) else {
        return Vec::new();
    };
    payload
        .get("warnings")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "interaction_turn_runner_cwd_tests.rs"]
mod cwd_tests;

#[cfg(test)]
#[path = "interaction_turn_runner_metrics_tests.rs"]
mod metrics_tests;

#[cfg(test)]
#[path = "interaction_turn_runner_warnings_tests.rs"]
mod warnings_tests;
