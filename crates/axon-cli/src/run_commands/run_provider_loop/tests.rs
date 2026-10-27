use super::*;
use axon_agent_runtime::{
    AGENT_RUN_ERROR_TYPE_INTERRUPTED, AgentControlRuntime, AgentModelMessage, AgentModelPayload,
    ProviderStreamProtocol, ProviderStreamingWireTransport, SubagentSessionRuntime,
};
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{ModelProfileId, PermissionSnapshotId};
use axon_protocol::wire::{WireEventKind, WireLifecycleStatus};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

mod settled_tool_receipt_tests;
mod streaming_completion_tests;

pub(super) fn temp_workspace(label: &str) -> PathBuf {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-scl-fixture-{label}-{}-{}",
        std::process::id(),
        crate::run_commands::now_ms()
    ));
    fs::create_dir_all(root.join("src")).expect("应可创建测试工作区");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"axon_scl_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("应可写入 Cargo.toml");
    root
}

pub(super) fn fixture_request(root: PathBuf) -> RunRequest {
    let config_path = root.join("config.toml");
    let session_id = "session-scl-fixture".to_owned();
    let (startup, execution_tools) =
        crate::run_commands::run_startup_snapshot::test_startup_runtime(
            &config_path,
            &root,
            &session_id,
        );
    RunRequest {
        user_question_port: None,
        client_channel: axon_client_identity::ClientChannel::Cli,
        config_path,
        session_id: session_id.clone(),
        invocation_id: format!("test-invocation:{session_id}"),
        resumed_from: None,
        parent_session_id: None,
        cwd: root,
        prompt: "修复构建失败".to_owned(),
        quoted_message: None,
        structured_output: None,
        selected_skill_name: None,
        selected_skill_source_ref: None,
        model_payload: AgentModelPayload {
            messages: vec![AgentModelMessage::text("user", "修复构建失败")],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        },
        persona_warnings: Vec::new(),
        startup,
        execution_tools,
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        provider: ProviderConfig::default(),
        model: ModelConfig::default(),
        reasoning_effort: None,
        channel_chain: Vec::new(),
        routing_key: "test".to_owned(),
        balancer: ChannelBalancerConfig::default(),
        timeout: Duration::from_secs(20),
        memory_recall: None,
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        model_profile_id: ModelProfileId(0),
        permission_snapshot_id: PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: Arc::new(Mutex::new(AgentControlRuntime::default())),
        agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(SubagentSessionRuntime::default())),
        subagent_executor: Arc::new(super::super::run_subagent_executor::NoopRunSubagentExecutor),
        subagent_cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        steer_inbox: None,
        auto_compaction_threshold: 0.8,
        performance: None,
    }
}

#[test]
fn failed_run_record_rejects_previous_turn_snapshot() {
    let request = fixture_request(temp_workspace("failed-record-previous-turn"));
    let started = super::super::run_session_store::RunSessionRecord::started(&request);
    let mut previous = started.clone();
    previous.assistant_part_id = "assistant-part:previous".to_owned();
    previous.response = "上一轮结果".to_owned();

    let selected = super::super::select_failed_run_record(started.clone(), Some(previous));

    assert_eq!(selected.assistant_part_id, started.assistant_part_id);
    assert!(selected.response.is_empty());
}

#[test]
fn failed_run_record_preserves_current_turn_partial() {
    let request = fixture_request(temp_workspace("failed-record-current-turn"));
    let started = super::super::run_session_store::RunSessionRecord::started(&request);
    let mut partial = started.clone();
    partial.response = "当前 partial".to_owned();

    let selected = super::super::select_failed_run_record(started, Some(partial));

    assert_eq!(selected.response, "当前 partial");
}

