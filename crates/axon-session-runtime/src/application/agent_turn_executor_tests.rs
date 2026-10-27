use super::*;
use crate::model::TurnFailureKind;
use axon_domain::{DaemonSessionId, SessionActionId, SessionActionStatus};
use axon_protocol::wire::WireEventKind;
use axon_session::SessionActionRecord;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct RecordingRunner {
    requests: Arc<Mutex<Vec<ProviderTurnRequest>>>,
    local_requests: Arc<Mutex<Vec<ProviderTurnRequest>>>,
}

impl ProviderTurnRunner for RecordingRunner {
    fn run(&self, request: ProviderTurnRequest) -> Result<ProviderTurnResult, TurnExecutionError> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request);
        Ok(ProviderTurnResult {
            response: "完成".to_owned(),
            wire_events: Vec::new(),
            continuation: None,
            transcript_ref: None,
            token_usage: None,
            time_usage_ms: None,
            agent_tree_summary: None,
            tool_receipt_sequence: None,
        })
    }

    fn run_local_action(
        &self,
        request: ProviderTurnRequest,
    ) -> Result<ProviderTurnResult, TurnExecutionError> {
        self.local_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request);
        Ok(ProviderTurnResult {
            response: String::new(),
            wire_events: Vec::new(),
            continuation: None,
            transcript_ref: None,
            token_usage: None,
            time_usage_ms: None,
            agent_tree_summary: None,
            tool_receipt_sequence: None,
        })
    }
}

#[test]
fn authoritative_context_supplies_session_identity() {
    let runner = RecordingRunner::default();
    let executor = AgentTurnExecutor::new(runner.clone());
    let session_id = DaemonSessionId::new("session-a").expect("有效 session");
    let outcome = executor
        .execute(ActionExecutionRequest {
            action: SessionActionRecord {
                action_id: SessionActionId::new("action-a").expect("有效 action"),
                goal_id: None,
                session_id,
                kind: "user_input".to_owned(),
                priority: 0,
                status: SessionActionStatus::Queued,
                dedupe_key: None,
                payload_json: r#"{"prompt":"测试"}"#.to_owned(),
                goal_revision: None,
                context_version: Some(3),
                claimed_by: None,
                claimed_generation: None,
                lease_expires_at_ms: None,
                attempt: 0,
            },
            context: SessionExecutionContext {
                run_session_id: "新会话".to_owned(),
                cwd: "新目录".to_owned(),
                context_version: 3,
            },
            runtime_profile: crate::JarvisRuntimeSelector::for_background_execution(),
        })
        .expect("执行成功");
    let requests = runner
        .requests
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(requests[0].session_id, "session-a");
    assert_eq!(requests[0].action_kind, "user_input");
    assert_eq!(requests[0].run_session_id, "新会话");
    assert_eq!(requests[0].cwd, "新目录");
    assert_eq!(outcome.events.len(), 1);
}

#[test]
fn provider_payload_rejects_execution_identity_fields() {
    let executor = AgentTurnExecutor::new(RecordingRunner::default());
    let error = executor
        .execute(ActionExecutionRequest {
            action: SessionActionRecord {
                action_id: SessionActionId::new("action-invalid").expect("有效 action"),
                goal_id: None,
                session_id: DaemonSessionId::new("session-invalid").expect("有效 session"),
                kind: "user_input".to_owned(),
                priority: 0,
                status: SessionActionStatus::Queued,
                dedupe_key: None,
                payload_json: r#"{"prompt":"测试","cwd":"旧目录","runSessionId":"旧会话"}"#
                    .to_owned(),
                goal_revision: None,
                context_version: Some(1),
                claimed_by: None,
                claimed_generation: None,
                lease_expires_at_ms: None,
                attempt: 0,
            },
            context: SessionExecutionContext {
                run_session_id: "新会话".to_owned(),
                cwd: "新目录".to_owned(),
                context_version: 1,
            },
            runtime_profile: crate::JarvisRuntimeSelector::for_background_execution(),
        })
        .expect_err("Provider payload 不得携带执行身份");
    assert!(error.message().contains("unknown field"));
    assert_eq!(error.kind(), TurnFailureKind::Permanent);
}

