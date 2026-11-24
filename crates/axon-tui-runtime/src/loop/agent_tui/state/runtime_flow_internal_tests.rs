use crate::r#loop::agent_tui::{
    AgentTuiDiffLine,
    run_output::AgentTuiRunLine,
    state::runtime_flow_test_support::{cell_count, test_state},
};
use crate::surface::cell::{RenderCellData, RenderCellKind};
use axon_protocol::diff::FileChange;
use std::{collections::BTreeMap, path::PathBuf};

#[test]
fn direct_diff_run_line_projects_aggregate_file_change_cell() {
    let mut state = test_state();

    state.apply_run_line(AgentTuiRunLine::Diff(AgentTuiDiffLine::new(
        "文件变更",
        BTreeMap::from([
            (
                PathBuf::from("src/lib.rs"),
                FileChange::Update {
                    unified_diff: "@@ -1 +1 @@\n-old\n+new".to_owned(),
                    move_path: None,
                },
            ),
            (
                PathBuf::from("src/new.rs"),
                FileChange::Add {
                    content: "new\n".to_owned(),
                },
            ),
            (
                PathBuf::from("src/old.rs"),
                FileChange::Delete {
                    content: "old\n".to_owned(),
                },
            ),
        ]),
        1,
        0,
    )));

    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 1);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(&line.data, Some(RenderCellData::Diff(cell)) if cell.changes.len() == 3)
    }));
}

#[test]
fn session_run_line_updates_current_override_for_followup_commands() {
    let mut state = test_state();

    state.apply_run_line(AgentTuiRunLine::Session(
        crate::r#loop::agent_tui::AgentTuiRunSessionInfo::new("session-42".to_owned(), None),
    ));

    assert_eq!(
        state.header.overrides.session.as_deref(),
        Some("session-42")
    );
    assert!(!state.header.overrides.continue_last);
}
