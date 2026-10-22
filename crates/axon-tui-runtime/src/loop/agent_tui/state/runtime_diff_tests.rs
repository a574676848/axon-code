//! Runtime diff cell 投射测试。

use super::runtime_flow_test_support::{cell_count, test_state, test_submission};
use crate::surface::cell::{RenderCellData, RenderCellKind};
use crate::{
    AgentTuiDiffLine,
    r#loop::agent_tui::run_output::{AgentTuiRunLine, AgentTuiRunOutput},
};
use axon_protocol::diff::FileChange;
use std::{collections::BTreeMap, path::PathBuf};

#[test]
fn noop_diff_run_line_does_not_project_file_change_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Diff(AgentTuiDiffLine::new(
        "文件变更",
        BTreeMap::from([(
            PathBuf::from("src/lib.rs"),
            FileChange::Update {
                unified_diff: String::new(),
                move_path: None,
            },
        )]),
        0,
        0,
    )));

    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 0);
    assert!(
        !state
            .cells
            .transcript
            .iter()
            .any(|line| matches!(&line.data, Some(RenderCellData::Diff(_))))
    );
}
