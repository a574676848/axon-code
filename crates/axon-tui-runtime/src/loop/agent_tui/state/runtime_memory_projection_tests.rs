//! Runtime memory recall transcript 投影测试。

use super::runtime_flow_test_support::*;
use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecycleStatus, WireMemoryStatus,
    WireMemorySurfaceEntry, WireSchemaVersion, WireSessionRef, WireToolResult,
};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::r#loop::agent_tui::run_output::runtime_driver_event;
use crate::r#loop::agent_tui::wire_event_test_support::wire_event;
use crate::surface::cell::{RenderCellData, RenderCellKind};

static MEMORY_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[test]
fn memory_recall_with_surfaced_memories_projects_history_cell() {
    let mut state = test_state();

    state.apply_runtime_event(memory_status_event(vec![memory_entry(
        "memory://axon/tui",
        "TUI 重构偏好",
        "相关度 98% · same_repo",
    )]));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    let memory = memory_history_cell(&state).expect("memory history cell");
    assert_eq!(memory.title, "召回记忆 1 条");
    assert_eq!(memory.items[0].name, "TUI 重构偏好");
    assert_eq!(memory.items[0].kind_label.as_deref(), Some("记忆"));
    assert_eq!(
        memory.items[0].status_label.as_deref(),
        Some("相关度 98% · same_repo")
    );
}

#[test]
fn empty_memory_recall_does_not_project_history_cell() {
    let mut state = test_state();

    state.apply_runtime_event(memory_status_event(Vec::new()));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 0);
}

#[test]
fn memory_recall_omits_tool_guidance_refs_from_visible_history() {
    let mut state = test_state();

    state.apply_runtime_event(memory_status_event(vec![
        memory_entry(
            "memory-ref:tool-guidance:glob",
            "memory-ref:tool-guidance:In this project use builtin.glob instead of shell file discovery",
            "内部工具建议",
        ),
        memory_entry(
            "memory://axon/tui",
            "TUI 重构偏好",
            "相关度 98% · same_repo",
        ),
    ]));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    let memory = memory_history_cell(&state).expect("memory history cell");
    assert_eq!(memory.title, "召回记忆 1 条");
    assert_eq!(memory.items.len(), 1);
    assert_eq!(memory.items[0].name, "TUI 重构偏好");
}

#[test]
fn memory_recall_with_only_tool_guidance_refs_stays_hidden() {
    let mut state = test_state();

    state.apply_runtime_event(memory_status_event(vec![
        memory_entry(
            "memory-ref:tool-guidance:glob",
            "memory-ref:tool-guidance:In this project use builtin.glob instead of shell file discovery",
            "内部工具建议",
        ),
        memory_entry(
            "memory://constraint",
            "memory_tool_guidance=memory-guidance: run fmt",
            "内部运行约束",
        ),
        memory_entry(
            "memory://runtime-note",
            "<runtime-note kind=\"tool-feedback\">x</runtime-note>",
            "内部运行 note",
        ),
        memory_entry(
            "memory://checkpoint",
            "<conversation-checkpoint>summary</conversation-checkpoint>",
            "内部压缩上下文",
        ),
    ]));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 0);
}

#[test]
fn memory_recall_with_memory_tool_ref_stays_hidden() {
    let mut state = test_state();

    state.apply_runtime_event(memory_status_event(vec![memory_entry(
        "memory-tool:8d2e2d",
        "memory-tool:8d2e2d",
        "内部记忆工具引用",
    )]));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 0);
}

#[test]
fn memory_recall_omits_guidance_markers_from_summary_too() {
    let mut state = test_state();

    state.apply_runtime_event(memory_status_event(vec![
        memory_entry(
            "memory://hidden-summary",
            "看似普通的标题",
            "memory_citation_required=true",
        ),
        memory_entry("memory://visible", "用户可见偏好", "相关度 90%"),
    ]));

    let memory = memory_history_cell(&state).expect("memory history cell");
    assert_eq!(memory.items.len(), 1);
    assert_eq!(memory.items[0].name, "用户可见偏好");
}

#[test]
fn memory_recall_updates_stable_history_cell_without_spam() {
    let mut state = test_state();

    state.apply_runtime_event(memory_status_event(vec![memory_entry(
        "memory://old",
        "旧记忆",
        "相关度 80% · same_repo",
    )]));
    state.apply_runtime_event(memory_status_event(vec![memory_entry(
        "memory://new",
        "新记忆",
        "相关度 90% · same_repo",
    )]));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    let memory = memory_history_cell(&state).expect("memory history cell");
    assert_eq!(memory.items[0].name, "新记忆");
}

