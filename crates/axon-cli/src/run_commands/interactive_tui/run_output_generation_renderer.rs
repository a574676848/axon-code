//! Provider generation 回放状态。

use std::{collections::HashSet, mem};

use axon_protocol::wire::{
    WireEventEnvelope, WireEventKind, WireLifecycleStatus, is_provider_retry_lifecycle,
};

const WIRE_LIFECYCLE_KIND_STREAMING_STARTED: &str = "StreamingStarted";

/// 仅跟踪当前尚未提交的 Provider generation part。
#[derive(Debug, Default)]
pub(super) struct RetryGenerationProjection {
    active_part_ids: HashSet<String>,
}

impl RetryGenerationProjection {
    /// 观察本 generation 新产生的 part。
    pub(super) fn observe_part(&mut self, part_id: &str) {
        self.active_part_ids.insert(part_id.to_owned());
    }

    /// 标记 part 已由结束事件提交，后续 retry 不再撤销它。
    pub(super) fn commit_part(&mut self, part_id: &str) {
        self.active_part_ids.remove(part_id);
    }

    /// 返回 retry 时需要撤销的当前 generation part。
    pub(super) fn failed_parts_for_event(
        &mut self,
        event: &WireEventEnvelope,
    ) -> Option<HashSet<String>> {
        let WireEventKind::TurnLifecycle(lifecycle) = &event.kind else {
            return None;
        };
        if is_provider_retry_lifecycle(lifecycle) {
            return Some(mem::take(&mut self.active_part_ids));
        }
        if lifecycle.status == WireLifecycleStatus::Started
            && lifecycle.kind.as_deref() == Some(WIRE_LIFECYCLE_KIND_STREAMING_STARTED)
            && lifecycle.attempt_index.is_none_or(|attempt| attempt <= 1)
        {
            self.active_part_ids.clear();
        }
        None
    }
}
