//! RenderCellRecord 的 RenderCell 适配实现。

use std::{any::Any, num::NonZeroU64};

use crate::surface::{
    cell::{
        CellSummary, RENDER_CELL_KIND_LINE, RENDER_CELL_KIND_STARTUP, RenderCell, RenderCellMode,
        RenderCellRecord, RenderCellTranscriptState, STREAM_TRANSCRIPT_REVISION_FACTOR,
    },
    cell_id::CellId,
    cell_payload::{CellPayloadEnvelope, CellPayloadSchemaError},
};

impl RenderCellTranscriptState {
    pub(crate) const fn stream_continuation() -> Self {
        Self {
            is_stream_continuation: true,
            animation_tick: None,
        }
    }
}

impl RenderCell for RenderCellRecord {
    fn id(&self) -> CellId {
        CellId::new(NonZeroU64::new(self.render_id).unwrap_or(NonZeroU64::MIN))
    }

    fn revision(&self) -> u64 {
        self.render_revision
    }

    fn kind(&self) -> &'static str {
        match self.mode {
            RenderCellMode::Line => RENDER_CELL_KIND_LINE,
            RenderCellMode::Startup => RENDER_CELL_KIND_STARTUP,
            RenderCellMode::Cell(kind) => kind.as_str(),
        }
    }

    fn payload_envelope(&self) -> Result<Option<CellPayloadEnvelope>, CellPayloadSchemaError> {
        self.cell_payload_envelope()
    }

    fn summary(&self) -> CellSummary {
        CellSummary {
            role: self.role,
            text: self.text.clone(),
            mode: self.mode,
            action: self.action,
        }
    }

    fn is_stream_continuation(&self) -> bool {
        self.transcript_state.is_stream_continuation
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        self.transcript_state.animation_tick
    }

    fn provider_call_id(&self) -> Option<&str> {
        self.provider_call_id()
    }

    fn transcript_key(&self) -> u64 {
        if !self.is_stream_continuation() && self.transcript_animation_tick().is_none() {
            return 0;
        }
        self.revision()
            .wrapping_mul(STREAM_TRANSCRIPT_REVISION_FACTOR)
            .wrapping_add(self.transcript_animation_tick().unwrap_or_default())
    }

    fn clone_box(&self) -> Box<dyn RenderCell> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
