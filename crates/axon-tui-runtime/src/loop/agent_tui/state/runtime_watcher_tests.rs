//! Watcher runtime 事件投影测试。

use super::runtime_flow_test_support::*;
use crate::{
    r#loop::agent_tui::{AgentTuiRuntimePayload, run_output::watcher_runtime_event},
    surface::watchers::{RuntimeWatcherEvent, RuntimeWatcherSource, RuntimeWatcherStatus},
};

#[test]
fn watcher_event_projects_through_runtime_bridge_without_transcript_noise() {
    let mut state = test_state();
    let before_cells = state.cells.transcript.len();

    state.apply_runtime_event(watcher_runtime_event(AgentTuiRuntimePayload::Watcher(
        RuntimeWatcherEvent::mcp_refresh(),
    )));

    assert_eq!(state.cells.transcript.len(), before_cells);
    assert_eq!(state.bridge.watcher_states.len(), 1);
    assert_eq!(
        state.bridge.watcher_states[0].source,
        RuntimeWatcherSource::Mcp
    );
    assert_eq!(
        state.bridge.watcher_states[0].status,
        RuntimeWatcherStatus::Ready
    );
}

#[test]
fn catalog_watchers_request_active_refresh() {
    let mut state = test_state();

    state.apply_runtime_event(watcher_runtime_event(AgentTuiRuntimePayload::Watcher(
        RuntimeWatcherEvent::mcp_refresh(),
    )));
    state.apply_runtime_event(watcher_runtime_event(AgentTuiRuntimePayload::Watcher(
        RuntimeWatcherEvent::skills_refresh(),
    )));

    assert_eq!(state.take_catalog_reload_request(), Some(true));
}
