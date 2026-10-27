//! 终态 wire 事件的内容断言。

use super::*;

fn builder() -> RunWireEventBuilder {
    RunWireEventBuilder {
        session: WireSessionRef {
            session_id: "session-1".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        recorded_at_ms: 1,
        next_sequence: 1,
        events: Vec::new(),
    }
}

#[test]
fn wire_builder_emits_reasoning_before_final_assistant_part() {
    let mut builder = RunWireEventBuilder {
        session: WireSessionRef {
            session_id: "session-1".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        recorded_at_ms: 1,
        next_sequence: 1,
        events: Vec::new(),
    };

    builder.push_reasoning_messages(&[AgentRunReasoningMessage {
        part_id: "reasoning-part:1".to_owned(),
        reasoning_id: "provider-reasoning:1".to_owned(),
        content: "先分析".to_owned(),
    }]);
    builder.push_assistant_message("assistant-part:1", "最终回答");

    let events = builder.into_events();
    assert!(matches!(
        &events[0].kind,
        WireEventKind::ReasoningStart(reasoning)
            if reasoning.part_id == "reasoning-part:1"
                && reasoning.reasoning_id == "provider-reasoning:1"
    ));
    assert!(matches!(
        &events[1].kind,
        WireEventKind::ReasoningDelta(reasoning)
            if reasoning.part_id == "reasoning-part:1"
                && reasoning.reasoning_id == "provider-reasoning:1"
                && reasoning.delta == "先分析"
    ));
    assert!(matches!(
        &events[2].kind,
        WireEventKind::ReasoningEnd(reasoning)
            if reasoning.part_id == "reasoning-part:1"
                && reasoning.reasoning_id == "provider-reasoning:1"
    ));
    assert!(matches!(
        &events[3].kind,
        WireEventKind::AssistantMessage(message)
            if message.part_id == "assistant-part:1"
                && message.message_id == ASSISTANT_FINAL_MESSAGE_ID
                && message.content == "最终回答"
                && !message.delta
    ));
}

#[test]
fn wire_builder_emits_content_before_terminal_lifecycle() {
    let mut builder = RunWireEventBuilder {
        session: WireSessionRef {
            session_id: "session-1".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        recorded_at_ms: 1,
        next_sequence: 1,
        events: Vec::new(),
    };

    builder.push_assistant_message("assistant-part:1", "最终回答");
    builder.push_run_lifecycle(
        WireLifecycleStatus::Completed,
        "Run completed",
        None,
        Vec::new(),
    );

    let events = builder.into_events();
    assert!(matches!(
        &events[0].kind,
        WireEventKind::AssistantMessage(message)
            if message.part_id == "assistant-part:1"
                && message.message_id == ASSISTANT_FINAL_MESSAGE_ID
                && message.content == "最终回答"
                && !message.delta
    ));
    assert!(matches!(
        &events[1].kind,
        WireEventKind::TurnLifecycle(lifecycle)
            if lifecycle.status == WireLifecycleStatus::Completed
                && lifecycle.kind.as_deref() == Some("RunLifecycle")
    ));
}

#[test]
fn wire_builder_emits_empty_canonical_assistant_before_terminal_lifecycle() {
    let mut builder = RunWireEventBuilder {
        session: WireSessionRef {
            session_id: "session-1".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        recorded_at_ms: 1,
        next_sequence: 1,
        events: Vec::new(),
    };

    builder.push_assistant_message("assistant-part:1", "   ");
    builder.push_run_lifecycle(
        WireLifecycleStatus::Completed,
        "Run completed",
        None,
        Vec::new(),
    );

    let events = builder.into_events();
    assert_eq!(events.len(), 2, "空正文回合仍必须产出 canonical assistant");
    assert!(matches!(
        &events[0].kind,
        WireEventKind::AssistantMessage(message)
            if message.part_id == "assistant-part:1"
                && message.content.trim().is_empty()
                && !message.delta
    ));
    assert!(matches!(
        &events[1].kind,
        WireEventKind::TurnLifecycle(lifecycle)
            if lifecycle.status == WireLifecycleStatus::Completed
                && lifecycle.kind.as_deref() == Some("RunLifecycle")
    ));
}

#[test]
fn wire_builder_uses_receipt_part_id_for_tool_events() {
    let mut builder = RunWireEventBuilder {
        session: WireSessionRef {
            session_id: "session-1".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        recorded_at_ms: 1,
        next_sequence: 1,
        events: Vec::new(),
    };

    builder.push_tool_receipts(&[RunToolReceipt {
        tool_call_id: 7,
        part_id: "tool-part:receipt-stable".to_owned(),
        provider_call_id: "provider-call-7".to_owned(),
        tool_name: "builtin.search".to_owned(),
        status: "completed".to_owned(),
        summary: "ok".to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: None,
    }]);

    let events = builder.into_events();
    assert!(matches!(
        &events[0].kind,
        WireEventKind::ToolCall(call)
            if call.part_id == "tool-part:receipt-stable"
                && call.tool_call_id == "provider-call-7"
    ));
    assert!(matches!(
        &events[1].kind,
        WireEventKind::ToolResult(result)
            if result.part_id == "tool-part:receipt-stable"
                && result.tool_call_id == "provider-call-7"
    ));
}

#[test]
fn terminal_lifecycle_carries_persona_warnings_on_both_paths() {
    let warnings = vec![
        "人格稳定段接近上限（5000 B）".to_owned(),
        "人格状态未装配：人格库不存在".to_owned(),
    ];

    // 完成路径：只有终态条目带告警，且被识别为终态。
    let mut completed = builder();
    completed.push_run_lifecycle(
        WireLifecycleStatus::Completed,
        "Run completed",
        None,
        warnings.clone(),
    );
    let events = completed.into_events();
    let WireEventKind::TurnLifecycle(lifecycle) = &events[0].kind else {
        panic!("完成事件必须是 lifecycle");
    };
    assert_eq!(lifecycle.warnings, warnings);
    assert!(lifecycle.is_terminal_run(), "完成条目必须判为终态");

    // 失败路径：同样携带，且同样判为终态。
    let mut failed = builder();
    failed.push_run_lifecycle(
        WireLifecycleStatus::Failed,
        "Run failed",
        Some("boom".to_owned()),
        warnings.clone(),
    );
    let events = failed.into_events();
    let WireEventKind::TurnLifecycle(lifecycle) = &events[0].kind else {
        panic!("失败事件必须是 lifecycle");
    };
    assert_eq!(lifecycle.warnings, warnings);
    assert!(lifecycle.is_terminal_run(), "失败条目必须判为终态");
}

#[test]
fn process_lifecycle_is_not_mistaken_for_terminal_run() {
    let mut process = builder();
    process.push_run_lifecycle(
        WireLifecycleStatus::Running,
        "Run started",
        None,
        Vec::new(),
    );
    let events = process.into_events();
    let WireEventKind::TurnLifecycle(lifecycle) = &events[0].kind else {
        panic!("必须是 lifecycle");
    };
    assert!(!lifecycle.is_terminal_run(), "进行中条目不得判为终态");
}

#[test]
fn lifecycle_is_terminal_only_for_run_lifecycle_kind() {
    let mut other = builder();
    other.push_run_lifecycle(
        WireLifecycleStatus::Completed,
        "Subagent done",
        None,
        vec!["子代理告警".to_owned()],
    );
    let events = other.into_events();
    let WireEventKind::TurnLifecycle(mut lifecycle) = events[0].kind.clone() else {
        panic!("必须是 lifecycle");
    };
    lifecycle.kind = Some("SubagentLifecycle".to_owned());
    assert!(
        !lifecycle.is_terminal_run(),
        "非 RunLifecycle 类别的条目不得被当成整轮终态"
    );

    lifecycle.kind = Some("RunLifecycle".to_owned());
    lifecycle.phase = WireLifecyclePhase::Governance;
    assert!(
        !lifecycle.is_terminal_run(),
        "非普通轮阶段的条目不得被当成整轮终态"
    );
}
