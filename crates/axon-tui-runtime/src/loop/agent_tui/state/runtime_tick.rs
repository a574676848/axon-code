//! 运行期 tick、spinner 与延迟 hook 显示。

use std::time::Instant;

use crate::r#loop::agent_tui::{
    run_output::{AgentTuiRuntimePayload, watcher_runtime_event},
    state::AgentTuiState,
    streaming::commit_tick::{CommitTickScope, run_commit_tick},
};
use crate::surface::animation::{
    axon_logo_animation_frame_count, spinner_frame, transcript_motion_tokens,
};
use crate::surface::{cell::RenderCellData, cell_event_data::AgentRenderExecStatus};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn tick(&mut self, now: Instant) -> bool {
        let motion = transcript_motion_tokens();
        debug_assert!(motion.stays_within_tui_budget());
        let before = self.invariant_snapshot();
        let watcher_changed = self.drain_due_watcher_events(now);
        let popup_completed = self.poll_composer_path_completion();
        let hook_revealed = self.reveal_due_hook_cells(now);
        let stream_committed = self.commit_streaming_lines(now);
        let dock_advanced = self.advance_bottom_dock_lifecycle(now, motion.dock);
        let toast_expired = self.toast.expire_if_due(now);
        let provider_activity_advanced = self.bridge.provider_activity.advance(now);
        let activity_visible = self.has_visible_subagent_activity(now);
        if (!self.is_working() && !activity_visible) || now < self.bridge.next_spinner_tick {
            self.debug_assert_transaction_invariants("tick", before);
            return popup_completed
                || hook_revealed
                || stream_committed
                || dock_advanced
                || watcher_changed
                || toast_expired
                || provider_activity_advanced;
        }
        self.bridge.spinner_frame =
            (self.bridge.spinner_frame + 1) % axon_logo_animation_frame_count();
        self.update_active_status_indicator();
        self.update_active_exec_durations(now);
        self.bridge.next_spinner_tick = now + motion.spinner;
        self.debug_assert_transaction_invariants("tick", before);
        true
    }

    fn drain_due_watcher_events(&mut self, now: Instant) -> bool {
        let config_path = self
            .header
            .config_status
            .config_path
            .clone()
            .map(std::path::PathBuf::from);
        let cwd = std::path::PathBuf::from(self.header.cwd.clone());
        let events = self
            .bridge
            .watchers
            .drain_due_events(now, config_path.as_deref(), &cwd);
        let changed = !events.is_empty();
        for event in events {
            self.apply_runtime_event(watcher_runtime_event(AgentTuiRuntimePayload::Watcher(
                event,
            )));
        }
        changed
    }

    pub(in crate::r#loop::agent_tui) fn spinner(&self) -> &'static str {
        spinner_frame(self.bridge.spinner_frame)
    }

    pub(in crate::r#loop::agent_tui) fn animation_tick(&self) -> u64 {
        self.bridge.spinner_frame as u64
    }

    fn reveal_due_hook_cells(&mut self, now: Instant) -> bool {
        let due_cells = self.cells.active_runtime_cells.reveal_due_hook_cells(now);
        let due_lingers = self
            .cells
            .active_runtime_cells
            .take_due_quiet_hook_lingers(now);
        if due_cells.is_empty() && due_lingers.is_empty() {
            return false;
        }
        for run_id in due_lingers {
            self.remove_active_hook_cell(&run_id);
        }
        for cell in due_cells {
            self.merge_or_replace_live_hook_line(cell.line);
            self.cells
                .active_runtime_cells
                .set_hook_visible_since(cell.run_id, cell.visible_since);
        }
        true
    }

    fn commit_streaming_lines(&mut self, now: Instant) -> bool {
        let assistant_part_id = self.bridge.assistant_streams.next_commit_part_id();
        let output = {
            let assistant_streams = &mut self.bridge.assistant_streams;
            let stream_chunking_policy = &mut self.bridge.stream_chunking_policy;
            let plan_stream_controller = &mut self.bridge.plan_stream_controller;
            let assistant = assistant_part_id
                .as_deref()
                .and_then(|part_id| assistant_streams.controller_mut(part_id));
            run_commit_tick(
                stream_chunking_policy,
                assistant,
                plan_stream_controller.as_mut(),
                CommitTickScope::AnyMode,
                now,
            )
        };
        let committed = output.assistant_cell.is_some() || !output.drained_plan_cells.is_empty();
        let assistant_layout = output.assistant_cell.as_ref().and_then(|_| {
            assistant_part_id.as_deref().and_then(|part_id| {
                self.bridge
                    .assistant_streams
                    .active_layout(part_id)
                    .map(|(width, lines)| (part_id.to_owned(), width, lines))
            })
        });
        if let Some(line) = output.assistant_cell {
            self.upsert_active_assistant_cell(line);
            if let Some((part_id, width, lines)) = assistant_layout {
                self.prime_active_assistant_layout(&part_id, width, lines);
            }
        }
        self.cells.transcript.extend(output.drained_plan_cells);
        if output.should_stop_commit_animation && !self.bridge.assistant_streams.has_live_output() {
            self.bridge.stream_chunking_policy.reset();
        }
        if committed {
            self.record_streaming_delta_latency(now.elapsed());
        }
        committed
    }

    fn advance_bottom_dock_lifecycle(
        &mut self,
        now: Instant,
        duration: std::time::Duration,
    ) -> bool {
        self.composer.bottom_views.advance_lifecycle(now, duration)
    }

    fn update_active_status_indicator(&mut self) {
        let animation_tick = self.bridge.spinner_frame as u64;
        let indicator = self.spinner().to_owned();
        self.update_live_lifecycle_indicator(indicator.clone(), animation_tick);
        self.update_live_hook_indicator(indicator, animation_tick);
    }

    fn update_active_exec_durations(&mut self, now: Instant) {
        let mut inactive_indices = Vec::new();
        for active_tool in self.cells.active_runtime_cells.active_tool_cells() {
            let Some(started_at) = active_tool.started_at else {
                continue;
            };
            let Some(line) = self.cells.transcript.get_mut(active_tool.index) else {
                inactive_indices.push(active_tool.index);
                continue;
            };
            let Some(RenderCellData::Exec(cell)) = line.data.as_mut() else {
                inactive_indices.push(active_tool.index);
                continue;
            };
            if !matches!(cell.status, AgentRenderExecStatus::Running) {
                inactive_indices.push(active_tool.index);
                continue;
            }
            let elapsed = now.saturating_duration_since(started_at);
            cell.set_duration_ms(Some(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)));
            line.bump_render_revision();
        }
        for index in inactive_indices {
            self.cells
                .active_runtime_cells
                .clear_tool_cell_at_index(index);
        }
    }
}
