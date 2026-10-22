//! Daemon Host Execution 完成交付（持久收件箱 / 全渠道投递）测试。

use super::super::completion_delivery::{
    CLIENT_PRESENCE_TTL_MS, COMPLETION_DISPATCH_BATCH_LIMIT, completion_notification_id,
};
use super::super::queued_action;
use super::*;
use crate::ActiveTurnSteerSink;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, ExecutionRecord, ExecutionStatus, GoalId,
    GoalPlan, GoalRecord, GoalStatus, InteractionRecord, InteractionSource, JarvisEvent,
    JarvisEventId, ReplyRoute, ResultId, ResultRecord, RuntimeHostId, ScopeRef,
};
use axon_session::{
    CheckpointTelemetry, ClientDeliveryStore, ClientPresenceRecord, DecisionStore, ExecutionStore,
    GoalStore, InteractionStore, LedgerCommitStore, NotificationDeliveryState, ResultStore,
    SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{
    ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome, model::RuntimeEvent,
};
use std::sync::{Arc, Mutex, PoisonError};

const TEST_NOW_MS: u64 = 10;
const TEST_LEASE_DURATION_MS: u64 = 1_000;
const USER_INPUT_ACTION_KIND: &str = "user_input";
const TUI_CHANNEL: &str = "tui";
const FEISHU_CHANNEL: &str = "feishu";
const TUI_CLIENT_ID: &str = "tui-delivery-client";
const TUI_DESTINATION: &str = "tui-delivery-destination";
const FEISHU_DESTINATION: &str = "oc_delivery_chat";

#[derive(Clone, Default)]
pub(super) struct RecordingSteerSink {
    steered: Arc<Mutex<Vec<InteractionRecord>>>,
}

impl RecordingSteerSink {
    fn steered(&self) -> Vec<InteractionRecord> {
        self.steered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ActiveTurnSteerSink for RecordingSteerSink {
    fn try_steer(&self, interaction: &InteractionRecord) -> Result<bool, String> {
        self.steered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(interaction.clone());
        // 记录后返回 false，模拟主 turn 不在线；后续 primary_turn_tick 负责消费持久化通知。
        Ok(false)
    }
}

#[derive(Clone, Copy)]
pub(super) struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        TEST_NOW_MS
    }
}

#[derive(Clone, Copy)]
pub(super) struct TestProcess;

impl crate::ports::ProcessIdentity for TestProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[derive(Clone, Copy)]
pub(super) struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(super) struct AnswerExecutor;

impl TurnExecutor for AnswerExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: "turn_completed".to_owned(),
                payload_json: serde_json::json!({"response": "主 turn 汇总回复"}).to_string(),
            }],
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

fn test_action(
    session_id: &DaemonSessionId,
    action_id: &str,
    prompt: &str,
    transport_context: serde_json::Value,
) -> axon_session::SessionActionRecord {
    queued_action(
        SessionActionId::new(action_id).expect("Action ID"),
        session_id.clone(),
        USER_INPUT_ACTION_KIND.to_owned(),
        serde_json::json!({"prompt": prompt, "transportContext": transport_context}).to_string(),
    )
}

/// 测试 Execution 的原始复合车道上下文；默认受众只保留当前私密上下文。
fn private_transport_context(channel: &str, destination: &str) -> serde_json::Value {
    serde_json::json!({
        "channel": channel,
        "destination": destination,
        "audience": {
            "kind": "private",
            "disclosure": "private_only",
        },
    })
}