#[test]
fn failed_run_json_and_wire_runtime_error_share_failure_code() {
    let root = temp_workspace("failed-json-wire-code");
    let request = fixture_request(root.clone());
    let error = RunExecutionError {
        message: "provider 返回不完整工具调用".to_owned(),
        http_status: Some(200),
        error_type: axon_agent_runtime::AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL,
        error_code: Some(
            axon_domain::RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE.to_owned(),
        ),
        provider_protocol: Some(ProviderStreamProtocol::OpenAiResponses.as_code().to_owned()),
        error_param: Some("arguments".to_owned()),
        failure_kind: Some(axon_domain::RUNTIME_FAILURE_KIND_INCOMPLETE.to_owned()),
        call_index: Some(0),
        provider_error: None,
        token_usage: None,
        tool_receipts: Box::new([]),
        visible_output_emitted: false,
        channel_attempts: Vec::new(),
        provider_event_type: None,
        upstream_request_id: None,
        committed_side_effects: false,
    };
    let failure = crate::run_commands::run_session_store::runtime_failure_from_execution_error(
        &request.session_id,
        &error,
        "",
        100,
    );
    let report = crate::run_commands::run_execution::RunJsonEvent::failed(
        &request, &error, &failure, None, None,
    );
    let json = serde_json::to_value(report).expect("失败 Run JSON 应可序列化");
    let text = crate::run_commands::run_text_render::render_text_run_failure(&request, &error);

    assert_eq!(json["event"], "run_failed");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["sessionId"], request.session_id);
    assert_eq!(json["provider"], request.provider.name);
    assert_eq!(json["model"], request.model.id);
    assert_eq!(
        json["failure"]["code"],
        axon_domain::RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE
    );
    assert!(json["wireEvents"].is_array());
    assert!(json.get("channel").is_none());
    assert!(text.contains(&format!("会话 {}", request.session_id)));
    assert!(text.contains(&format!("模型 {}", request.model.id)));
    assert!(text.contains("错误 provider_protocol"));
    let wire_code = json["wireEvents"]
        .as_array()
        .expect("wireEvents 应为数组")
        .iter()
        .find(|event| event["kind"]["type"] == "runtimeError")
        .and_then(|event| event["kind"]["payload"]["code"].as_str())
        .expect("wire RuntimeError 应携带 code");

    assert_eq!(json["failure"]["code"], wire_code);
    assert_eq!(json["failure"]["providerProtocol"], "openai");
    std::fs::remove_dir_all(root).expect("测试目录应可清理");
}

#[test]
fn subagent_publisher_keeps_one_stream_with_monotonic_sequence() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let event_bus: crate::run_commands::run_progress::RunProgressEventBus =
        Arc::new(move |event| {
            captured_events.lock().unwrap().push(event);
            Ok(())
        });
    let task_path = axon_agent_runtime::AgentPath::parse("/root/delegate").unwrap();
    let registry = super::super::run_subagent_lifecycle::SubagentLifecycleRegistry::new(
        Some(event_bus),
        "parent-session".to_owned(),
    );
    let publisher = registry.begin_execution(&task_path);
    publisher
        .publish(RunLifecycleStatusView::with_id(
            "lifecycle:subagent_activity:root:delegate",
            "子代理",
            "subagent_activity",
            "/root/delegate 正在连接供应商",
            "provider_connecting",
            None,
        ))
        .unwrap();
    publisher
        .publish_terminal(
            RunLifecycleStatusView::with_id(
                "lifecycle:subagent_activity:root:delegate",
                "子代理",
                "subagent_activity",
                "/root/delegate 已完成",
                "completed",
                None,
            )
            .completed()
            .with_total_tokens(12_345),
        )
        .unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].event.session.run_id,
        events[1].event.session.run_id
    );
    assert_eq!(events[0].event.sequence, 1);
    assert_eq!(events[1].event.sequence, 2);
    assert_ne!(events[0].event.event_id, events[1].event.event_id);
    let WireEventKind::TurnLifecycle(lifecycle) = &events[1].event.kind else {
        panic!("第二个事件应为子代理终态 lifecycle");
    };
    assert_eq!(lifecycle.total_tokens, Some(12_345));
    assert_eq!(
        lifecycle.status,
        axon_protocol::wire::WireLifecycleStatus::Completed
    );
}

#[test]
fn subagent_publisher_reports_disconnected_event_bus() {
    let event_bus: crate::run_commands::run_progress::RunProgressEventBus =
        Arc::new(|_| Err("runtime channel disconnected".to_owned()));
    let task_path = axon_agent_runtime::AgentPath::parse("/root/delegate").unwrap();
    let registry = super::super::run_subagent_lifecycle::SubagentLifecycleRegistry::new(
        Some(event_bus),
        "parent-session".to_owned(),
    );
    let publisher = registry.begin_execution(&task_path);

    let error = publisher
        .publish(RunLifecycleStatusView::with_id(
            "lifecycle:subagent_activity:root:delegate",
            "子代理",
            "subagent_activity",
            "/root/delegate 正在连接供应商",
            "provider_connecting",
            None,
        ))
        .unwrap_err();

    assert_eq!(error, "runtime channel disconnected");
}

