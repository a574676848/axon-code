use crate::surface::event_envelope::RuntimeTransportTiming;

#[derive(Debug, Default)]
pub(super) struct PendingAssistantDeltaBatch {
    pub(super) part_id: Option<String>,
    pub(super) deltas: Vec<String>,
    visible_part_id: Option<String>,
    first_occurred_at_ms: Option<u128>,
    transport_timing: Option<RuntimeTransportTiming>,
}

#[derive(Debug)]
pub(super) struct AssistantDeltaFrame {
    pub(super) part_id: String,
    pub(super) content: String,
    pub(super) occurred_at_ms: u128,
    pub(super) transport_timing: Option<RuntimeTransportTiming>,
}

impl PendingAssistantDeltaBatch {
    pub(super) fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    pub(super) fn push(&mut self, frame: AssistantDeltaFrame) {
        self.part_id = Some(frame.part_id);
        self.first_occurred_at_ms
            .get_or_insert(frame.occurred_at_ms);
        if self.transport_timing.is_none() {
            self.transport_timing = frame.transport_timing;
        }
        self.deltas.push(frame.content);
    }

    pub(super) fn has_different_part_id(&self, part_id: &str) -> bool {
        !self.is_empty()
            && self
                .part_id
                .as_deref()
                .is_some_and(|active| active != part_id)
    }

    pub(super) fn take(
        &mut self,
    ) -> Option<(String, Vec<String>, u128, Option<RuntimeTransportTiming>)> {
        if self.deltas.is_empty() {
            return None;
        }
        let part_id = self.part_id.take()?;
        let occurred_at_ms = self.first_occurred_at_ms.take()?;
        self.visible_part_id = Some(part_id.clone());
        Some((
            part_id,
            std::mem::take(&mut self.deltas),
            occurred_at_ms,
            self.transport_timing.take(),
        ))
    }

    pub(super) fn first_delta_pending(&self) -> bool {
        !self.is_empty() && self.visible_part_id.as_deref() != self.part_id.as_deref()
    }
}
