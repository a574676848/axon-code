//! 运行期 transcript 活动 cell 索引。

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use crate::surface::cell::RenderCellRecord;

#[derive(Debug, Clone, Default)]
pub(crate) struct ActiveRuntimeCells {
    part_interner: PartInterner,
    active_parts: ActivePartTable,
    diff_snapshot_part_ids: HashSet<String>,
    pending_hook_cells: HashMap<String, PendingHookCell>,
    hook_visible_since: HashMap<String, Instant>,
    quiet_hook_lingers: HashMap<String, Instant>,
    revision: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingHookCell {
    line: RenderCellRecord,
    reveal_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct RevealedHookCell {
    pub(crate) run_id: String,
    pub(crate) line: RenderCellRecord,
    pub(crate) visible_since: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ActiveRuntimePart {
    pub(crate) part_id: String,
    pub(crate) index: usize,
}

impl ActiveRuntimeCells {
    pub(crate) fn clear(&mut self) {
        self.part_interner.clear();
        self.active_parts.clear();
        self.diff_snapshot_part_ids.clear();
        self.pending_hook_cells.clear();
        self.hook_visible_since.clear();
        self.quiet_hook_lingers.clear();
        self.revision = 0;
    }

    pub(crate) fn set_command_cell(&mut self, part_id: String, index: usize) {
        self.set_part(part_id, ActivePartKind::Command, index, None);
    }

    pub(crate) fn take_command_cell(&mut self, part_id: &str) -> Option<ActiveRuntimePart> {
        self.take_part(part_id, ActivePartKind::Command)
            .map(|part| part.0)
    }

    pub(crate) fn take_command_cells(&mut self) -> Vec<ActiveRuntimePart> {
        self.take_parts_of_kind(ActivePartKind::Command)
    }

    pub(crate) fn set_history_lifecycle_cell(&mut self, part_id: String, index: usize) {
        self.set_part(part_id, ActivePartKind::HistoryLifecycle, index, None);
    }

    pub(crate) fn take_history_lifecycle_cell(
        &mut self,
        part_id: &str,
    ) -> Option<ActiveRuntimePart> {
        self.take_part(part_id, ActivePartKind::HistoryLifecycle)
            .map(|part| part.0)
    }

    pub(crate) fn take_history_lifecycle_cells(&mut self) -> Vec<ActiveRuntimePart> {
        self.take_parts_of_kind(ActivePartKind::HistoryLifecycle)
    }

    pub(crate) fn tool_cell(&self, part_id: &str) -> Option<usize> {
        self.part_record(part_id)
            .filter(|record| record.kind == ActivePartKind::Tool)
            .map(|record| record.transcript_index)
    }

    pub(crate) fn tool_part_ids(&self) -> impl Iterator<Item = &str> + '_ {
        self.active_parts.active_slots.iter().filter_map(|slot| {
            self.active_parts
                .record(*slot)
                .filter(|record| record.kind == ActivePartKind::Tool)
                .and_then(|_| self.part_interner.part_id(*slot))
        })
    }

    pub(crate) fn is_diff_snapshot_tool_part(&self, part_id: &str) -> bool {
        self.diff_snapshot_part_ids.contains(part_id)
            || self
                .part_record(part_id)
                .is_some_and(|record| record.flags.diff_snapshot)
    }

    pub(crate) fn active_tool_cells(&self) -> Vec<ActiveToolCell> {
        self.active_parts
            .active_slots
            .iter()
            .filter_map(|slot| {
                self.active_parts
                    .record(*slot)
                    .filter(|record| record.kind == ActivePartKind::Tool)
                    .map(|record| ActiveToolCell {
                        index: record.transcript_index,
                        started_at: record.started_at,
                    })
            })
            .collect()
    }

    pub(crate) fn active_indices(&self) -> Vec<usize> {
        let mut indices = self
            .active_parts
            .active_slots
            .iter()
            .filter_map(|slot| {
                self.active_parts
                    .record(*slot)
                    .map(|record| record.transcript_index)
            })
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        indices
    }

    pub(crate) fn active_runtime_parts(&self) -> Vec<ActiveRuntimePart> {
        let mut parts = self
            .active_parts
            .active_slots
            .iter()
            .filter_map(|slot| {
                let record = self.active_parts.record(*slot)?;
                Some(ActiveRuntimePart {
                    part_id: self.part_interner.part_id(*slot)?.to_owned(),
                    index: record.transcript_index,
                })
            })
            .collect::<Vec<_>>();
        parts.sort_by(|left, right| {
            left.index
                .cmp(&right.index)
                .then(left.part_id.cmp(&right.part_id))
        });
        parts
    }

    pub(crate) fn set_tool_cell(&mut self, part_id: String, index: usize, now: Instant) {
        self.set_part(part_id, ActivePartKind::Tool, index, Some(now));
    }

    pub(crate) fn take_tool_cell(&mut self, part_id: &str) -> Option<ActiveToolCell> {
        self.take_part(part_id, ActivePartKind::Tool)
            .map(|(part, started_at)| ActiveToolCell {
                index: part.index,
                started_at,
            })
    }
    pub(crate) fn clear_tool_cell_at_index(&mut self, index: usize) {
        let slots = self.active_parts.active_slots.clone();
        for slot in slots {
            let Some(record) = self.active_parts.record(slot).copied() else {
                continue;
            };
            if record.kind == ActivePartKind::Tool && record.transcript_index == index {
                self.remove_slot(slot);
            }
        }
        self.refresh_active_runtime_revision();
    }

    pub(crate) fn mark_diff_snapshot_tool_part(&mut self, part_id: String) {
        let slot = self.part_interner.intern(part_id);
        if let Some(record) = self.active_parts.record_mut(slot) {
            record.flags.diff_snapshot = true;
            self.refresh_active_runtime_revision();
        } else if let Some(part_id) = self.part_interner.part_id(slot) {
            self.diff_snapshot_part_ids.insert(part_id.to_owned());
        }
    }

    pub(crate) fn take_diff_snapshot_tool_part(&mut self, part_id: &str) -> bool {
        if self.diff_snapshot_part_ids.remove(part_id) {
            return true;
        }
        let Some(slot) = self.part_interner.lookup(part_id) else {
            return false;
        };
        let Some(record) = self.active_parts.record_mut(slot) else {
            return false;
        };
        let was_marked = record.flags.diff_snapshot;
        record.flags.diff_snapshot = false;
        was_marked
    }

    pub(crate) fn stage_hook_cell(
        &mut self,
        run_id: String,
        line: RenderCellRecord,
        reveal_at: Instant,
    ) {
        self.pending_hook_cells
            .insert(run_id, PendingHookCell { line, reveal_at });
    }

    pub(crate) fn take_pending_hook_cell(&mut self, run_id: &str) -> Option<RenderCellRecord> {
        self.pending_hook_cells
            .remove(run_id)
            .map(|pending| pending.line)
    }

    pub(crate) fn reveal_due_hook_cells(&mut self, now: Instant) -> Vec<RevealedHookCell> {
        let due_run_ids = self
            .pending_hook_cells
            .iter()
            .filter(|&(_run_id, pending)| now >= pending.reveal_at)
            .map(|(run_id, _pending)| run_id.clone())
            .collect::<Vec<_>>();
        due_run_ids
            .into_iter()
            .filter_map(|run_id| {
                self.pending_hook_cells
                    .remove(&run_id)
                    .map(|pending| RevealedHookCell {
                        run_id,
                        line: pending.line,
                        visible_since: now,
                    })
            })
            .collect()
    }

    pub(crate) fn hook_visible_since(&self, run_id: &str) -> Option<Instant> {
        self.hook_visible_since.get(run_id).copied()
    }

    pub(crate) fn set_hook_visible_since(&mut self, run_id: String, visible_since: Instant) {
        self.hook_visible_since
            .entry(run_id)
            .or_insert(visible_since);
    }

    pub(crate) fn clear_hook_run(&mut self, run_id: &str) {
        self.pending_hook_cells.remove(run_id);
        self.hook_visible_since.remove(run_id);
        self.quiet_hook_lingers.remove(run_id);
    }

    pub(crate) fn clear_hook_runs(&mut self) {
        self.pending_hook_cells.clear();
        self.hook_visible_since.clear();
        self.quiet_hook_lingers.clear();
    }

    pub(crate) fn stage_quiet_hook_linger(&mut self, run_id: String, removal_at: Instant) {
        self.quiet_hook_lingers.insert(run_id, removal_at);
    }

    pub(crate) fn take_due_quiet_hook_lingers(&mut self, now: Instant) -> Vec<String> {
        let due_run_ids = self
            .quiet_hook_lingers
            .iter()
            .filter(|&(_run_id, removal_at)| now >= *removal_at)
            .map(|(run_id, _removal_at)| run_id.clone())
            .collect::<Vec<_>>();
        for run_id in &due_run_ids {
            self.quiet_hook_lingers.remove(run_id);
        }
        due_run_ids
    }

    pub(crate) fn remove_transcript_index(&mut self, removed_index: usize) {
        let slots = self.active_parts.active_slots.clone();
        for slot in slots {
            let Some(record) = self.active_parts.record_mut(slot) else {
                continue;
            };
            if record.transcript_index == removed_index {
                self.remove_slot(slot);
            } else if record.transcript_index > removed_index {
                record.transcript_index -= 1;
            }
        }
        self.refresh_active_runtime_revision();
    }

    pub(crate) const fn active_runtime_revision(&self) -> u64 {
        self.revision
    }

    fn refresh_active_runtime_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn set_part(
        &mut self,
        part_id: String,
        kind: ActivePartKind,
        transcript_index: usize,
        started_at: Option<Instant>,
    ) {
        let slot = self.part_interner.intern(part_id);
        let current_started_at = self
            .active_parts
            .record(slot)
            .and_then(|record| record.started_at);
        self.active_parts.upsert(
            slot,
            ActivePartRecord {
                kind,
                transcript_index,
                started_at: started_at.or(current_started_at),
                flags: self
                    .active_parts
                    .record(slot)
                    .map(|record| record.flags)
                    .unwrap_or_default(),
            },
        );
        self.refresh_active_runtime_revision();
    }

    fn take_part(
        &mut self,
        part_id: &str,
        expected_kind: ActivePartKind,
    ) -> Option<(ActiveRuntimePart, Option<Instant>)> {
        let slot = self.part_interner.lookup(part_id)?;
        let record = self.active_parts.record(slot).copied()?;
        if record.kind != expected_kind {
            return None;
        }
        let part = ActiveRuntimePart {
            part_id: part_id.to_owned(),
            index: record.transcript_index,
        };
        let started_at = record.started_at;
        self.remove_slot(slot);
        self.refresh_active_runtime_revision();
        Some((part, started_at))
    }

    fn take_parts_of_kind(&mut self, kind: ActivePartKind) -> Vec<ActiveRuntimePart> {
        let slots = self.active_parts.active_slots.clone();
        let mut parts = Vec::new();
        for slot in slots {
            let Some(record) = self.active_parts.record(slot).copied() else {
                continue;
            };
            if record.kind != kind {
                continue;
            }
            if let Some(part_id) = self.part_interner.part_id(slot) {
                parts.push(ActiveRuntimePart {
                    part_id: part_id.to_owned(),
                    index: record.transcript_index,
                });
            }
            self.remove_slot(slot);
        }
        self.refresh_active_runtime_revision();
        parts
    }

    fn part_record(&self, part_id: &str) -> Option<&ActivePartRecord> {
        let slot = self.part_interner.lookup(part_id)?;
        self.active_parts.record(slot)
    }

    fn remove_slot(&mut self, slot: PartSlot) {
        self.active_parts.remove(slot);
        self.part_interner.release(slot);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveToolCell {
    pub(crate) index: usize,
    pub(crate) started_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PartSlot(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ActivePartKind {
    Command,
    HistoryLifecycle,
    Tool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ActivePartFlags {
    diff_snapshot: bool,
}

#[derive(Debug, Clone, Copy)]
struct ActivePartRecord {
    kind: ActivePartKind,
    transcript_index: usize,
    started_at: Option<Instant>,
    flags: ActivePartFlags,
}

#[derive(Debug, Default, Clone)]
struct ActivePartTable {
    records: Vec<Option<ActivePartRecord>>,
    active_slots: Vec<PartSlot>,
    active_positions: Vec<Option<usize>>,
}

impl ActivePartTable {
    fn clear(&mut self) {
        self.records.clear();
        self.active_slots.clear();
        self.active_positions.clear();
    }

    fn record(&self, slot: PartSlot) -> Option<&ActivePartRecord> {
        self.records.get(slot.0 as usize)?.as_ref()
    }

    fn record_mut(&mut self, slot: PartSlot) -> Option<&mut ActivePartRecord> {
        self.records.get_mut(slot.0 as usize)?.as_mut()
    }

    fn upsert(&mut self, slot: PartSlot, record: ActivePartRecord) {
        let index = slot.0 as usize;
        if index >= self.records.len() {
            self.records.resize(index + 1, None);
            self.active_positions.resize(index + 1, None);
        }
        self.records[index] = Some(record);
        if self.active_positions[index].is_none() {
            self.active_positions[index] = Some(self.active_slots.len());
            self.active_slots.push(slot);
        }
    }

    fn remove(&mut self, slot: PartSlot) {
        let index = slot.0 as usize;
        let Some(position) = self.active_positions.get_mut(index).and_then(Option::take) else {
            return;
        };
        let moved = self.active_slots.last().copied();
        self.active_slots.swap_remove(position);
        if let Some(moved) = moved.filter(|moved| *moved != slot) {
            self.active_positions[moved.0 as usize] = Some(position);
        }
        if let Some(record) = self.records.get_mut(index) {
            *record = None;
        }
    }
}

#[derive(Debug, Default, Clone)]
struct PartInterner {
    slots_by_part_id: HashMap<String, PartSlot>,
    part_ids: Vec<Option<String>>,
    free_slots: Vec<u32>,
}

impl PartInterner {
    fn clear(&mut self) {
        self.slots_by_part_id.clear();
        self.part_ids.clear();
        self.free_slots.clear();
    }

    fn intern(&mut self, part_id: String) -> PartSlot {
        if let Some(slot) = self.slots_by_part_id.get(&part_id).copied() {
            return slot;
        }
        let slot = if let Some(raw) = self.free_slots.pop() {
            let slot = PartSlot(raw);
            self.part_ids[raw as usize] = Some(part_id.clone());
            slot
        } else {
            let raw = u32::try_from(self.part_ids.len()).expect("part slot count exceeded u32");
            self.part_ids.push(Some(part_id.clone()));
            PartSlot(raw)
        };
        self.slots_by_part_id.insert(part_id, slot);
        slot
    }

    fn lookup(&self, part_id: &str) -> Option<PartSlot> {
        self.slots_by_part_id.get(part_id).copied()
    }

    fn part_id(&self, slot: PartSlot) -> Option<&str> {
        self.part_ids.get(slot.0 as usize)?.as_deref()
    }

    fn release(&mut self, slot: PartSlot) {
        let Some(part_id) = self
            .part_ids
            .get_mut(slot.0 as usize)
            .and_then(Option::take)
        else {
            return;
        };
        self.slots_by_part_id.remove(&part_id);
        self.free_slots.push(slot.0);
    }
}

#[cfg(test)]
#[path = "runtime_cells_tests.rs"]
mod tests;
