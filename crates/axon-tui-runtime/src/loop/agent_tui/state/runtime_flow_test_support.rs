//! Runtime flow 测试共享 fixture。

use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiLaunchRequest, AgentTuiRunOverrides,
    input::AgentTuiInputResult,
    run_output::{AgentTuiLifecyclePhase, AgentTuiRuntimeEvent, runtime_driver_event},
    state::{AgentTuiState, AgentTuiSubmit},
};
use crate::surface::cell::{RenderCellData, RenderCellKind, RenderCellMode, RenderCellRecord};

pub(super) use crate::r#loop::agent_tui::run_output::AgentTuiRuntimePayload;

pub(super) fn runtime_event(payload: AgentTuiRuntimePayload) -> AgentTuiRuntimeEvent {
    runtime_driver_event(payload)
}

pub(super) fn test_state() -> AgentTuiState {
    AgentTuiState::new_for_test(&AgentTuiLaunchRequest {
        cwd: axon_test_support::test_temp_root(),
        prompt: None,
        workspace_instruction_paths: Vec::new(),
        config_status: AgentTuiConfigStatus::new("fixture", "model", "已认证"),
        overrides: AgentTuiRunOverrides::default(),
        catalog: AgentTuiCatalog::default(),
        runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
    })
}

pub(super) fn test_submission(text: &str) -> AgentTuiSubmit {
    AgentTuiSubmit {
        input: AgentTuiInputResult::Submit {
            text: text.to_owned(),
            elements: Vec::new(),
        },
        overrides: AgentTuiRunOverrides::default(),
        cwd: axon_test_support::test_temp_root(),
        question_answer: None,
    }
}

pub(super) fn cell_count(state: &AgentTuiState, kind: RenderCellKind) -> usize {
    state
        .cells
        .transcript
        .iter()
        .filter(|line| cell_kind(line) == Some(kind))
        .count()
}

pub(super) fn cell_kind(line: &RenderCellRecord) -> Option<RenderCellKind> {
    match line.mode {
        RenderCellMode::Cell(kind) => Some(kind),
        RenderCellMode::Line | RenderCellMode::Startup => None,
    }
}

pub(super) fn runtime_cell_has_indicator(line: &RenderCellRecord) -> bool {
    match &line.data {
        Some(RenderCellData::Status(cell)) => cell
            .items
            .first()
            .and_then(|item| item.indicator.as_deref())
            .is_some_and(|indicator| !indicator.is_empty()),
        Some(RenderCellData::Command(cell)) => cell
            .items
            .first()
            .and_then(|item| item.indicator.as_deref())
            .is_some_and(|indicator| !indicator.is_empty()),
        Some(RenderCellData::GenerationStatus(cell)) => cell
            .items
            .first()
            .and_then(|item| item.indicator.as_deref())
            .is_some_and(|indicator| !indicator.is_empty()),
        _ => false,
    }
}

pub(super) fn lifecycle_cell_has_phase(
    state: &AgentTuiState,
    kind: RenderCellKind,
    phase: &str,
) -> bool {
    let expected = AgentTuiLifecyclePhase::from_raw(phase);
    state
        .active_transcript_lines()
        .into_iter()
        .chain(state.cells.transcript.iter())
        .any(|line| {
            cell_kind(line) == Some(kind)
                && match &line.data {
                    Some(RenderCellData::Status(cell)) => {
                        cell.items.iter().any(|item| item.phase == Some(expected))
                    }
                    Some(RenderCellData::GenerationStatus(cell)) => {
                        cell.items.iter().any(|item| item.phase == expected)
                    }
                    _ => false,
                }
        })
}

pub(super) fn active_lifecycle_cell_count(state: &AgentTuiState, kind: RenderCellKind) -> usize {
    state
        .active_transcript_lines()
        .into_iter()
        .filter(|line| cell_kind(line) == Some(kind))
        .count()
}

pub(super) fn active_lifecycle_has_indicator(state: &AgentTuiState) -> bool {
    state
        .active_transcript_lines()
        .into_iter()
        .any(runtime_cell_has_indicator)
}

pub(super) fn active_generation_summary_contains(state: &AgentTuiState, expected: &str) -> bool {
    state.active_transcript_lines().into_iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::GenerationStatus(cell))
                if cell.items.iter().any(|item| item.summary.contains(expected))
        )
    })
}

pub(super) fn typed_message_source(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Message(_)) => Some(line.text.clone()),
            _ => None,
        })
}

pub(super) fn active_message_source(state: &AgentTuiState) -> Option<String> {
    state
        .active_transcript_lines()
        .into_iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Message(_)) => Some(line.text.clone()),
            _ => None,
        })
}

pub(super) fn active_reasoning_text(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Reasoning(cell)) if !cell.done => Some(cell.text.clone()),
            _ => None,
        })
}

pub(super) fn typed_reasoning(state: &AgentTuiState) -> Option<(String, bool)> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Reasoning(cell)) => Some((cell.text.clone(), cell.done)),
            _ => None,
        })
}

pub(super) fn typed_reasoning_cells(state: &AgentTuiState) -> Vec<(String, String, bool)> {
    state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::Reasoning(cell)) => {
                Some((cell.id.clone(), cell.text.clone(), cell.done))
            }
            _ => None,
        })
        .collect()
}

pub(super) fn active_hook_item_count(state: &AgentTuiState) -> usize {
    state
        .active_transcript_lines()
        .into_iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Hook(cell)) => Some(cell.items.len()),
            _ => None,
        })
        .unwrap_or_default()
}

pub(super) fn active_hook_run_ids(state: &AgentTuiState) -> Vec<String> {
    state
        .active_transcript_lines()
        .into_iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Hook(cell)) => Some(
                cell.items
                    .iter()
                    .map(|item| item.run_id.clone())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

pub(super) fn active_hook_indicators(state: &AgentTuiState) -> Vec<Option<String>> {
    state
        .active_transcript_lines()
        .into_iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Hook(cell)) => Some(
                cell.items
                    .iter()
                    .map(|item| item.indicator.clone())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

pub(super) fn typed_exec_first_status(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Exec(cell)) => Some(cell.status.label().to_owned()),
            _ => None,
        })
}

pub(super) fn typed_exec_first_output(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Exec(cell)) => cell.output_preview.clone(),
            _ => None,
        })
}

pub(super) fn typed_exec_first_summary(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Exec(cell)) => cell.summary.clone(),
            _ => None,
        })
}

pub(super) fn typed_exec_first_duration_ms(state: &AgentTuiState) -> Option<u64> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Exec(cell)) => cell.duration_ms,
            _ => None,
        })
}

pub(super) fn typed_mcp_first_target(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Mcp(cell)) => {
                let item = cell.items.first()?;
                Some(item.tool.as_ref().map_or_else(
                    || item.server.clone(),
                    |tool| format!("{}/{}", item.server, tool),
                ))
            }
            _ => None,
        })
}

pub(super) fn typed_mcp_first_status(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Mcp(cell)) => {
                cell.items.first().map(|item| format!("{:?}", item.status))
            }
            _ => None,
        })
}

pub(super) fn typed_mcp_first_output(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Mcp(cell)) => cell
                .items
                .first()
                .and_then(|item| item.output_preview.clone()),
            _ => None,
        })
}

pub(super) fn typed_history_first_name(state: &AgentTuiState) -> Option<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::History(cell)) => cell.items.first().map(|item| item.name.clone()),
            _ => None,
        })
}
