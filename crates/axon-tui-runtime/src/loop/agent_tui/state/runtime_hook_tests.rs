//! 运行期 Hook transcript cell 测试。

use std::time::{Duration, Instant};

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::run_output::{
    AgentTuiHookOutputEntry, AgentTuiHookOutputKind, AgentTuiHookStatusView,
};
use crate::surface::cell::{RenderCellData, RenderCellKind};

#[test]
fn quiet_hook_success_does_not_persist_in_transcript() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行 hook"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::new("PostToolUse", "running", Some("检查输出".to_owned())),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::new("PostToolUse", "completed", None),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
}

#[test]
fn slow_running_hook_reveals_after_delay_with_indicator() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行 hook"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::new("PostToolUse", "running", Some("检查输出".to_owned())),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);

    assert!(state.tick(Instant::now() + Duration::from_secs(1)));

    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(active_hook_item_count(&state), 1);
    let indicators = active_hook_indicators(&state);
    assert_eq!(indicators.len(), 1);
    assert!(
        indicators[0]
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    );
}

#[test]
fn same_name_hooks_are_tracked_by_run_id() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行 hook"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::with_run_id(
                "run-a",
                "PostToolUse",
                "running",
                Some("检查输出".to_owned()),
            ),
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::with_run_id(
                "run-b",
                "PostToolUse",
                "running",
                Some("检查输出".to_owned()),
            ),
        ),
    );
    let revealed_at = Instant::now() + Duration::from_secs(1);
    assert!(state.tick(revealed_at));
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(active_hook_item_count(&state), 2);

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::with_run_id("run-a", "PostToolUse", "completed", None),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(active_hook_item_count(&state), 2);
    assert!(state.tick(revealed_at + Duration::from_secs(1)));
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(active_hook_item_count(&state), 1);
    assert_eq!(active_hook_run_ids(&state), vec!["run-b".to_owned()]);
}

#[test]
fn visible_quiet_hook_success_lingers_before_removal() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行 hook"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::new("PostToolUse", "running", Some("检查输出".to_owned())),
        ),
    );
    let revealed_at = Instant::now() + Duration::from_secs(1);
    assert!(state.tick(revealed_at));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::new("PostToolUse", "completed", None),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(active_hook_item_count(&state), 1);
    assert!(state.tick(revealed_at + Duration::from_secs(1)));
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(active_hook_item_count(&state), 0);
}

#[test]
fn completed_hook_with_short_status_persists_compact_history() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行 hook"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::with_run_id(
                "run-a",
                "Skill",
                "completed",
                Some("已增强 draft-vibe-coding".to_owned()),
            ),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::History(cell))
                if cell.items.len() == 1
                    && cell.items[0].name == "已增强 draft-vibe-coding"
                    && cell.items[0].kind_label.as_deref() == Some("Skill")
        )
    }));
}

#[test]
fn completed_skill_hook_with_status_drops_output_entry_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("反思 Skill"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::with_run_id_and_entries(
                "skill-run",
                "Skill",
                "completed",
                Some("已增强 draft-vibe-coding".to_owned()),
                vec![AgentTuiHookOutputEntry::new(
                    AgentTuiHookOutputKind::Feedback,
                    "- builtin_read_file\n- builtin_search\n- builtin_glob",
                )],
            ),
        ),
    );
    let transcript_text = state
        .cells
        .transcript
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert!(transcript_text.contains("Hook"));
    assert!(!transcript_text.contains("builtin_read_file"));
    assert!(state.cells.transcript.iter().any(|line| matches!(
        &line.data,
        Some(RenderCellData::History(cell))
            if cell.items.len() == 1
                && cell.items[0].name == "已增强 draft-vibe-coding"
                && cell.items[0].kind_label.as_deref() == Some("Skill")
    )));
}

#[test]
fn hook_failure_persists_as_structured_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行 hook"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::new("PreToolUse", "failed", Some("权限不足".to_owned())),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HOOK), 0);
    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert!(
        state.cells.transcript.iter().any(|line| {
            line.text == "Hook" && cell_kind(line) == Some(RenderCellKind::HISTORY)
        })
    );
    assert!(state.cells.transcript.iter().any(|line| matches!(
        &line.data,
        Some(RenderCellData::History(cell))
            if cell.items.len() == 1
                && cell.items[0].name == "权限不足"
                && cell.items[0].kind_label.as_deref() == Some("PreToolUse")
                && cell.items[0].status_label.as_deref() == Some("失败")
    )));
}
