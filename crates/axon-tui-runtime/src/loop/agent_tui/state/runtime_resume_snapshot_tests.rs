use std::path::PathBuf;

use axon_protocol::{
    diff::FileChange,
    wire::{
        WireAssistantMessage, WireFileChangeEvent, WireInputMode, WireInputPanelState,
        WireLifecycleStatus, WirePlanStep, WirePlanUpdate, WireReasoningMessage,
        WireRenderTimelineItem, WireRenderTimelineItemKind, WireResumeSnapshot, WireReviewStatus,
        WireSchemaVersion, WireSessionRef, WireStatusBarState, WireToolCall, WireToolResult,
        WireUsageUpdate, WireUserMessage, WireUserQuestion, WireViewportSnapshot,
    },
};
use serde_json::json;

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::{
    bottom_view::BottomView,
    run_output::AgentTuiRuntimePayload,
    state::viewport_state::{TranscriptViewBoundsCause, ViewportScrollState},
};
use crate::surface::{
    cell::{RenderCellAction, RenderCellData, RenderCellKind},
    replay_snapshot::{RESUME_LOADING_TITLE, ReplaySnapshotPhase},
};

#[test]
fn wire_resume_snapshot_restores_three_regions_and_structured_history() {
    let mut state = test_state();
    let snapshot = wire_resume_snapshot();

    prepare_resume_replay_state(&mut state);

    state.apply_runtime_event(runtime_event(AgentTuiRuntimePayload::WireResumeSnapshot(
        Box::new(snapshot),
    )));

    assert_resume_header_and_viewport(&state);
    assert_resume_cells(&state);
}

fn prepare_resume_replay_state(state: &mut crate::r#loop::agent_tui::state::AgentTuiState) {
    state.begin_resume_replay();
    assert_eq!(state.cells.transcript.len(), 1);
    assert_eq!(
        state.cells.transcript.last().map(|cell| cell.text.as_str()),
        Some(RESUME_LOADING_TITLE)
    );

    state.viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ViewportReflow,
        240,
        20,
        None,
        None,
    );
    state.viewport.detach_transcript_view_at_offset(80);
    assert_eq!(
        state.viewport.transcript_view_scroll_state,
        ViewportScrollState::JumpAvailable
    );
}

fn assert_resume_header_and_viewport(state: &crate::r#loop::agent_tui::state::AgentTuiState) {
    assert_eq!(
        state.header.overrides.session.as_deref(),
        Some("session-008")
    );
    assert!(!state.header.overrides.continue_last);
    assert_eq!(state.header.overrides.model, None);
    assert_eq!(state.header.config_status.provider, "fixture");
    assert_eq!(state.header.config_status.model, "model");
    assert_eq!(state.header.cwd, PathBuf::from("E:/zbg/axon-code"));
    assert_eq!(state.header.session_usage.total_tokens(), 42);
    assert_eq!(
        state.header.replay_snapshot.map(|state| state.phase),
        Some(ReplaySnapshotPhase::Completed)
    );
    assert_eq!(
        state.viewport.transcript_view_scroll_state,
        ViewportScrollState::JumpAvailable
    );
    assert!(state.viewport.transcript_view_jump_affordance().available);
    assert!(
        !state
            .cells
            .transcript
            .iter()
            .any(|cell| cell.text == RESUME_LOADING_TITLE)
    );
}

fn assert_resume_cells(state: &crate::r#loop::agent_tui::state::AgentTuiState) {
    assert_eq!(cell_count(state, RenderCellKind::EXEC), 1);
    assert_eq!(cell_count(state, RenderCellKind::DIFF), 1);
    assert_eq!(cell_count(state, RenderCellKind::PLAN), 1);
    assert_eq!(cell_count(state, RenderCellKind::REVIEW), 1);
    assert_eq!(cell_count(state, RenderCellKind::REASONING), 0);
    assert_eq!(cell_count(state, RenderCellKind::STATUS), 0);
    assert_eq!(cell_count(state, RenderCellKind::USAGE), 1);
    // 快照里的待决提问必须恢复成提问单元，并重新打开作答面板。
    assert_eq!(cell_count(state, RenderCellKind::QUESTION), 1);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Question(cell))
                if cell.call_id == "call-snapshot-question" && cell.status == "pending"
        )
    }));
    assert!(state.bottom_view_open(), "快照恢复后待决提问应重新弹出面板");
    let Some(BottomView::Question(view)) = state.bottom_view() else {
        panic!("快照恢复后底部面板应为提问面板");
    };
    assert_eq!(view.call_id(), "call-snapshot-question");
    assert_eq!(view.lane_id(), "session-008");
    assert!(state.cells.transcript.iter().any(|line| {
        line.text == "历史回答"
            && matches!(
                &line.data,
                Some(RenderCellData::Message(cell))
                    if cell.id == "assistant:timeline"
            )
    }));
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Plan(cell)) if cell.id == "plan-part:snapshot:restore"
        )
    }));
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Diff(cell)) if cell.id == "diff-part:snapshot:file-change"
        )
    }));
    assert!(!state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Diff(cell)) if cell.id == "diff-part:snapshot:noop-file-change"
        )
    }));
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Review(cell)) if cell.id == "review-part:snapshot:restore"
                && cell.details.len() == 2
                && cell.details[0].text == "结构化 timeline 已恢复"
                && cell.details[1].text == "Review 明细保留"
        )
    }));
}

