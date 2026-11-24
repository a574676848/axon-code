use super::{ActiveRuntimeCells, ActiveRuntimePart};
use crate::surface::{
    cell::RenderCellRecord,
    cell_event_data::{AgentRenderHookCell, AgentRenderHookItem, AgentRenderLifecycleStatus},
};

#[test]
fn slot_kind_transition_keeps_one_active_record_per_part() {
    let mut cells = ActiveRuntimeCells::default();
    cells.set_command_cell("tool-part:test:1".to_owned(), 3);
    cells.set_history_lifecycle_cell("tool-part:test:2".to_owned(), 4);
    cells.set_tool_cell("tool-part:test:1".to_owned(), 8, std::time::Instant::now());

    cells.remove_transcript_index(2);

    assert_eq!(cells.take_command_cell("tool-part:test:1"), None);
    assert_eq!(
        cells.take_history_lifecycle_cell("tool-part:test:2"),
        Some(ActiveRuntimePart {
            part_id: "tool-part:test:2".to_owned(),
            index: 3
        })
    );
    assert_eq!(cells.tool_cell("tool-part:test:1"), Some(7));
}

#[test]
fn active_runtime_parts_expose_part_identity_and_index() {
    let mut cells = ActiveRuntimeCells::default();
    cells.set_command_cell("command-part:test".to_owned(), 2);
    cells.set_history_lifecycle_cell("history-part:test".to_owned(), 3);
    cells.set_tool_cell("tool-part:test".to_owned(), 4, std::time::Instant::now());

    assert_eq!(
        cells.active_runtime_parts(),
        vec![
            ActiveRuntimePart {
                part_id: "command-part:test".to_owned(),
                index: 2,
            },
            ActiveRuntimePart {
                part_id: "history-part:test".to_owned(),
                index: 3,
            },
            ActiveRuntimePart {
                part_id: "tool-part:test".to_owned(),
                index: 4,
            },
        ]
    );
}

#[test]
fn active_tool_cells_expose_started_at_for_tick_updates() {
    let mut cells = ActiveRuntimeCells::default();
    let now = std::time::Instant::now();
    cells.set_tool_cell("tool-part:test:started".to_owned(), 3, now);

    assert_eq!(
        cells.active_tool_cells(),
        vec![super::ActiveToolCell {
            index: 3,
            started_at: Some(now),
        }]
    );
}

#[test]
fn clear_tool_cell_at_index_removes_matching_active_tool() {
    let mut cells = ActiveRuntimeCells::default();
    let now = std::time::Instant::now();
    cells.set_tool_cell("tool-part:test:clear".to_owned(), 5, now);
    assert_eq!(cells.tool_cell("tool-part:test:clear"), Some(5));

    cells.clear_tool_cell_at_index(5);
    assert_eq!(cells.tool_cell("tool-part:test:clear"), None);
    assert!(cells.active_tool_cells().is_empty());
}

#[test]
fn reveal_due_hook_cells_drains_ready_pending_cells() {
    let mut cells = ActiveRuntimeCells::default();
    let now = std::time::Instant::now();
    cells.stage_hook_cell("ready".to_owned(), hook_line("ready"), now);
    cells.stage_hook_cell(
        "later".to_owned(),
        hook_line("later"),
        now + std::time::Duration::from_secs(1),
    );

    let revealed = cells.reveal_due_hook_cells(now);

    assert_eq!(revealed.len(), 1);
    assert_eq!(revealed[0].run_id, "ready");
    assert!(cells.take_pending_hook_cell("later").is_some());
}

#[test]
fn quiet_hook_lingers_drain_after_deadline() {
    let mut cells = ActiveRuntimeCells::default();
    let now = std::time::Instant::now();
    cells.stage_quiet_hook_linger("ready".to_owned(), now);
    cells.stage_quiet_hook_linger("later".to_owned(), now + std::time::Duration::from_secs(1));

    assert_eq!(
        cells.take_due_quiet_hook_lingers(now),
        vec!["ready".to_owned()]
    );
    assert!(cells.take_due_quiet_hook_lingers(now).is_empty());
    assert_eq!(
        cells.take_due_quiet_hook_lingers(now + std::time::Duration::from_secs(1)),
        vec!["later".to_owned()]
    );
}

fn hook_line(name: &str) -> RenderCellRecord {
    RenderCellRecord::hook_with_data(AgentRenderHookCell {
        id: "hook:test:runtime-cells".to_owned(),
        title: "Hook".to_owned(),
        items: vec![AgentRenderHookItem {
            run_id: "hook".to_owned(),
            name: name.to_owned(),
            status: AgentRenderLifecycleStatus::Running,
            indicator: None,
            status_message: None,
            entries: Vec::new(),
        }],
    })
}

#[test]
fn remove_transcript_index_drops_matching_active_cells() {
    let mut cells = ActiveRuntimeCells::default();
    cells.set_command_cell("tool-part:test:drop".to_owned(), 2);
    cells.set_history_lifecycle_cell("tool-part:test:drop".to_owned(), 4);
    cells.set_tool_cell(
        "tool-part:test:drop".to_owned(),
        7,
        std::time::Instant::now(),
    );
    cells.set_hook_visible_since("pre".to_owned(), std::time::Instant::now());

    cells.remove_transcript_index(7);
    cells.remove_transcript_index(6);
    cells.remove_transcript_index(4);
    cells.remove_transcript_index(2);

    assert_eq!(cells.take_command_cell("tool-part:test:drop"), None);
    assert_eq!(
        cells.take_history_lifecycle_cell("tool-part:test:drop"),
        None
    );
    assert_eq!(cells.tool_cell("tool-part:test:drop"), None);
    assert!(cells.hook_visible_since("pre").is_some());
}

#[test]
fn command_lifecycle_part_is_tracked_by_identity() {
    let mut cells = ActiveRuntimeCells::default();
    cells.set_command_cell("tool-part:test:1".to_owned(), 2);

    assert_eq!(
        cells.take_command_cell("tool-part:test:1"),
        Some(ActiveRuntimePart {
            part_id: "tool-part:test:1".to_owned(),
            index: 2
        })
    );
}

#[test]
fn command_lifecycle_part_does_not_replace_other_parts() {
    let mut cells = ActiveRuntimeCells::default();
    cells.set_command_cell("tool-part:test:1".to_owned(), 2);

    assert_eq!(cells.take_command_cell("tool-part:test:other"), None);
    assert_eq!(
        cells.take_command_cell("tool-part:test:1"),
        Some(ActiveRuntimePart {
            part_id: "tool-part:test:1".to_owned(),
            index: 2
        })
    );
}
