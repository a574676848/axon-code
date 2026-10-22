//! 多 Assistant part 的有界流式状态注册表。

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::Arc,
    time::Instant,
};

use axon_agent_runtime::ProjectionRuntimeBudget;

use crate::{
    r#loop::agent_tui::{
        runtime_bridge::RuntimePartAnchor, streaming::controller::AssistantStreamController,
    },
    surface::cell::RenderCellRecord,
    widgets::HyperlinkLine,
};

#[derive(Debug, Clone)]
pub(in crate::r#loop::agent_tui) struct AssistantStreamRegistry {
    controllers: BTreeMap<String, AssistantStreamController>,
    anchors: BTreeMap<String, RuntimePartAnchor>,
    order: VecDeque<String>,
    capacity: usize,
}

impl Default for AssistantStreamRegistry {
    fn default() -> Self {
        Self::with_capacity(ProjectionRuntimeBudget::current().semantic_mailbox_slots)
    }
}

impl AssistantStreamRegistry {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            controllers: BTreeMap::new(),
            anchors: BTreeMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub(in crate::r#loop::agent_tui) fn push_delta(
        &mut self,
        part_id: &str,
        delta: &str,
        width: u16,
        cwd: &Path,
        now: Instant,
    ) -> Option<RenderCellRecord> {
        let evicted = self.ensure_controller(part_id, width, cwd);
        self.controllers
            .get_mut(part_id)
            .expect("刚建立的 Assistant stream 必须存在")
            .push_delta(delta, now);
        evicted
    }

    #[cfg(test)]
    fn finalize_part(&mut self, part_id: &str) -> Option<RenderCellRecord> {
        let mut controller = self.remove_controller(part_id)?;
        controller.finalize().cell
    }

    pub(in crate::r#loop::agent_tui) fn discard_controller(&mut self, part_id: &str) {
        self.remove_controller(part_id);
    }

    pub(in crate::r#loop::agent_tui) fn finalize_all(&mut self) -> Vec<RenderCellRecord> {
        let part_ids = self.order.drain(..).collect::<Vec<_>>();
        part_ids
            .into_iter()
            .filter_map(|part_id| {
                let mut controller = self.controllers.remove(&part_id)?;
                controller.finalize().cell
            })
            .collect()
    }

    pub(in crate::r#loop::agent_tui) fn clear(&mut self) {
        self.controllers.clear();
        self.anchors.clear();
        self.order.clear();
    }

    pub(in crate::r#loop::agent_tui) fn next_commit_part_id(&mut self) -> Option<String> {
        let candidates = self.order.len();
        for _ in 0..candidates {
            let part_id = self.order.pop_front()?;
            let pending = self
                .controllers
                .get(&part_id)
                .is_some_and(AssistantStreamController::has_pending_projection);
            self.order.push_back(part_id.clone());
            if pending {
                return Some(part_id);
            }
        }
        None
    }

    pub(in crate::r#loop::agent_tui) fn controller_mut(
        &mut self,
        part_id: &str,
    ) -> Option<&mut AssistantStreamController> {
        self.controllers.get_mut(part_id)
    }

    pub(in crate::r#loop::agent_tui) fn set_width(&mut self, width: u16, now: Instant) {
        for controller in self.controllers.values_mut() {
            controller.set_width(width, now);
        }
    }

    pub(in crate::r#loop::agent_tui) fn has_live_output(&self) -> bool {
        self.controllers
            .values()
            .any(|controller| controller.has_live_tail() || controller.queued_lines() > 0)
    }

    pub(in crate::r#loop::agent_tui) fn stream_count(&self) -> usize {
        self.controllers.len()
    }

    pub(in crate::r#loop::agent_tui) fn stream_part_ids(&self) -> impl Iterator<Item = &str> {
        self.controllers.keys().map(String::as_str)
    }

    pub(in crate::r#loop::agent_tui) fn anchor(&self, part_id: &str) -> Option<&RuntimePartAnchor> {
        self.anchors.get(part_id)
    }

