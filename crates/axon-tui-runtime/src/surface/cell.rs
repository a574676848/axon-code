//! RenderCell 结构化事实源。

use std::{
    any::Any,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::surface::{
    cell_id::CellId,
    cell_payload::{CellPayloadEnvelope, CellPayloadSchemaError},
    projection,
};

mod identity;
mod record_render;

pub(crate) use super::cell_status_data::{
    RenderCommandCell, RenderGenerationStatusCell, RenderLifecycleItem, RenderStatusCell,
    RenderStatusItem,
};

#[cfg(test)]
#[path = "cell_identity_tests.rs"]
mod cell_identity_tests;

pub(crate) use identity::{
    is_primary_runtime_cell_kind, render_cell_record, render_cell_record_mut,
    render_cell_registry_entry,
};

pub(crate) const ROLE_AGENT: &str = "agent";
pub(crate) const ROLE_ERROR: &str = "error";
pub(crate) const ROLE_NOTICE: &str = "notice";
pub(crate) const ROLE_SYSTEM: &str = "system";
pub(crate) const ROLE_TOOL: &str = "tool";
pub(crate) const ROLE_USER: &str = "user";
pub(crate) const ROLE_VERIFY: &str = "verify";
pub(crate) const STATUS_KIND_INTERRUPT_REQUESTED: &str = "InterruptRequested";

static NEXT_RENDER_CELL_ID: AtomicU64 = AtomicU64::new(1);
const STREAM_TRANSCRIPT_REVISION_FACTOR: u64 = 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderCellMode {
    Line,
    Startup,
    Cell(RenderCellKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RenderCellKind(&'static str);

pub(crate) const RENDER_CELL_KIND_LINE: &str = "line";
pub(crate) const RENDER_CELL_KIND_STARTUP: &str = "startup";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderCellRegistryEntry {
    pub(crate) kind: RenderCellKind,
    pub(crate) runtime_selectable: bool,
}

const RUNTIME_SELECTABLE: bool = true;

pub(crate) const RENDER_CELL_REGISTRY: &[RenderCellRegistryEntry] = &[
    RenderCellRegistryEntry::new(RenderCellKind::COMMAND, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::DIFF, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::MESSAGE, false),
    RenderCellRegistryEntry::new(RenderCellKind::EXEC, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::HISTORY, false),
    RenderCellRegistryEntry::new(RenderCellKind::HOOK, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::MCP, false),
    RenderCellRegistryEntry::new(RenderCellKind::PLAN, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::REVIEW, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::STATUS, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::GENERATION_STATUS, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::REASONING, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::TURN_SUMMARY, false),
    RenderCellRegistryEntry::new(RenderCellKind::USAGE, false),
    RenderCellRegistryEntry::new(RenderCellKind::ERROR, RUNTIME_SELECTABLE),
    RenderCellRegistryEntry::new(RenderCellKind::QUESTION, RUNTIME_SELECTABLE),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RenderCellData {
    Diff(crate::surface::cell_diff_data::AgentRenderDiffCell),
    Command(RenderCommandCell),
    Exec(crate::surface::cell_event_data::AgentRenderExecCell),
    History(crate::surface::cell_event_data::AgentRenderHistoryCell),
    Hook(crate::surface::cell_event_data::AgentRenderHookCell),
    Mcp(crate::surface::cell_event_data::AgentRenderMcpCell),
    Message(RenderMessageCell),
    Plan(crate::surface::cell_plan_data::AgentRenderPlanCell),
    Review(crate::surface::cell_review_data::AgentRenderReviewCell),
    Error(crate::surface::cell_event_data::AgentRenderErrorCell),
    Status(RenderStatusCell),
    GenerationStatus(RenderGenerationStatusCell),
    Reasoning(RenderReasoningCell),
    TurnSummary(crate::surface::cell_turn_summary_data::AgentRenderTurnSummaryCell),
    Usage(crate::surface::cell_event_data::AgentRenderUsageCell),
    Question(crate::surface::cell_question_data::AgentRenderQuestionCell),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderMessageCell {
    pub(crate) id: String,
    pub(crate) source_kind: RenderMessageSourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderMessageSourceKind {
    Markdown,
    Stream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderReasoningCell {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) done: bool,
    pub(crate) elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderCellAction {
    Answer,
    Called,
    Command,
    Error,
    History,
    Hook,
    List,
    Mcp,
    Patched,
    Plan,
    Read,
    Review,
    Run,
    Ran,
    Running,
    Search,
    Searched,
    Status,
    GenerationStatus,
    Reasoning,
    TurnSummary,
    Waiting,
    Usage,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CellSummary {
    pub(crate) role: &'static str,
    pub(crate) text: String,
    pub(crate) mode: RenderCellMode,
    pub(crate) action: RenderCellAction,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RenderCellTranscriptState {
    pub(crate) is_stream_continuation: bool,
    pub(crate) animation_tick: Option<u64>,
}

pub(crate) trait RenderCell: std::fmt::Debug + Send + Sync + Any {
    fn id(&self) -> CellId;
    fn revision(&self) -> u64;
    fn kind(&self) -> &'static str;
    fn payload_envelope(&self) -> Result<Option<CellPayloadEnvelope>, CellPayloadSchemaError>;
    fn summary(&self) -> CellSummary;
    fn is_stream_continuation(&self) -> bool {
        false
    }
    fn transcript_animation_tick(&self) -> Option<u64> {
        None
    }
    fn provider_call_id(&self) -> Option<&str> {
        None
    }
    fn transcript_key(&self) -> u64;
    fn clone_box(&self) -> Box<dyn RenderCell>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl Clone for Box<dyn RenderCell> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderCellRecord {
    render_id: u64,
    render_revision: u64,
    transcript_state: RenderCellTranscriptState,
    pub(crate) role: &'static str,
    pub(crate) text: String,
    pub(crate) mode: RenderCellMode,
    pub(crate) action: RenderCellAction,
    pub(crate) data: Option<RenderCellData>,
}

impl RenderCellRecord {
    pub(crate) fn agent_message(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::agent_with_source_kind(id, text, RenderMessageSourceKind::Markdown)
    }

    pub(crate) fn streaming_agent_message(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::agent_with_source_kind(id, text, RenderMessageSourceKind::Stream)
            .with_transcript_state(RenderCellTranscriptState::stream_continuation())
    }

    fn agent_with_source_kind(
        id: impl Into<String>,
        text: impl Into<String>,
        source_kind: RenderMessageSourceKind,
    ) -> Self {
        let cell = RenderMessageCell::new(id, source_kind);
        Self::cell_with_state_and_data(
            ROLE_AGENT,
            text,
            RenderCellKind::MESSAGE,
            RenderCellAction::Answer,
            Some(RenderCellData::Message(cell)),
        )
    }

    pub(crate) fn error_with_data(
        cell: crate::surface::cell_event_data::AgentRenderErrorCell,
    ) -> Self {
        let plain_text = cell.plain_text_fallback();
        Self::cell_with_state_and_data(
            ROLE_ERROR,
            plain_text,
            RenderCellKind::ERROR,
            RenderCellAction::Error,
            Some(RenderCellData::Error(cell)),
        )
    }

    pub(crate) fn system(text: impl Into<String>) -> Self {
        Self::new(ROLE_SYSTEM, text, RenderCellAction::Called)
    }

    pub(crate) fn startup(text: impl Into<String>) -> Self {
        Self {
            render_id: next_render_cell_id(),
            render_revision: 0,
            transcript_state: RenderCellTranscriptState::default(),
            role: ROLE_SYSTEM,
            text: text.into(),
            mode: RenderCellMode::Startup,
            action: RenderCellAction::Called,
            data: None,
        }
    }

    pub(crate) fn exec_with_data(
        cell: crate::surface::cell_event_data::AgentRenderExecCell,
        action: RenderCellAction,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_TOOL,
            cell.display_label.clone(),
            RenderCellKind::EXEC,
            action,
            Some(RenderCellData::Exec(cell)),
        )
    }

    pub(crate) fn file_change_with_data(
        cell: crate::surface::cell_diff_data::AgentRenderDiffCell,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_TOOL,
            cell.title.clone(),
            RenderCellKind::DIFF,
            RenderCellAction::Patched,
            Some(RenderCellData::Diff(cell)),
        )
    }

    pub(crate) fn plan_with_data(
        cell: crate::surface::cell_plan_data::AgentRenderPlanCell,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_VERIFY,
            cell.title.clone(),
            RenderCellKind::PLAN,
            RenderCellAction::Plan,
            Some(RenderCellData::Plan(cell)),
        )
    }

    pub(crate) fn review_with_data(
        cell: crate::surface::cell_review_data::AgentRenderReviewCell,
    ) -> Self {
        Self::review_with_data_and_action(cell, RenderCellAction::Review)
    }

    pub(crate) fn review_with_data_and_action(
        cell: crate::surface::cell_review_data::AgentRenderReviewCell,
        action: RenderCellAction,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_VERIFY,
            cell.subject.clone(),
            RenderCellKind::REVIEW,
            action,
            Some(RenderCellData::Review(cell)),
        )
    }

    pub(crate) fn history_with_data(
        cell: crate::surface::cell_event_data::AgentRenderHistoryCell,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.title.clone(),
            RenderCellKind::HISTORY,
            RenderCellAction::History,
            Some(RenderCellData::History(cell)),
        )
    }

    pub(crate) fn hook_with_data(
        cell: crate::surface::cell_event_data::AgentRenderHookCell,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.title.clone(),
            RenderCellKind::HOOK,
            RenderCellAction::Hook,
            Some(RenderCellData::Hook(cell)),
        )
    }

    pub(crate) fn question_with_data(
        cell: crate::surface::cell_question_data::AgentRenderQuestionCell,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.id.clone(),
            RenderCellKind::QUESTION,
            RenderCellAction::Answer,
            Some(RenderCellData::Question(cell)),
        )
    }

    pub(crate) fn mcp_with_data(cell: crate::surface::cell_event_data::AgentRenderMcpCell) -> Self {
        Self::cell_with_state_and_data(
            ROLE_TOOL,
            cell.title.clone(),
            RenderCellKind::MCP,
            RenderCellAction::Mcp,
            Some(RenderCellData::Mcp(cell)),
        )
    }

    pub(crate) fn status_with_data(cell: RenderStatusCell) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.title.clone(),
            RenderCellKind::STATUS,
            RenderCellAction::Status,
            Some(RenderCellData::Status(cell)),
        )
    }

    pub(crate) fn command_with_data(cell: RenderCommandCell) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.title.clone(),
            RenderCellKind::COMMAND,
            RenderCellAction::Command,
            Some(RenderCellData::Command(cell)),
        )
    }

    pub(crate) fn generation_status_with_data(cell: RenderGenerationStatusCell) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.title.clone(),
            RenderCellKind::GENERATION_STATUS,
            RenderCellAction::GenerationStatus,
            Some(RenderCellData::GenerationStatus(cell)),
        )
    }

    pub(crate) fn reasoning_with_data(cell: RenderReasoningCell) -> Self {
        Self::cell_with_state_and_data(
            ROLE_AGENT,
            cell.text.clone(),
            RenderCellKind::REASONING,
            RenderCellAction::Reasoning,
            Some(RenderCellData::Reasoning(cell)),
        )
    }

    pub(crate) fn turn_summary_with_data(
        cell: crate::surface::cell_turn_summary_data::AgentRenderTurnSummaryCell,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.id.clone(),
            RenderCellKind::TURN_SUMMARY,
            RenderCellAction::TurnSummary,
            Some(RenderCellData::TurnSummary(cell)),
        )
    }

    pub(crate) fn usage_with_data(
        cell: crate::surface::cell_event_data::AgentRenderUsageCell,
    ) -> Self {
        Self::cell_with_state_and_data(
            ROLE_NOTICE,
            cell.title.clone(),
            RenderCellKind::USAGE,
            RenderCellAction::Usage,
            Some(RenderCellData::Usage(cell)),
        )
    }

    pub(crate) fn user(text: impl Into<String>) -> Self {
        Self::new(ROLE_USER, text, RenderCellAction::User)
    }

    #[cfg(test)]
    pub(crate) fn user_with_data(text: impl Into<String>) -> Self {
        Self::user_message("user", text)
    }

    pub(crate) fn user_message(id: impl Into<String>, text: impl Into<String>) -> Self {
        let cell = RenderMessageCell::new(id, RenderMessageSourceKind::Markdown);
        Self::cell_with_state_and_data(
            ROLE_USER,
            text,
            RenderCellKind::MESSAGE,
            RenderCellAction::User,
            Some(RenderCellData::Message(cell)),
        )
    }

    pub(crate) fn provider_call_id(&self) -> Option<&str> {
        identity::provider_call_id(self.data.as_ref())
    }

    pub(crate) fn part_id(&self) -> String {
        identity::part_id(self)
    }

    pub(crate) fn semantic_part_id(&self) -> Option<String> {
        identity::semantic_part_id(self)
    }

    pub(crate) fn cell_payload_envelope(
        &self,
    ) -> Result<Option<CellPayloadEnvelope>, CellPayloadSchemaError> {
        let Some(data) = &self.data else {
            return Ok(None);
        };
        match data {
            RenderCellData::Message(cell) => {
                projection::message_cell_payload_envelope(cell, &self.text).map(Some)
            }
            _ => projection::cell_payload_envelope(data).map(Some),
        }
    }

    pub(crate) const fn render_id(&self) -> u64 {
        self.render_id
    }

    pub(crate) const fn render_revision(&self) -> u64 {
        self.render_revision
    }

    pub(crate) fn replace_with(&mut self, mut next: Self) {
        next.render_id = self.render_id;
        next.render_revision = self.render_revision.wrapping_add(1);
        *self = next;
    }

    pub(crate) fn bump_render_revision(&mut self) {
        self.render_revision = self.render_revision.wrapping_add(1);
    }

    pub(crate) fn set_transcript_animation_tick(&mut self, tick: Option<u64>) {
        self.transcript_state.animation_tick = tick;
    }

    fn with_transcript_state(mut self, transcript_state: RenderCellTranscriptState) -> Self {
        self.transcript_state = transcript_state;
        self
    }

    fn new(role: &'static str, text: impl Into<String>, action: RenderCellAction) -> Self {
        Self {
            render_id: next_render_cell_id(),
            render_revision: 0,
            transcript_state: RenderCellTranscriptState::default(),
            role,
            text: text.into(),
            mode: RenderCellMode::Line,
            action,
            data: None,
        }
    }

    fn cell_with_state_and_data(
        role: &'static str,
        text: impl Into<String>,
        kind: RenderCellKind,
        action: RenderCellAction,
        data: Option<RenderCellData>,
    ) -> Self {
        assert!(
            kind != RenderCellKind::ERROR || matches!(data, Some(RenderCellData::Error(_))),
            "RenderCellKind::ERROR 必须携带 RenderCellData::Error"
        );
        Self {
            render_id: next_render_cell_id(),
            render_revision: 0,
            transcript_state: RenderCellTranscriptState::default(),
            role,
            text: text.into(),
            mode: RenderCellMode::Cell(kind),
            action,
            data,
        }
    }
}

