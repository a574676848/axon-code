//! Agent TUI debug state dump 投影。

use std::{fs, io, path::PathBuf};

#[cfg(test)]
use crate::surface::state_dump::TuiStateDumpLoadError;
use crate::surface::state_dump::{
    TuiStateDump, TuiStateDumpBridge, TuiStateDumpCell, TuiStateDumpCells, TuiStateDumpComposer,
    TuiStateDumpHeader, TuiStateDumpPlanProgress, TuiStateDumpRuntimePartAnchor,
    TuiStateDumpRuntimePartState, TuiStateDumpViewport, redact_optional_prompt, redact_path,
    redact_prompt, state_dump_path,
};
use crate::surface::{
    cell::{RenderCellData, RenderCellMode, RenderCellRecord},
    projection::cell_payload_envelope,
};

use super::AgentTuiState;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn state_dump_json(
        &self,
    ) -> Result<String, serde_json::Error> {
        self.state_dump().to_json()
    }

    #[cfg(test)]
    pub(in crate::r#loop::agent_tui) fn load_state_dump_json(
        json: &str,
    ) -> Result<TuiStateDump, TuiStateDumpLoadError> {
        TuiStateDump::load_json(json)
    }

    pub(in crate::r#loop::agent_tui) fn write_state_dump_for_debug(
        &self,
        transaction: &str,
    ) -> io::Result<PathBuf> {
        let path = state_dump_path(transaction);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = self
            .state_dump_json()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        TuiStateDump::load_json(&json)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        fs::write(&path, json)?;
        Ok(path)
    }

    fn state_dump(&self) -> TuiStateDump {
        TuiStateDump::new(
            self.header_dump(),
            self.viewport_dump(),
            self.composer_dump(),
            self.cells_dump(),
            self.bridge_dump(),
            self.bridge.telemetry.snapshot(),
        )
    }

    fn header_dump(&self) -> TuiStateDumpHeader {
        TuiStateDumpHeader {
            cwd: redact_path(&self.header.cwd),
            workspace_instruction_paths: self
                .header
                .workspace_instruction_paths
                .iter()
                .map(redact_path)
                .collect(),
            config_provider: self.header.config_status.provider.clone(),
            config_model: self.header.config_status.model.clone(),
            config_auth_status: self.header.config_status.api_key_status.clone(),
            hint: self.header.hint.to_owned(),
            plan_progress: self
                .plan_progress()
                .map(|progress| TuiStateDumpPlanProgress {
                    completed: progress.completed,
                    total: progress.total,
                }),
            session_usage_total: self.header.session_usage.total_tokens(),
            last_turn_usage_total: self.header.last_turn_usage.total_tokens(),
            live_turn_usage_total: self.header.live_turn_usage.total_tokens(),
            lsp_active_count: self.header.lsp_active_count,
            gateway_active_turn_count: self.header.gateway_active_turn_count,
        }
    }

    fn viewport_dump(&self) -> TuiStateDumpViewport {
        let jump_affordance = self.viewport.transcript_view_jump_affordance();
        TuiStateDumpViewport {
            active_dock_kind: self.active_dock_kind().map(|kind| format!("{kind:?}")),
            transcript_view_offset_rows: self.viewport.transcript_view_offset_rows,
            max_transcript_view_offset_rows: self.viewport.max_transcript_view_offset_rows,
            transcript_view_page_rows: self.viewport.transcript_view_page_rows,
            transcript_view_scroll_state: self
                .viewport
                .transcript_view_scroll_state
                .as_str()
                .to_owned(),
            transcript_view_jump_available: jump_affordance.available,
            transcript_view_jump_distance_rows: jump_affordance.distance_rows,
            transcript_view_jump_threshold_rows: jump_affordance.threshold_rows,
            transcript_view_top_anchor_cell_id: self
                .viewport
                .transcript_view_top_anchor
                .map(|anchor| anchor.cell_id),
            transcript_view_top_anchor_row_offset: self
                .viewport
                .transcript_view_top_anchor
                .map(|anchor| anchor.row_offset_in_cell),
            selected_runtime_render_index: self.selected_runtime_render_index_by_identity(),
            selected_runtime_part_id: self.viewport.selected_runtime_part_id().map(str::to_owned),
            tool_output_dock_open: self
                .composer
                .bottom_views
                .top_view()
                .and_then(|view| view.tool_output())
                .and_then(super::super::output_dock::ToolOutputDock::copy_text)
                .is_some(),
        }
    }

    fn composer_dump(&self) -> TuiStateDumpComposer {
        TuiStateDumpComposer {
            draft: redact_prompt(self.composer.draft.text()),
            draft_chars: self.composer.draft.text().chars().count(),
            submitted_prompt: redact_optional_prompt(self.composer.submitted_prompt.as_deref()),
            command_suggestions_count: self.composer.command_suggestions.len(),
            selected_command_suggestion: self.composer.selected_command_suggestion,
            composer_popup_open: self
                .composer
                .composer_popup
                .as_ref()
                .is_some_and(|popup| !popup.is_empty()),
            history_count: self.composer.history.len(),
            history_path: redact_path(&self.composer.history_path),
            history_index: self.composer.history_index,
            queued_user_messages: self.composer.input_queue.queued_user_messages.len(),
            pending_steers: self.composer.input_queue.pending_steers.len(),
            user_turn_pending_start: self.composer.input_queue.user_turn_pending_start,
        }
    }

    fn cells_dump(&self) -> TuiStateDumpCells {
        TuiStateDumpCells {
            transcript: self.cells.transcript.iter().map(cell_dump).collect(),
            active_runtime_parts: self
                .cells
                .active_runtime_cells
                .active_runtime_parts()
                .into_iter()
                .map(|part| TuiStateDumpRuntimePartAnchor {
                    part_id: part.part_id,
                    cell_index: part.index,
                })
                .collect(),
            tool_output_count: self.cells.tool_outputs_by_part_id.len(),
        }
    }

    fn bridge_dump(&self) -> TuiStateDumpBridge {
        TuiStateDumpBridge {
            runtime_status: self.bridge.runtime_status.as_str().to_owned(),
            runtime_tmp_dir: redact_path(&self.bridge.runtime_tmp_dir),
            assistant_stream_count: self.bridge.assistant_streams.stream_count(),
            active_assistant_anchors: self
                .bridge
                .assistant_streams
                .anchors()
                .map(|(_, anchor)| TuiStateDumpRuntimePartAnchor {
                    part_id: anchor.part_id().to_owned(),
                    cell_index: anchor.cell_index(),
                })
                .collect(),
            active_reasoning_parts: self
                .bridge
                .active_reasoning_parts
                .iter()
                .map(|(part_id, state)| TuiStateDumpRuntimePartState {
                    part_id: part_id.clone(),
                    anchor: state.anchor().map(|anchor| TuiStateDumpRuntimePartAnchor {
                        part_id: anchor.part_id().to_owned(),
                        cell_index: anchor.cell_index(),
                    }),
                })
                .collect(),
            has_live_plan: self.bridge.live_plan_line.is_some(),
            has_live_review: self.bridge.live_review_line.is_some(),
            has_live_lifecycle: self.bridge.live_lifecycle_line.is_some(),
            has_live_hook: self.bridge.live_hook_line.is_some(),
            interrupt_requested: self.bridge.interrupt_requested,
            should_exit: self.bridge.should_exit,
            runtime_submission_enabled: self.bridge.runtime_submission_enabled,
        }
    }
}