#[test]
fn wire_resume_snapshot_settles_synthetic_resume_turn() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("继续历史会话 session-008"));
    prepare_resume_replay_state(&mut state);

    state.apply_runtime_event(runtime_event(AgentTuiRuntimePayload::WireResumeSnapshot(
        Box::new(wire_resume_snapshot()),
    )));

    assert!(!state.is_working());
    assert!(state.composer.submitted_prompt.is_none());
    assert!(!state.composer.input_queue.user_turn_pending_start);
    assert!(state.collect_invariant_violations().is_empty());
}

#[test]
fn wire_resume_snapshot_rejects_mismatched_payload_part_id() {
    let mut state = test_state();
    let mut snapshot = wire_resume_snapshot();
    let WireRenderTimelineItemKind::ReasoningMessage(reasoning) = &mut snapshot.timeline[2].kind
    else {
        panic!("reasoning fixture missing");
    };
    reasoning.part_id = "reasoning:payload-mismatch".to_owned();

    state.apply_runtime_event(runtime_event(AgentTuiRuntimePayload::WireResumeSnapshot(
        Box::new(snapshot),
    )));

    assert_eq!(
        state.header.replay_snapshot.map(|state| state.phase),
        Some(ReplaySnapshotPhase::Error)
    );
    assert_eq!(cell_count(&state, RenderCellKind::REASONING), 0);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Error(cell))
                if cell.details.iter().any(|detail| detail.contains("wire resume snapshot part_id 不一致"))
        )
    }));
}

#[test]
fn wire_resume_snapshot_rejects_mismatched_review_payload_part_id() {
    let mut state = test_state();
    let mut snapshot = wire_resume_snapshot();
    let WireRenderTimelineItemKind::ReviewStatus(review) = &mut snapshot.timeline[7].kind else {
        panic!("review fixture missing");
    };
    review.part_id = "review-part:payload-mismatch".to_owned();

    state.apply_runtime_event(runtime_event(AgentTuiRuntimePayload::WireResumeSnapshot(
        Box::new(snapshot),
    )));

    assert_eq!(
        state.header.replay_snapshot.map(|state| state.phase),
        Some(ReplaySnapshotPhase::Error)
    );
    assert_eq!(cell_count(&state, RenderCellKind::REVIEW), 0);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Error(cell))
                if cell.details.iter().any(|detail| detail.contains("wire resume snapshot part_id 不一致"))
        )
    }));
}

#[test]
fn wire_resume_snapshot_filters_shell_wrapper_from_restored_tool_arguments() {
    let mut state = test_state();
    let mut snapshot = wire_resume_snapshot();
    let WireRenderTimelineItemKind::ToolResult(result) = &mut snapshot.timeline[4].kind else {
        panic!("tool result fixture missing");
    };
    result.summary =
        "shell command=pwsh -NoLogo -NoProfile -NonInteractive -Command exit_code=0".to_owned();
    result.metadata = Some(json!({
        "argumentsPreview": "pwsh -NoLogo -NoProfile -NonInteractive -Command try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {} Get-ChildItem"
    }));

    state.apply_runtime_event(runtime_event(AgentTuiRuntimePayload::WireResumeSnapshot(
        Box::new(snapshot),
    )));

    let exec_cell = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Exec(cell)) => Some(cell),
            _ => None,
        })
        .expect("restored tool result should render as exec cell");
    assert_eq!(
        exec_cell.arguments_preview.as_deref(),
        Some("Get-ChildItem")
    );
    assert_eq!(exec_cell.summary, None);
}

