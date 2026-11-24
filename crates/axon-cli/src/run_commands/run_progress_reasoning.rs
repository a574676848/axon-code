use std::collections::BTreeMap;

use axon_protocol::wire::{
    WireEventKind, WireReasoningDelta, WireReasoningEnd, WireReasoningStart,
};

use super::RunProgressSink;

impl RunProgressSink<'_> {
    pub(crate) fn reasoning_delta(
        &self,
        part_id: impl Into<String>,
        reasoning_id: impl Into<String>,
        delta: impl Into<String>,
    ) {
        let part_id = part_id.into();
        let reasoning_id = reasoning_id.into();
        let delta = delta.into();
        if delta.trim().is_empty() {
            return;
        }
        if self.ended_reasoning_parts.borrow().contains(&part_id) {
            return;
        }
        self.start_reasoning_part(&part_id, &reasoning_id);
        self.emitted_reasoning_text
            .borrow_mut()
            .entry(part_id.clone())
            .or_default()
            .push_str(&delta);
        self.emit_kind(
            part_id.clone(),
            WireEventKind::ReasoningDelta(WireReasoningDelta {
                part_id: part_id.clone(),
                reasoning_id,
                delta,
            }),
        );
    }

    fn start_reasoning_part(&self, part_id: &str, reasoning_id: &str) {
        if self.emitted_reasoning_text.borrow().contains_key(part_id) {
            return;
        }
        self.emitted_reasoning_text
            .borrow_mut()
            .insert(part_id.to_owned(), String::new());
        self.emit_kind(
            part_id.to_owned(),
            WireEventKind::ReasoningStart(WireReasoningStart {
                part_id: part_id.to_owned(),
                reasoning_id: reasoning_id.to_owned(),
            }),
        );
    }

    pub(crate) fn end_reasoning(
        &self,
        part_id: impl Into<String>,
        reasoning_id: impl Into<String>,
    ) {
        let part_id = part_id.into();
        let reasoning_id = reasoning_id.into();
        if !self.emitted_reasoning_text.borrow().contains_key(&part_id) {
            return;
        }
        if !self
            .ended_reasoning_parts
            .borrow_mut()
            .insert(part_id.clone())
        {
            return;
        }
        self.emit_kind(
            part_id.clone(),
            WireEventKind::ReasoningEnd(WireReasoningEnd {
                part_id: part_id.clone(),
                reasoning_id,
            }),
        );
    }

    pub(crate) fn finish_reasoning(
        &self,
        part_id: impl Into<String>,
        reasoning_id: impl Into<String>,
        full_content: impl Into<String>,
    ) {
        let part_id = part_id.into();
        let reasoning_id = reasoning_id.into();
        let full_content = full_content.into();
        if self.ended_reasoning_parts.borrow().contains(&part_id) {
            return;
        }
        self.start_reasoning_part(&part_id, &reasoning_id);
        let delta = {
            let emitted = self.emitted_reasoning_text.borrow();
            finish_reasoning_delta(&part_id, &full_content, &emitted)
        };
        if !delta.trim().is_empty() {
            self.reasoning_delta(part_id.clone(), reasoning_id.clone(), delta);
        }
        self.emit_kind(
            part_id.clone(),
            WireEventKind::ReasoningEnd(WireReasoningEnd {
                part_id: part_id.clone(),
                reasoning_id,
            }),
        );
        self.ended_reasoning_parts.borrow_mut().insert(part_id);
    }
}

fn finish_reasoning_delta(
    part_id: &str,
    full_content: &str,
    emitted: &BTreeMap<String, String>,
) -> String {
    let Some(streamed) = emitted.get(part_id).filter(|value| !value.is_empty()) else {
        return full_content.to_owned();
    };
    full_content
        .strip_prefix(streamed)
        .unwrap_or_default()
        .to_owned()
}