#[test]
fn remote_control_action_is_committed_without_provider_call() {
    let runner = RecordingRunner::default();
    let executor = AgentTurnExecutor::new(runner.clone());
    let session_id = DaemonSessionId::new("session-control").expect("有效 session");
    let outcome = executor
        .execute(ActionExecutionRequest {
            action: SessionActionRecord {
                action_id: SessionActionId::new("action-control").expect("有效 action"),
                goal_id: None,
                session_id,
                kind: "remote_control".to_owned(),
                priority: 100,
                status: SessionActionStatus::Queued,
                dedupe_key: Some("feishu-control:action-control".to_owned()),
                payload_json: r#"{"control":{"preview":"/status"},"transportContext":{"kind":"feishu","chatId":"oc_chat"}}"#.to_owned(),
                goal_revision: None,
                context_version: None,
                claimed_by: None,
                claimed_generation: None,
                lease_expires_at_ms: None,
                attempt: 0,
            },
            context: SessionExecutionContext {
                run_session_id: "session-control".to_owned(),
                cwd: ".".to_owned(),
                context_version: 0,
            },
            runtime_profile: crate::JarvisRuntimeSelector::for_background_execution(),
        })
        .expect("控制 Action 应执行");
    assert!(
        runner
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    let payload: serde_json::Value =
        serde_json::from_str(&outcome.events[0].payload_json).expect("事件应为 JSON");
    assert_eq!(payload["transportContext"]["kind"], "feishu");
    assert_eq!(payload["response"], "状态: 已处理");
}

#[test]
fn remote_control_commands_keep_visible_interaction_replies() {
    let cases = [
        ("/stop", "已停止当前会话。"),
        ("/status", "状态: 已处理"),
        ("/queue", "队列查询已完成。"),
    ];
    for (preview, expected) in cases {
        let response = remote_control_response(preview, "session-control");
        assert!(
            response.contains(expected),
            "命令 {preview} 的回执缺少 {expected}: {response}"
        );
        assert!(
            !response.contains("session-control"),
            "用户可见输出不应泄露内部 session_id: {response}"
        );
    }
}

#[test]
fn preserves_transport_context_without_copying_quoted_message_to_completed_event() {
    let runner = RecordingRunner::default();
    let executor = AgentTurnExecutor::new(runner.clone());
    let session_id = DaemonSessionId::new("session-b").expect("有效 session");
    let outcome = executor
        .execute(ActionExecutionRequest {
            action: SessionActionRecord {
                action_id: SessionActionId::new("action-b").expect("有效 action"),
                goal_id: None,
                session_id,
                kind: "user_input".to_owned(),
                priority: 10,
                status: SessionActionStatus::Queued,
                dedupe_key: Some("feishu:om_source".to_owned()),
                payload_json: r#"{"prompt":"问题","quotedMessage":{"messageId":"om_parent","visibleText":"父消息正文"},"transportContext":{"kind":"feishu"}}"#
                    .to_owned(),
                goal_revision: None,
                context_version: None,
                claimed_by: None,
                claimed_generation: None,
                lease_expires_at_ms: None,
                attempt: 0,
            },
            context: SessionExecutionContext {
                run_session_id: "run-session-b".to_owned(),
                cwd: ".".to_owned(),
                context_version: 0,
            },
            runtime_profile: crate::JarvisRuntimeSelector::for_background_execution(),
        })
        .expect("Turn 应正常执行");
    let requests = runner.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .quoted_message
            .as_ref()
            .map(|quoted| quoted.visible_text.as_str()),
        Some("父消息正文")
    );
    let payload: serde_json::Value =
        serde_json::from_str(&outcome.events[0].payload_json).expect("事件应为 JSON");
    assert_eq!(
        payload["transportContext"]["kind"],
        serde_json::Value::String("feishu".to_owned())
    );
    assert!(payload.get("quotedMessage").is_none());
}
#[derive(Clone, Default)]
struct LiveSinkProbe {
    published: Arc<Mutex<Vec<String>>>,
}