    pub(in crate::r#loop::agent_tui) fn anchors(
        &self,
    ) -> impl Iterator<Item = (&str, &RuntimePartAnchor)> {
        self.anchors
            .iter()
            .map(|(part_id, anchor)| (part_id.as_str(), anchor))
    }

    pub(in crate::r#loop::agent_tui) fn set_anchor(
        &mut self,
        part_id: String,
        anchor: RuntimePartAnchor,
    ) {
        self.anchors.insert(part_id, anchor);
    }

    pub(in crate::r#loop::agent_tui) fn take_anchor(
        &mut self,
        part_id: &str,
    ) -> Option<RuntimePartAnchor> {
        self.anchors.remove(part_id)
    }

    pub(in crate::r#loop::agent_tui) fn active_layout(
        &mut self,
        part_id: &str,
    ) -> Option<(u16, Arc<[HyperlinkLine]>)> {
        self.controllers.get_mut(part_id).map(|controller| {
            (
                controller.width(),
                controller.current_active_hyperlink_lines(),
            )
        })
    }

    fn ensure_controller(
        &mut self,
        part_id: &str,
        width: u16,
        cwd: &Path,
    ) -> Option<RenderCellRecord> {
        if self.controllers.contains_key(part_id) {
            return None;
        }
        let evicted = if self.controllers.len() >= self.capacity {
            self.evict_oldest_controller()
        } else {
            None
        };
        self.controllers.insert(
            part_id.to_owned(),
            AssistantStreamController::new(part_id, width, cwd),
        );
        self.order.push_back(part_id.to_owned());
        evicted
    }

    fn evict_oldest_controller(&mut self) -> Option<RenderCellRecord> {
        while let Some(part_id) = self.order.pop_front() {
            let Some(mut controller) = self.controllers.remove(&part_id) else {
                continue;
            };
            return controller.finalize().cell;
        }
        None
    }

    fn remove_controller(&mut self, part_id: &str) -> Option<AssistantStreamController> {
        self.order.retain(|current| current != part_id);
        self.controllers.remove(part_id)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::AssistantStreamRegistry;

    #[test]
    fn interleaved_parts_keep_independent_controllers() {
        let mut registry = AssistantStreamRegistry::with_capacity(2);
        let now = Instant::now();

        registry.push_delta("assistant:a", "A", 80, std::path::Path::new("."), now);
        registry.push_delta("assistant:b", "B", 80, std::path::Path::new("."), now);
        registry.push_delta("assistant:a", "2", 80, std::path::Path::new("."), now);

        assert_eq!(registry.stream_count(), 2);
        assert_eq!(registry.finalize_part("assistant:a").unwrap().text, "A2");
        assert_eq!(registry.finalize_part("assistant:b").unwrap().text, "B");
    }

    #[test]
    fn capacity_finalizes_oldest_part_without_losing_its_source() {
        let mut registry = AssistantStreamRegistry::with_capacity(1);
        let now = Instant::now();

        registry.push_delta("assistant:a", "A", 80, std::path::Path::new("."), now);
        let evicted = registry
            .push_delta("assistant:b", "B", 80, std::path::Path::new("."), now)
            .expect("最旧流应被结算");

        assert_eq!(evicted.text, "A");
        assert_eq!(registry.finalize_part("assistant:b").unwrap().text, "B");
    }

    #[test]
    fn commit_selection_rotates_across_parts() {
        let mut registry = AssistantStreamRegistry::with_capacity(2);
        let now = Instant::now();
        registry.push_delta("assistant:a", "A", 80, std::path::Path::new("."), now);
        registry.push_delta("assistant:b", "B", 80, std::path::Path::new("."), now);

        assert_eq!(
            registry.next_commit_part_id().as_deref(),
            Some("assistant:a")
        );
        assert_eq!(
            registry.next_commit_part_id().as_deref(),
            Some("assistant:b")
        );
        assert_eq!(
            registry.next_commit_part_id().as_deref(),
            Some("assistant:a")
        );
    }
}