#[test]
fn subagent_publisher_serializes_sequence_assignment_and_delivery() {
    const DELIVERY_RACE_WINDOW: Duration = Duration::from_millis(20);
    let first_entered = Arc::new(std::sync::Barrier::new(2));
    let release_first = Arc::new(std::sync::Barrier::new(2));
    let second_started = Arc::new(std::sync::Barrier::new(2));
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let event_bus: crate::run_commands::run_progress::RunProgressEventBus = {
        let first_entered = Arc::clone(&first_entered);
        let release_first = Arc::clone(&release_first);
        let delivered = Arc::clone(&delivered);
        Arc::new(move |event| {
            if event.event.sequence == 1 {
                first_entered.wait();
                release_first.wait();
            }
            delivered.lock().unwrap().push(event.event.sequence);
            Ok(())
        })
    };
    let task_path = axon_agent_runtime::AgentPath::parse("/root/delegate").unwrap();
    let registry = super::super::run_subagent_lifecycle::SubagentLifecycleRegistry::new(
        Some(event_bus),
        "parent-session".to_owned(),
    );
    let publisher = registry.begin_execution(&task_path);
    let first = {
        let publisher = publisher.clone();
        std::thread::spawn(move || {
            publisher.publish(RunLifecycleStatusView::with_id(
                "lifecycle:subagent_activity:root:delegate",
                "子代理",
                "subagent_activity",
                "/root/delegate 正在连接供应商",
                "provider_connecting",
                None,
            ))
        })
    };
    first_entered.wait();
    let second = {
        let second_started = Arc::clone(&second_started);
        std::thread::spawn(move || {
            second_started.wait();
            publisher.publish(RunLifecycleStatusView::with_id(
                "lifecycle:subagent_activity:root:delegate",
                "子代理",
                "subagent_activity",
                "/root/delegate 正在运行",
                "running",
                None,
            ))
        })
    };
    second_started.wait();
    std::thread::sleep(DELIVERY_RACE_WINDOW);
    release_first.wait();

    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    assert_eq!(*delivered.lock().unwrap(), vec![1, 2]);
}

#[test]
fn subagent_publisher_rejects_progress_after_terminal() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let event_bus: crate::run_commands::run_progress::RunProgressEventBus =
        Arc::new(move |event| {
            captured_events.lock().unwrap().push(event);
            Ok(())
        });
    let task_path = axon_agent_runtime::AgentPath::parse("/root/delegate").unwrap();
    let registry = super::super::run_subagent_lifecycle::SubagentLifecycleRegistry::new(
        Some(event_bus),
        "parent-session".to_owned(),
    );
    let publisher = registry.begin_execution(&task_path);

    assert!(
        publisher
            .publish_terminal(
                RunLifecycleStatusView::with_id(
                    "lifecycle:subagent_activity:root:delegate",
                    "子代理",
                    "subagent_activity",
                    "/root/delegate 已完成",
                    "completed",
                    None,
                )
                .completed(),
            )
            .unwrap()
    );
    assert!(
        !publisher
            .publish(RunLifecycleStatusView::with_id(
                "lifecycle:subagent_activity:root:delegate",
                "子代理",
                "subagent_activity",
                "/root/delegate 迟到运行事件",
                "running",
                None,
            ))
            .unwrap()
    );
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[test]
fn subagent_publisher_orders_terminal_against_concurrent_progress() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let event_bus: crate::run_commands::run_progress::RunProgressEventBus =
        Arc::new(move |event| {
            captured_events.lock().unwrap().push(event);
            Ok(())
        });
    let task_path = axon_agent_runtime::AgentPath::parse("/root/delegate").unwrap();
    let registry = super::super::run_subagent_lifecycle::SubagentLifecycleRegistry::new(
        Some(event_bus),
        "parent-session".to_owned(),
    );
    let publisher = registry.begin_execution(&task_path);
    let start = Arc::new(std::sync::Barrier::new(3));
    let progress = {
        let publisher = publisher.clone();
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            start.wait();
            publisher.publish(RunLifecycleStatusView::with_id(
                "lifecycle:subagent_activity:root:delegate",
                "子代理",
                "subagent_activity",
                "/root/delegate 正在运行",
                "running",
                None,
            ))
        })
    };
    let terminal = {
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            start.wait();
            publisher.publish_terminal(
                RunLifecycleStatusView::with_id(
                    "lifecycle:subagent_activity:root:delegate",
                    "子代理",
                    "subagent_activity",
                    "/root/delegate 已完成",
                    "completed",
                    None,
                )
                .completed(),
            )
        })
    };
    start.wait();
    progress.join().unwrap().unwrap();
    assert!(terminal.join().unwrap().unwrap());

    let events = events.lock().unwrap();
    let terminal_sequence = events
        .iter()
        .find_map(|event| match &event.event.kind {
            WireEventKind::TurnLifecycle(lifecycle)
                if lifecycle.status == WireLifecycleStatus::Completed =>
            {
                Some(event.event.sequence)
            }
            _ => None,
        })
        .expect("并发发布必须保留 terminal");
    assert!(events.iter().all(|event| {
        event.event.sequence <= terminal_sequence
            || !matches!(
                &event.event.kind,
                WireEventKind::TurnLifecycle(lifecycle)
                    if lifecycle.status == WireLifecycleStatus::Running
            )
    }));
}