#[test]
fn wire_resume_snapshot_restores_in_progress_tool_as_running_cell() {
    let mut state = test_state();
    let mut snapshot = wire_resume_snapshot();
    snapshot.timeline = vec![item(
        0,
        "tool-part:running",
        WireRenderTimelineItemKind::ToolCall(WireToolCall {
            part_id: "tool-part:running".to_owned(),
            tool_call_id: "call-running".to_owned(),
            request_id: None,
            tool_name: "mcp.search".to_owned(),
            tool_kind: "mcp.search".to_owned(),
            summary: "工具调用：mcp.search".to_owned(),
            arguments_summary: Some(r#"{"query":"恢复"}"#.to_owned()),
            intent: None,
            execution_permission: None,
        }),
    )];

    state.apply_runtime_event(runtime_event(AgentTuiRuntimePayload::WireResumeSnapshot(
        Box::new(snapshot),
    )));

    let running = state
        .cells
        .transcript
        .iter()
        .find(|line| line.action == RenderCellAction::Running)
        .expect("InProgress tool-call 应恢复为 running cell");
    let Some(RenderCellData::Exec(cell)) = &running.data else {
        panic!("running cell 应保留结构化 exec data");
    };
    assert_eq!(cell.id, "tool-part:running");
    assert_eq!(cell.provider_call_id.as_deref(), Some("call-running"));
}

fn wire_resume_snapshot() -> WireResumeSnapshot {
    WireResumeSnapshot {
        schema_version: WireSchemaVersion::default(),
        session: WireSessionRef {
            session_id: "session-008".to_owned(),
            run_id: None,
            parent_session_id: Some("parent-007".to_owned()),
        },
        status_bar: status_bar(),
        viewport: WireViewportSnapshot::default(),
        input_panel: input_panel(),
        timeline: timeline(),
    }
}

fn status_bar() -> WireStatusBarState {
    WireStatusBarState {
        title: "Axon".to_owned(),
        subtitle: Some("session-008".to_owned()),
        lifecycle_status: WireLifecycleStatus::Completed,
        cwd: Some("E:/zbg/axon-code".to_owned()),
        provider: Some("openai".to_owned()),
        model: Some("gpt-5".to_owned()),
        selected_skill_name: None,
        usage: Some(usage()),
    }
}

fn input_panel() -> WireInputPanelState {
    WireInputPanelState {
        mode: WireInputMode::Prompt,
        draft: None,
    }
}

fn timeline() -> Vec<WireRenderTimelineItem> {
    vec![
        item(
            0,
            "user:timeline",
            WireRenderTimelineItemKind::UserMessage(WireUserMessage {
                part_id: "user:timeline".to_owned(),
                message_id: "user:payload".to_owned(),
                content: "历史问题".to_owned(),
                local_files: Vec::new(),
            }),
        ),
        item(
            1,
            "assistant:timeline",
            WireRenderTimelineItemKind::AssistantMessage(WireAssistantMessage {
                part_id: "assistant:timeline".to_owned(),
                message_id: "assistant:payload".to_owned(),
                content: "历史回答".to_owned(),
                delta: false,
            }),
        ),
        item(
            2,
            "reasoning:timeline-b",
            WireRenderTimelineItemKind::ReasoningMessage(WireReasoningMessage {
                part_id: "reasoning:timeline-b".to_owned(),
                reasoning_id: "reasoning:payload-b".to_owned(),
                content: "历史 reasoning B".to_owned(),
                done: true,
            }),
        ),
        item(
            3,
            "reasoning:timeline-a",
            WireRenderTimelineItemKind::ReasoningMessage(WireReasoningMessage {
                part_id: "reasoning:timeline-a".to_owned(),
                reasoning_id: "reasoning:payload-a".to_owned(),
                content: "历史 reasoning A".to_owned(),
                done: true,
            }),
        ),
        item(
            4,
            "tool-part:snapshot:result",
            WireRenderTimelineItemKind::ToolResult(tool_result()),
        ),
        item(
            5,
            "diff-part:snapshot:file-change",
            WireRenderTimelineItemKind::FileChange(file_change()),
        ),
        item(
            6,
            "plan-part:snapshot:restore",
            WireRenderTimelineItemKind::PlanUpdate(plan_update()),
        ),
        item(
            7,
            "review-part:snapshot:restore",
            WireRenderTimelineItemKind::ReviewStatus(review_status()),
        ),
        item(
            8,
            "usage:session-008",
            WireRenderTimelineItemKind::UsageUpdate(usage()),
        ),
        item(
            9,
            "error:resume",
            WireRenderTimelineItemKind::Error(axon_domain::RuntimeFailure::new(
                "runtime",
                "runtime_failed",
                "历史错误",
                axon_domain::RUNTIME_FAILURE_TIMESTAMP_UNKNOWN,
            )),
        ),
        item(
            10,
            "diff-part:snapshot:noop-file-change",
            WireRenderTimelineItemKind::FileChange(noop_file_change()),
        ),
        item(
            11,
            "question-part:snapshot:pending",
            WireRenderTimelineItemKind::UserQuestion(pending_question()),
        ),
    ]
}

fn pending_question() -> WireUserQuestion {
    WireUserQuestion {
        part_id: "question-part:snapshot:pending".to_owned(),
        call_id: "call-snapshot-question".to_owned(),
        status: "pending".to_owned(),
        questions: vec![axon_domain::UserQuestionItem {
            id: "q1".to_owned(),
            question: "快照恢复后是否继续？".to_owned(),
            header: Some("恢复确认".to_owned()),
            detail: None,
            options: vec![
                axon_domain::UserQuestionOption {
                    label: "继续".to_owned(),
                    description: None,
                },
                axon_domain::UserQuestionOption {
                    label: "暂停".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        answers: Vec::new(),
        handoff: None,
        deadline_at_ms: 1_700_000_000_000,
    }
}

fn tool_result() -> WireToolResult {
    WireToolResult {
        part_id: "tool-part:snapshot:result".to_owned(),
        tool_call_id: "shell_command".to_owned(),
        tool_name: Some("shell_command".to_owned()),
        tool_kind: Some("shell".to_owned()),
        status: WireLifecycleStatus::Completed,
        summary: "完成".to_owned(),
        output_preview: Some("ok".to_owned()),
        stream: None,
        sequence: None,
        progress_phase: None,
        items_completed: None,
        items_total: None,
        current_item: None,
        duration_ms: None,
        metadata: None,
        error_type: None,
        filesystem_write_performed: false,
    }
}

fn file_change() -> WireFileChangeEvent {
    WireFileChangeEvent {
        part_id: "diff-part:snapshot:file-change".to_owned(),
        tool_part_id: Some("tool-part:snapshot:file-change-source".to_owned()),
        tool_call_id: Some("tool-1".to_owned()),
        path: "src/restored.rs".to_owned(),
        change: FileChange::Add {
            content: "pub fn restored() {}\n".to_owned(),
        },
    }
}

fn noop_file_change() -> WireFileChangeEvent {
    WireFileChangeEvent {
        part_id: "diff-part:snapshot:noop-file-change".to_owned(),
        tool_part_id: Some("tool-part:snapshot:noop-file-change-source".to_owned()),
        tool_call_id: Some("tool-noop".to_owned()),
        path: "src/unchanged.rs".to_owned(),
        change: FileChange::Update {
            unified_diff: String::new(),
            move_path: None,
        },
    }
}

fn plan_update() -> WirePlanUpdate {
    WirePlanUpdate {
        part_id: "plan-part:snapshot:restore".to_owned(),
        title: "恢复计划".to_owned(),
        explanation: None,
        steps: vec![WirePlanStep {
            step: "完成 snapshot 契约".to_owned(),
            status: "completed".to_owned(),
            cycle: None,
            diagnostic_count: 0,
            failing_step_count: 0,
        }],
    }
}

fn review_status() -> WireReviewStatus {
    WireReviewStatus {
        part_id: "review-part:snapshot:restore".to_owned(),
        subject: "恢复评审".to_owned(),
        status: WireLifecycleStatus::Completed,
        detail: Some("不应覆盖 details".to_owned()),
        details: vec![
            "结构化 timeline 已恢复".to_owned(),
            "Review 明细保留".to_owned(),
        ],
    }
}

fn usage() -> WireUsageUpdate {
    WireUsageUpdate {
        input_tokens: 10,
        output_tokens: 12,
        cached_input_tokens: 20,
        cache_write_input_tokens: 0,
        reasoning_output_tokens: 0,
        current_context_tokens: 42,
        system_prompt_tokens: 0,
        tool_definition_tokens: 0,
        conversation_tokens: 0,
    }
}

fn item(sequence: u64, part_id: &str, kind: WireRenderTimelineItemKind) -> WireRenderTimelineItem {
    WireRenderTimelineItem {
        part_id: part_id.to_owned(),
        sequence,
        recorded_at_ms: 1,
        kind,
    }
}