#[test]
fn wire_event_replay_is_dropped_before_projection() {
    let mut state = test_state();
    state.apply_runtime_event(memory_status_event_with_identity(
        vec![memory_entry("memory://replay", "同一事件", "相关度 90%")],
        "event-memory-replay",
        1,
    ));
    state.apply_runtime_event(memory_status_event_with_identity(
        vec![memory_entry("memory://replay", "同一事件", "相关度 90%")],
        "event-memory-replay",
        1,
    ));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
}

#[test]
fn final_tool_receipt_settles_once_for_one_part() {
    let mut state = test_state();
    let first = wire_event(WireEventKind::ToolResult(WireToolResult {
        part_id: "tool-part:replay".to_owned(),
        tool_call_id: "call-replay".to_owned(),
        tool_name: Some("builtin.read_file".to_owned()),
        tool_kind: Some("read_file".to_owned()),
        status: WireLifecycleStatus::Completed,
        summary: "完成".to_owned(),
        output_preview: None,
        stream: None,
        sequence: None,
        progress_phase: None,
        items_completed: None,
        items_total: None,
        current_item: None,
        duration_ms: Some(1),
        metadata: None,
        error_type: None,
        filesystem_write_performed: false,
    }));
    state.apply_runtime_event(first);
    state.apply_runtime_event(wire_event(WireEventKind::ToolResult(WireToolResult {
        part_id: "tool-part:replay".to_owned(),
        tool_call_id: "call-replay".to_owned(),
        tool_name: Some("builtin.read_file".to_owned()),
        tool_kind: Some("read_file".to_owned()),
        status: WireLifecycleStatus::Completed,
        summary: "重复完成".to_owned(),
        output_preview: None,
        stream: None,
        sequence: None,
        progress_phase: None,
        items_completed: None,
        items_total: None,
        current_item: None,
        duration_ms: Some(2),
        metadata: None,
        error_type: None,
        filesystem_write_performed: false,
    })));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
}

#[test]
fn memory_usefulness_cited_status_is_not_projected() {
    let mut state = test_state();
    state.apply_runtime_event(runtime_driver_event(
        crate::AgentTuiRuntimePayload::WireEvent {
            event: Box::new(WireEventEnvelope {
                schema_version: WireSchemaVersion::default(),
                event_id: WireCorrelationId("event-memory-usefulness".to_owned()),
                correlation_id: WireCorrelationId("turn-memory".to_owned()),
                session: WireSessionRef {
                    session_id: "session-memory".to_owned(),
                    run_id: Some("run-memory".to_owned()),
                    parent_session_id: None,
                },
                sequence: 99,
                recorded_at_ms: 99,
                kind: WireEventKind::MemoryStatus(WireMemoryStatus {
                    subject: "usefulness".to_owned(),
                    status: WireLifecycleStatus::Completed,
                    outcome: "evaluated".to_owned(),
                    summary: Some("Memory cited 2 adopted 1".to_owned()),
                    refs: vec!["memory://one".to_owned()],
                    surfaced_memories: Vec::new(),
                    metadata: None,
                }),
            }),
        },
    ));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 0);
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
}

fn memory_status_event(
    surfaced_memories: Vec<WireMemorySurfaceEntry>,
) -> crate::AgentTuiRuntimeEvent {
    let sequence = MEMORY_EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    memory_status_event_with_identity(
        surfaced_memories,
        format!("event-memory-recall-{sequence}"),
        sequence,
    )
}

fn memory_status_event_with_identity(
    surfaced_memories: Vec<WireMemorySurfaceEntry>,
    event_id: impl Into<String>,
    sequence: u64,
) -> crate::AgentTuiRuntimeEvent {
    runtime_driver_event(crate::AgentTuiRuntimePayload::WireEvent {
        event: Box::new(WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(event_id.into()),
            correlation_id: WireCorrelationId("turn-memory".to_owned()),
            session: WireSessionRef {
                session_id: "session-memory".to_owned(),
                run_id: Some("run-memory".to_owned()),
                parent_session_id: None,
            },
            sequence,
            recorded_at_ms: i64::try_from(sequence).unwrap_or(i64::MAX),
            kind: WireEventKind::MemoryStatus(WireMemoryStatus {
                subject: "recall".to_owned(),
                status: WireLifecycleStatus::Completed,
                outcome: "hybrid".to_owned(),
                summary: Some("Memory recall hit count 1".to_owned()),
                refs: Vec::new(),
                surfaced_memories,
                metadata: None,
            }),
        }),
    })
}

fn memory_entry(id: &str, title: &str, summary: &str) -> WireMemorySurfaceEntry {
    WireMemorySurfaceEntry {
        id: id.to_owned(),
        title: title.to_owned(),
        summary: summary.to_owned(),
        relevance_score: Some(0.9),
    }
}

fn memory_history_cell(
    state: &crate::r#loop::agent_tui::state::AgentTuiState,
) -> Option<&crate::surface::cell_event_data::AgentRenderHistoryCell> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::History(cell)) => Some(cell),
            _ => None,
        })
}