#[test]
fn cancelled_request_stops_before_provider_channel_work() {
    let root = temp_workspace("cancel-before-provider");
    let mut request = fixture_request(root.clone());
    let cancel_token = super::super::RunCancelToken::default();
    cancel_token.cancel();
    request.cancel_token = Some(cancel_token);
    request.channel_chain = vec![(ModelConfig::default(), ProviderConfig::default())];
    let turn_inbox = crate::run_commands::run_steer::RunSteerBridge::default().open();

    let error = run_provider_single_channel(
        &request,
        &ModelConfig::default(),
        &ProviderConfig::default(),
        None,
        &turn_inbox,
    )
    .expect_err("已取消请求应直接返回 interrupted");

    assert_eq!(error.error_type, AGENT_RUN_ERROR_TYPE_INTERRUPTED);
    assert_eq!(error.message, "任务已由用户中断");
    assert!(error.tool_receipts.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn tool_lifecycle_turn_events_share_receipt_part_id() {
    let tool_call = axon_agent_runtime::ProviderToolCall::new(
        "provider-call-1",
        "builtin.edit",
        r#"{"patch":"x"}"#,
    );
    let receipt = RunToolReceipt {
        tool_call_id: 7,
        part_id: "tool-part:receipt-7".to_owned(),
        provider_call_id: "provider-call-1".to_owned(),
        tool_name: "builtin.edit".to_owned(),
        status: "completed".to_owned(),
        summary: "patch applied".to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: true,
        output_ref: None,
        duration_ms: Some(250),
    };
    let report = axon_agent_runtime::AgentTurnRuntimeReport {
        content: String::new(),
        http_status: 200,
        token_usage: None,
        tool_receipts: vec![receipt],
        turn_reports: vec![axon_agent_runtime::AgentTurnIterationReport {
            turn_id: axon_domain::TurnId(1),
            transport_report: axon_agent_runtime::ProviderStreamingTransportReport {
                runner_report: axon_agent_runtime::ProviderStreamingRunnerReport {
                    deltas: Vec::new(),
                    content: String::new(),
                    end_turn: Some(true),
                    http_status: 200,
                    provider_event_count: 0,
                    token_usage: None,
                    reasoning_content: None,
                    tool_calls: vec![tool_call],
                    server_conversation_state_ref: None,
                    max_tokens_truncated: false,
                    remote_provider_payload_allowed: false,
                    reasons: Vec::new(),
                },
                cache_shape:
                    axon_agent_runtime::ProviderVisibleCacheShape::from_serialized_request(
                        axon_agent_runtime::ProviderStreamProtocol::OpenAiCompatible,
                        r#"{"model":"test-model","tools":[],"messages":[]}"#,
                    )
                    .unwrap(),
                attempt_count: 1,
                remote_request_body_allowed: false,
                remote_provider_payload_allowed: false,
                reasons: Vec::new(),
            },
        }],
        turn_events: vec![{
            let mut event = AgentTurnEvent::new(
                axon_domain::TurnId(1),
                AgentTurnEventKind::ToolResultsRecorded,
                "tool receipts recorded: 1",
            );
            event.recorded_at_ms = 10_000;
            event
        }],
        max_tokens_truncated: false,
    };

    let events = tool_lifecycle_turn_events(&report);

    let tool_events = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                AgentTurnEventKind::ToolExecutionStarted | AgentTurnEventKind::ToolCallCompleted
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_events.len(), 2);
    assert_eq!(tool_events[0].part_id, "tool-part:receipt-7");
    assert_eq!(tool_events[1].part_id, tool_events[0].part_id);
    assert_eq!(tool_events[0].recorded_at_ms, 9_750);
    assert_eq!(tool_events[1].recorded_at_ms, 10_000);
}