fn cell_dump(line: &RenderCellRecord) -> TuiStateDumpCell {
    TuiStateDumpCell {
        render_id: line.render_id(),
        part_id: line.part_id(),
        semantic_part_id: line.semantic_part_id(),
        revision: line.render_revision(),
        role: line.role.to_owned(),
        mode: render_mode_label(line.mode),
        action: format!("{:?}", line.action),
        text: redacted_cell_text(line),
        payload: line.data.as_ref().and_then(redacted_payload),
    }
}

fn render_mode_label(mode: RenderCellMode) -> String {
    match mode {
        RenderCellMode::Line => "line".to_owned(),
        RenderCellMode::Startup => "startup".to_owned(),
        RenderCellMode::Cell(kind) => format!("{kind:?}"),
    }
}

fn redacted_cell_text(line: &RenderCellRecord) -> String {
    match &line.data {
        Some(RenderCellData::Message(_)) => redact_prompt(&line.text),
        _ if line.role == crate::surface::cell::ROLE_USER => redact_prompt(&line.text),
        _ => line.text.clone(),
    }
}

fn redacted_payload(
    data: &RenderCellData,
) -> Option<crate::surface::cell_payload::CellPayloadEnvelope> {
    if matches!(data, RenderCellData::Message(_)) {
        return None;
    }
    cell_payload_envelope(data)
        .and_then(crate::surface::cell_payload::CellPayloadEnvelope::validate)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        r#loop::agent_tui::{
            state::runtime_flow_test_support::{test_state, test_submission},
            wire_event_test_support::{reasoning_delta_event, reasoning_start_event},
        },
        surface::cell::RenderCellRecord,
    };

    #[test]
    fn state_dump_roundtrip_redacts_prompt_and_sensitive_paths() {
        let mut state = test_state();
        state.header.cwd = r"C:\Users\dev\.codex\secret-project".to_owned();
        state
            .composer
            .draft
            .replace_all("sk-test prompt".to_owned());
        state.composer.submitted_prompt = Some("submitted secret".to_owned());
        state
            .cells
            .transcript
            .push(RenderCellRecord::user("user secret prompt"));
        state.cells.transcript.push(RenderCellRecord::agent_message(
            "assistant-part:state-dump:redaction",
            "assistant answer with private context",
        ));

        let json = state
            .state_dump_json()
            .expect("state dump should serialize");
        let dump = AgentTuiState::load_state_dump_json(&json).expect("state dump should load");

        assert!(dump.redaction.enabled);
        assert_eq!(dump.composer.draft, "[REDACTED]");
        assert_eq!(
            dump.composer.submitted_prompt.as_deref(),
            Some("[REDACTED]")
        );
        assert!(!json.contains("sk-test prompt"));
        assert!(!json.contains("submitted secret"));
        assert!(!json.contains("assistant answer with private context"));
        assert!(json.contains("[REDACTED_SEGMENT]"));
    }

    #[test]
    fn state_dump_preserves_structured_tool_payload_for_replay() {
        let mut state = test_state();
        state.push_status(
            "测试状态",
            vec![crate::surface::cell::RenderStatusItem {
                kind: "phase".to_owned(),
                summary: "可回放结构化 cell".to_owned(),
                phase: None,
                indicator: None,
                tool_name: None,
                duration_ms: None,
                turn_id: None,
                attempt_index: None,
            }],
        );

        let json = state
            .state_dump_json()
            .expect("state dump should serialize");
        let dump = AgentTuiState::load_state_dump_json(&json).expect("state dump should load");

        assert!(dump.cells.transcript.iter().any(|cell| {
            cell.payload
                .as_ref()
                .is_some_and(|payload| payload.kind == "command")
        }));
    }

    #[test]
    fn state_dump_preserves_active_reasoning_payload_and_ref() {
        let mut state = test_state();
        state.start_runtime_submit(&test_submission("解释状态转储"));
        state.apply_runtime_event(reasoning_start_event(
            "reasoning-part:dump",
            "provider-reasoning:dump",
        ));
        state.apply_runtime_event(reasoning_delta_event(
            "reasoning-part:dump",
            "provider-reasoning:dump",
            "状态转储 reasoning",
        ));

        let json = state
            .state_dump_json()
            .expect("state dump should serialize");
        let dump = AgentTuiState::load_state_dump_json(&json).expect("state dump should load");

        assert_eq!(dump.bridge.active_reasoning_parts.len(), 1);
        assert_eq!(
            dump.bridge.active_reasoning_parts[0].part_id,
            "reasoning-part:dump"
        );
        assert_eq!(
            dump.bridge.active_reasoning_parts[0]
                .anchor
                .as_ref()
                .map(|anchor| anchor.part_id.as_str()),
            Some("reasoning-part:dump")
        );

        let payload = dump
            .cells
            .transcript
            .iter()
            .filter_map(|cell| cell.payload.as_ref())
            .find(|payload| payload.kind == "reasoning")
            .expect("reasoning payload should be dumped");
        let reasoning_cell = dump
            .cells
            .transcript
            .iter()
            .find(|cell| cell.part_id == "reasoning-part:dump")
            .expect("reasoning part id should be dumped");

        assert_eq!(payload.payload["id"], "reasoning-part:dump");
        assert_eq!(payload.payload["text"], "状态转储 reasoning");
        assert_eq!(payload.payload["done"], false);
        assert_eq!(
            reasoning_cell.semantic_part_id.as_deref(),
            Some("reasoning-part:dump")
        );
        assert_ne!(reasoning_cell.render_id.to_string(), reasoning_cell.part_id);
    }
}