pub(super) fn register_workspace(
    store: &SqliteRuntimeStore,
    path: &std::path::Path,
) -> axon_session::WorkspaceRecord {
    let workspace = WorkspaceResolver
        .resolve(path, TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    workspace
}

fn scope_of(workspace: &axon_session::WorkspaceRecord) -> ScopeRef {
    ScopeRef::workspace(&workspace.workspace_id).expect("构造作用域")
}

fn new_goal(goal_id: &GoalId, scope: &ScopeRef, title: &str) -> GoalRecord {
    GoalRecord::new(
        goal_id.clone(),
        scope.clone(),
        title,
        GoalPlan {
            objective: format!("验证{title}"),
            steps: vec!["步骤 1".to_owned()],
        },
        vec!["验收标准".to_owned()],
        TEST_NOW_MS,
    )
    .expect("Goal")
}

/// 建立 Goal、Allow Decision、Session 与已入队的后台 Execution Action。
pub(super) fn seed_completion_request(
    store: &SqliteRuntimeStore,
    workspace: &axon_session::WorkspaceRecord,
    session_id: &DaemonSessionId,
    goal_id: &GoalId,
    action_id: &str,
    title: &str,
) -> axon_session::SessionActionRecord {
    seed_completion_request_for_transport(
        store,
        workspace,
        session_id,
        goal_id,
        action_id,
        title,
        private_transport_context(TUI_CHANNEL, TUI_DESTINATION),
    )
}

pub(super) fn seed_completion_request_for_transport(
    store: &SqliteRuntimeStore,
    workspace: &axon_session::WorkspaceRecord,
    session_id: &DaemonSessionId,
    goal_id: &GoalId,
    action_id: &str,
    title: &str,
    transport_context: serde_json::Value,
) -> axon_session::SessionActionRecord {
    let scope = scope_of(workspace);
    let goal = new_goal(goal_id, &scope, title);
    store.create_goal(&goal).expect("保存 Goal");
    store
        .append_decision(
            &axon_domain::DecisionRecord {
                decision_id: axon_domain::DecisionId::new(format!("allow-{action_id}"))
                    .expect("Decision ID"),
                scope,
                actor: "test".to_owned(),
                effect: axon_domain::DecisionEffect::Allow,
                revision: 1,
                expires_at_ms: None,
                created_at_ms: TEST_NOW_MS,
            },
            0,
        )
        .expect("写入 Allow Decision");
    let mut action = test_action(session_id, action_id, title, transport_context);
    action.goal_id = Some(goal_id.clone());
    store
        .create_session(session_id, &workspace.workspace_root, TEST_NOW_MS)
        .expect("创建 Session");
    store
        .enqueue_action(&action, TEST_NOW_MS)
        .expect("入队 Action");
    action
}

pub(super) fn register_tui_presence(
    store: &SqliteRuntimeStore,
    workspace: &axon_session::WorkspaceRecord,
) {
    store
        .register_client_presence(&ClientPresenceRecord {
            client_id: TUI_CLIENT_ID.to_owned(),
            workspace_id: workspace.workspace_id.as_str().to_owned(),
            channel: TUI_CHANNEL.to_owned(),
            destination: TUI_DESTINATION.to_owned(),
            registered_at_ms: TEST_NOW_MS,
            expires_at_ms: TEST_NOW_MS.saturating_add(CLIENT_PRESENCE_TTL_MS),
        })
        .expect("注册 TUI presence");
}

/// 写入一条历史飞书 Interaction，用于验证它不会自动成为通知目标。
pub(super) fn append_feishu_interaction(
    store: &SqliteRuntimeStore,
    workspace: &axon_session::WorkspaceRecord,
    source_message_id: &str,
) {
    let interaction = InteractionRecord::new(
        axon_domain::InteractionId::new(format!("feishu-{source_message_id}"))
            .expect("Interaction ID"),
        InteractionSource::new(FEISHU_CHANNEL, source_message_id).expect("来源"),
        scope_of(workspace),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new(FEISHU_CHANNEL, FEISHU_DESTINATION).expect("回复路由"),
        "飞书原始请求",
        TEST_NOW_MS,
    )
    .expect("Interaction");
    let receipt = store.append(&interaction).expect("追加 Interaction");
    assert!(receipt.inserted, "外部渠道 Interaction 必须首次写入");
}

pub(super) fn unread_results(
    store: &SqliteRuntimeStore,
    workspace: &axon_session::WorkspaceRecord,
) -> usize {
    store
        .list_unread_results(&scope_of(workspace), 10)
        .expect("读取未读 Result")
        .len()
}

pub(super) fn notification_of(
    store: &SqliteRuntimeStore,
    execution_id: &ExecutionId,
) -> axon_session::CompletionNotificationRecord {
    store
        .completion_notification(&completion_notification_id(execution_id))
        .expect("读取完成通知")
        .expect("完成通知必须已落库")
}

pub(super) fn start_host(
    store: &SqliteRuntimeStore,
    host_id: RuntimeHostId,
    steer_sink: &RecordingSteerSink,
) -> DaemonHost<SqliteRuntimeStore, AnswerExecutor, NoopPublisher, FixedClock, TestProcess> {
    let mut host = DaemonHost::new(
        store.clone(),
        AnswerExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        host_id,
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    )
    .with_active_turn_steer_sink(steer_sink.clone());
    host.start().expect("启动 Host");
    host
}

#[test]
fn all_available_channels_receive_one_primary_turn_completion() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let session_id = DaemonSessionId::new("all-channel-session").expect("Session ID");
    let goal_id = GoalId::new("all-channel-task").expect("Goal ID");
    let action = seed_completion_request(
        &store,
        &workspace,
        &session_id,
        &goal_id,
        "execution-all-channel",
        "多渠道任务",
    );
    // TUI 与最近使用的飞书渠道都可用时，两者都必须登记投递。
    append_feishu_interaction(&store, &workspace, "om_feishu_recent");
    register_tui_presence(&store, &workspace);

    let steer_sink = RecordingSteerSink::default();
    let mut host = start_host(
        &store,
        RuntimeHostId::new("all-channel-host").expect("Host ID"),
        &steer_sink,
    );
    host.set_feishu_notification_chat_id(Some(FEISHU_DESTINATION))
        .expect("配置飞书通知目标");

    assert_eq!(
        host.pump_session_once(session_id).expect("pump"),
        Some(action.action_id)
    );

    let steered = steer_sink.steered();
    assert_eq!(steered.len(), 1, "同一次 Execution 只能通知主 turn 一次");
    assert_eq!(steered[0].reply_route.channel(), TUI_CHANNEL);
    assert_eq!(steered[0].reply_route.destination(), TUI_DESTINATION);
    assert!(steered[0].content.contains("多渠道任务"));

    let execution_id = ExecutionId::new("execution-all-channel").expect("Execution ID");
    let notification = notification_of(&store, &execution_id);
    assert_eq!(notification.state, NotificationDeliveryState::Dispatched);
    assert_eq!(notification.result_ids.len(), 1);
    let deliveries = store
        .completion_deliveries(&notification.notification_id)
        .expect("读取渠道投递");
    assert_eq!(deliveries.len(), 2, "TUI 与 Feishu 都必须登记");
    assert!(deliveries.iter().any(|delivery| {
        delivery.channel == TUI_CHANNEL
            && delivery.destination == TUI_DESTINATION
            && delivery.state == NotificationDeliveryState::Pending
    }));
    assert!(deliveries.iter().any(|delivery| {
        delivery.channel == FEISHU_CHANNEL
            && delivery.destination == FEISHU_DESTINATION
            && delivery.state == NotificationDeliveryState::Pending
    }));
    // 客户端确认展示之前，Result 必须保持未读：禁止静默 ack。
    assert_eq!(unread_results(&store, &workspace), 1);

    let tick = host
        .primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("主 turn tick");
    assert!(tick.understood >= 1, "完成通知未被主 turn 消费");
    host.primary_turn_tick(COMPLETION_DISPATCH_BATCH_LIMIT)
        .expect("继续运行主 turn");
    let polled = poll_delivered_notifications(&mut host);
    assert_eq!(polled, 1, "poll 必须返回已派发通知");
    assert_eq!(
        acknowledge_test_notification(&mut host, &notification.notification_id),
        1,
        "客户端确认必须结算通知"
    );
    assert_eq!(
        acknowledge_test_notification(&mut host, &notification.notification_id),
        0,
        "重复 ack 不得重复计数"
    );
    assert_eq!(
        notification_of(&store, &execution_id).state,
        NotificationDeliveryState::Delivered
    );
    assert_eq!(unread_results(&store, &workspace), 0);
}