impl crate::ports::TurnEventSink for LiveSinkProbe {
    fn publish(&self, event: &RuntimeEvent) -> Result<(), String> {
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.payload_json.clone());
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct StreamingRunner;

impl ProviderTurnRunner for StreamingRunner {
    fn run(&self, _request: ProviderTurnRequest) -> Result<ProviderTurnResult, TurnExecutionError> {
        Err(TurnExecutionError::permanent("必须走实时投影通道"))
    }

    fn run_streaming(
        &self,
        _request: ProviderTurnRequest,
        sink: Option<&dyn crate::ports::TurnEventSink>,
    ) -> Result<ProviderTurnResult, TurnExecutionError> {
        sink.expect("实时投影通道必须注入")
            .publish(&RuntimeEvent {
                event_type: WIRE_EVENT.to_owned(),
                payload_json: LIVE_EVENT_PAYLOAD.to_owned(),
            })
            .expect("实时事件应被通道接受");
        let mut repeated_delta = batch_wire_event();
        if let WireEventKind::AssistantMessage(message) = &mut repeated_delta.kind {
            message.delta = true;
        }
        Ok(ProviderTurnResult {
            response: "实时回答".to_owned(),
            wire_events: vec![
                repeated_delta,
                batch_wire_event(),
                run_completed_wire_event(),
            ],
            continuation: None,
            transcript_ref: None,
            token_usage: None,
            time_usage_ms: None,
            agent_tree_summary: None,
            tool_receipt_sequence: None,
        })
    }
}

const LIVE_EVENT_PAYLOAD: &str = r#"{"kind":"live-progress"}"#;

fn batch_wire_event() -> WireEventEnvelope {
    WireEventEnvelope {
        schema_version: axon_protocol::wire::WireSchemaVersion::default(),
        event_id: axon_protocol::wire::WireCorrelationId("batch-event".to_owned()),
        correlation_id: axon_protocol::wire::WireCorrelationId("batch".to_owned()),
        session: axon_protocol::wire::WireSessionRef {
            session_id: "session-batch".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 1,
        recorded_at_ms: 1,
        kind: axon_protocol::wire::WireEventKind::AssistantMessage(
            axon_protocol::wire::WireAssistantMessage {
                part_id: "assistant-part:1".to_owned(),
                message_id: "assistant-provider:1".to_owned(),
                content: "最终回答".to_owned(),
                delta: false,
            },
        ),
    }
}

fn run_completed_wire_event() -> WireEventEnvelope {
    WireEventEnvelope {
        schema_version: axon_protocol::wire::WireSchemaVersion::default(),
        event_id: axon_protocol::wire::WireCorrelationId("run-completed".to_owned()),
        correlation_id: axon_protocol::wire::WireCorrelationId("run".to_owned()),
        session: axon_protocol::wire::WireSessionRef {
            session_id: "session-batch".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 2,
        recorded_at_ms: 2,
        kind: axon_protocol::wire::WireEventKind::TurnLifecycle(
            axon_protocol::wire::WireTurnLifecycle {
                part_id: "lifecycle:run".to_owned(),
                turn_id: "run".to_owned(),
                phase: axon_protocol::wire::WireLifecyclePhase::RegularTurn,
                status: axon_protocol::wire::WireLifecycleStatus::Completed,
                title: "Run completed".to_owned(),
                kind: Some("RunLifecycle".to_owned()),
                ui_phase: None,
                tool_name: None,
                attempt_index: None,
                summary: None,
                duration_ms: None,
                started_at_ms: None,
                total_tokens: None,
                cache_hit_rate_basis_points: None,

                warnings: Vec::new(),
            },
        ),
    }
}

#[derive(Clone, Copy)]
struct FailingCanonicalSink;

impl crate::ports::TurnEventSink for FailingCanonicalSink {
    fn publish(&self, _event: &RuntimeEvent) -> Result<(), String> {
        Ok(())
    }

    fn publish_batch(&self, _events: &[RuntimeEvent]) -> Result<(), String> {
        Err("durable store 写入失败".to_owned())
    }
}

fn live_request(action_id: &str) -> ActionExecutionRequest {
    ActionExecutionRequest {
        action: SessionActionRecord {
            action_id: SessionActionId::new(action_id).expect("有效 action"),
            goal_id: None,
            session_id: DaemonSessionId::new("session-live").expect("有效 session"),
            kind: "user_input".to_owned(),
            priority: 10,
            status: SessionActionStatus::Queued,
            dedupe_key: None,
            payload_json: r#"{"prompt":"实时投影"}"#.to_owned(),
            goal_revision: None,
            context_version: None,
            claimed_by: None,
            claimed_generation: None,
            lease_expires_at_ms: None,
            attempt: 0,
        },
        context: SessionExecutionContext {
            run_session_id: "run-session-live".to_owned(),
            cwd: ".".to_owned(),
            context_version: 0,
        },
        runtime_profile: crate::JarvisRuntimeSelector::for_background_execution(),
    }
}

#[test]
fn canonical_terminal_publish_failure_aborts_before_turn_completed() {
    let executor = AgentTurnExecutor::new(StreamingRunner);
    let sink = FailingCanonicalSink;
    let error = executor
        .execute_streaming(live_request("action-failing-sink"), Some(&sink))
        .expect_err("canonical 终止事件投影失败必须终止本轮");

    assert_eq!(error.kind(), TurnFailureKind::Permanent);
    assert!(
        error.message().contains("canonical 终止事件"),
        "失败事实必须指向 durable 缺口：{}",
        error.message()
    );
}

#[test]
fn streaming_execution_projects_canonical_terminal_events_before_turn_completed() {
    let executor = AgentTurnExecutor::new(StreamingRunner);
    let sink = LiveSinkProbe::default();
    let session_id = DaemonSessionId::new("session-live").expect("有效 session");
    let outcome = executor
        .execute_streaming(
            ActionExecutionRequest {
                action: SessionActionRecord {
                    action_id: SessionActionId::new("action-live").expect("有效 action"),
                    goal_id: None,
                    session_id,
                    kind: "user_input".to_owned(),
                    priority: 10,
                    status: SessionActionStatus::Queued,
                    dedupe_key: None,
                    payload_json: r#"{"prompt":"实时投影"}"#.to_owned(),
                    goal_revision: None,
                    context_version: None,
                    claimed_by: None,
                    claimed_generation: None,
                    lease_expires_at_ms: None,
                    attempt: 0,
                },
                context: SessionExecutionContext {
                    run_session_id: "run-session-live".to_owned(),
                    cwd: ".".to_owned(),
                    context_version: 0,
                },
                runtime_profile: crate::JarvisRuntimeSelector::for_background_execution(),
            },
            Some(&sink),
        )
        .expect("Turn 应实时执行");

    let published = sink
        .published
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        published.first().map(String::as_str),
        Some(LIVE_EVENT_PAYLOAD)
    );
    let terminal_events = published[1..]
        .iter()
        .map(|payload| {
            serde_json::from_str::<WireEventEnvelope>(payload).expect("终态 wire 事件应可解析")
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        terminal_events[0].kind,
        WireEventKind::AssistantMessage(ref message) if !message.delta
    ));
    assert!(matches!(
        terminal_events[1].kind,
        WireEventKind::TurnLifecycle(ref lifecycle)
            if lifecycle.kind.as_deref() == Some("RunLifecycle")
                && lifecycle.status == axon_protocol::wire::WireLifecycleStatus::Completed
    ));
    assert_eq!(
        outcome
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [TURN_COMPLETED_EVENT],
        "实时 wire 事件不得在收口阶段重复持久化"
    );
}
