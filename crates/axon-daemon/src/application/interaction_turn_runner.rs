//! Daemon Interaction 到 Primary Turn 与 Work Ledger 的执行边界。

use super::super::egress_bridge::CommittedEventEgress;
use super::super::event_stream_bridge::EventStreamBridge;
use super::super::primary_turn_service::{
    PrimaryTurnRequest, PrimaryTurnRunner, PrimaryTurnRunnerError,
};
use super::completion_delivery::{
    completion_notification_id, completion_steer_execution_id, cron_delivery_id,
    resolve_completion_targets,
};
use super::daemon_host_projection::queued_action;
#[path = "child_session_event_sink.rs"]
mod child_session_event_sink;
#[path = "interaction_turn_event_sink.rs"]
mod interaction_turn_event_sink;
#[path = "interaction_turn_generation_watch.rs"]
mod interaction_turn_generation_watch;
#[path = "interaction_turn_terminal.rs"]
mod interaction_turn_terminal;
use axon_crypto::sha256_hex;
use axon_domain::{DaemonSessionId, InteractionRecord, SessionActionId};
use axon_session::{
    DaemonHostRecord, NotificationDeliveryState, RuntimeStore, SessionRuntimeRecord,
    interaction_session_lane_id,
};
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
use std::sync::{Arc, RwLock};
/// Daemon Primary Turn 使用的稳定 Action 类型。
pub const PRIMARY_TURN_ACTION_KIND: &str = "user_input";
const PRIMARY_ACTION_ID_PREFIX: &str = "interaction-action-";
/// 用户消息事件的关联标识。
const USER_CORRELATION_ID: &str = "user";
#[derive(Clone)]
pub(super) struct InteractionTurnRunner<S, E, P, C> {
    pub(super) store: S,
    pub(super) executor: E,
    pub(super) publisher: P,
    pub(super) stream_bridge: Arc<EventStreamBridge>,
    pub(super) feishu_notification_chat_id: Arc<RwLock<Option<String>>>,
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
                cwd: self
                    .store
                    .get_session(&session_id)
                    .map(|record| record.cwd)
                    .unwrap_or_default(),
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
                )?;
                if kind == TurnFailureKind::Interrupted {
                    return Err(PrimaryTurnRunnerError::Interrupted(response));
                }
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
        self.persist_and_publish(
            &session_id,
            &interaction.interaction_id,
            &lease_token,
            outcome.events,
            performance.as_deref(),
            generation,
        )?;
        self.settle_completion_delivery(&interaction, summary.as_deref());
        self.record_cron_delivery(&interaction, summary.as_deref());
        Ok(())
    }
}

impl<S, E, P, C> InteractionTurnRunner<S, E, P, C>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    P: EventPublisher,
    C: RuntimeClock,
{
    fn execution_request(
        &self,
        request: PrimaryTurnRequest,
    ) -> Result<(InteractionRecord, DaemonSessionId, ActionExecutionRequest), PrimaryTurnRunnerError>
    {
        let interaction = request.interaction;
        let interaction_id = interaction.interaction_id.as_str();
        let session_id = interaction_session_lane_id(&interaction)
            .ok_or_else(|| runtime_error("Primary Turn 必须绑定 Workspace"))?;
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
                run_session_id: runtime.run_session_id,
                cwd: runtime.cwd,
                context_version: runtime.context_version,
            },
            runtime_profile: request.runtime_profile,
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
        self.store
            .create_session(session_id, &workspace.workspace_root, self.now_ms)
            .map_err(runtime_error)
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
                    generation
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
        for event in &persisted {
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
    /// 飞书持久投递器）自行确认送达。这里不做任何送达确认：即便飞书是当时唯一
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

    /// 把 Cron 派发主 turn 的汇总回复登记为投递项，并推进到所有可用渠道。
    ///
    /// Cron 投递与 Task 完成投递共用同一套持久收件箱：正文只在主 turn 产出后登记，
    /// 因此不存在「先登记空正文、再补写」的竞态；没有可用渠道时投递项保留在收件箱，
    /// 等渠道出现后补投。同一 Cron 运行重复结算命中唯一约束，只产生一条投递项。
    fn record_cron_delivery(&self, interaction: &InteractionRecord, summary: Option<&str>) {
        if cron_delivery_id(interaction.source.source_message_id()).is_none() {
            return;
        }
        let Some(summary) = summary.filter(|text| !text.is_empty()) else {
            return;
        };
        let Some(workspace_id) = interaction.scope.workspace_id() else {
            return;
        };
        let notification_id =
            axon_domain::cron_delivery_notification_id(interaction.interaction_id.as_str());
        let notification = axon_session::CompletionNotificationRecord {
            notification_id: notification_id.clone(),
            workspace_id: workspace_id.as_str().to_owned(),
            goal_id: interaction.source.source_message_id().to_owned(),
            execution_id: interaction.source.source_message_id().to_owned(),
            content: summary.to_owned(),
            summary: summary.to_owned(),
            result_ids: Vec::new(),
            state: NotificationDeliveryState::Dispatched,
            created_at_ms: self.now_ms,
            updated_at_ms: self.now_ms,
            expires_at_ms: super::completion_delivery::completion_expiry_at_ms(self.now_ms),
        };
        if self
            .store
            .upsert_completion_notification(&notification)
            .is_err()
        {
            return;
        }
        let target = match self.feishu_notification_chat_id.read() {
            Ok(target) => target.clone(),
            Err(_) => return,
        };
        let Ok(targets) = resolve_completion_targets(
            &self.store,
            workspace_id.as_str(),
            self.now_ms,
            target.as_deref(),
        ) else {
            return;
        };
        if targets.is_empty() {
            return;
        }
        if self
            .store
            .upsert_completion_deliveries(&notification_id, &targets, self.now_ms)
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
pub fn primary_session_id_for_interaction(
    interaction: &InteractionRecord,
) -> Result<DaemonSessionId, String> {
    let session_id = interaction_session_lane_id(interaction)
        .ok_or_else(|| "Primary Turn 必须绑定 Workspace".to_owned())?;
    DaemonSessionId::new(session_id).map_err(|error| error.to_string())
}

pub(super) fn stable_interaction_id(prefix: &str, interaction_id: &str) -> String {
    format!("{prefix}{}", sha256_hex(interaction_id.as_bytes()))
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
#[path = "interaction_turn_runner_warnings_tests.rs"]
mod warnings_tests;