fn poll_delivered_notifications(
    host: &mut DaemonHost<
        SqliteRuntimeStore,
        AnswerExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
    >,
) -> usize {
    let body = send_daemon_command(
        host,
        axon_protocol::DaemonCommand::PollNotifications {
            client_id: TUI_CLIENT_ID.to_owned(),
            limit: COMPLETION_DISPATCH_BATCH_LIMIT,
        },
    );
    body["notifications"]
        .as_array()
        .map_or(0, std::vec::Vec::len)
}

fn acknowledge_test_notification(
    host: &mut DaemonHost<
        SqliteRuntimeStore,
        AnswerExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
    >,
    notification_id: &str,
) -> u64 {
    let body = send_daemon_command(
        host,
        axon_protocol::DaemonCommand::AcknowledgeNotifications {
            client_id: TUI_CLIENT_ID.to_owned(),
            notification_ids: vec![notification_id.to_owned()],
        },
    );
    body["acknowledged"].as_u64().unwrap_or_default()
}

fn send_daemon_command(
    host: &mut DaemonHost<
        SqliteRuntimeStore,
        AnswerExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
    >,
    command: axon_protocol::DaemonCommand,
) -> serde_json::Value {
    let response = host
        .handle_request(axon_protocol::DaemonRequest {
            message_type: "command".to_owned(),
            request_id: "delivery-test-request".to_owned(),
            protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
            client_id: "delivery-test-client".to_owned(),
            session_id: None,
            command,
        })
        .into_iter()
        .next()
        .expect("Daemon 必须返回响应");
    assert!(response.ok, "Daemon 命令必须成功：{:?}", response.body);
    response.body
}

#[path = "daemon_host_execution_delivery_recovery_tests.rs"]
mod daemon_host_execution_delivery_recovery_tests;