fn next_render_cell_id() -> u64 {
    NEXT_RENDER_CELL_ID.fetch_add(1, Ordering::Relaxed)
}

impl RenderCellKind {
    pub(crate) const COMMAND: Self = Self("command");
    pub(crate) const DIFF: Self = Self("diff");
    pub(crate) const MESSAGE: Self = Self("message");
    pub(crate) const EXEC: Self = Self("exec");
    pub(crate) const HISTORY: Self = Self("history");
    pub(crate) const HOOK: Self = Self("hook");
    pub(crate) const MCP: Self = Self("mcp");
    pub(crate) const PLAN: Self = Self("plan");
    pub(crate) const REVIEW: Self = Self("review");
    pub(crate) const STATUS: Self = Self("status");
    pub(crate) const GENERATION_STATUS: Self = Self("generation_status");
    pub(crate) const REASONING: Self = Self("reasoning");
    pub(crate) const TURN_SUMMARY: Self = Self("turn_summary");
    pub(crate) const USAGE: Self = Self("usage");
    pub(crate) const ERROR: Self = Self("error");
    pub(crate) const QUESTION: Self = Self("question");

    pub(crate) const fn as_str(self) -> &'static str {
        self.0
    }
}

impl RenderCellRegistryEntry {
    const fn new(kind: RenderCellKind, runtime_selectable: bool) -> Self {
        Self {
            kind,
            runtime_selectable,
        }
    }
}

impl RenderMessageCell {
    fn new(id: impl Into<String>, source_kind: RenderMessageSourceKind) -> Self {
        Self {
            id: id.into(),
            source_kind,
        }
    }
}
