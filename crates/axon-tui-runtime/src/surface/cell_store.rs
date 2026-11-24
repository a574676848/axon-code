//! Render cell 存储事实源。

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    hash::{Hash, Hasher},
};

use crate::surface::{
    cell::{RenderCell, RenderCellRecord, render_cell_record, render_cell_record_mut},
    cell_id::CellId,
};

#[path = "cell_store_conversions.rs"]
mod cell_store_conversions;
#[path = "cell_store_types.rs"]
mod cell_store_types;
use cell_store_types::{CELL_COMMIT_STATE_MISSING_ERROR, RENDER_CELL_RECORD_CONTRACT_ERROR};
pub(crate) use cell_store_types::{CellCommitState, CellStore, CellStoreError};

pub(crate) const CELL_HISTORY_MAX_CELLS: usize = 4_000;
pub(crate) const CELL_HISTORY_GC_TARGET_CELLS: usize = 3_800;
const TRANSCRIPT_SIGNATURE_INDEX_FACTOR: u64 = 0x9E37_79B1_85EB_CA87;

#[derive(Debug, Clone)]
struct RenderCellEntry {
    cell: Box<dyn RenderCell>,
    state: CellCommitState,
    last_access: Cell<u64>,
}

impl RenderCellEntry {
    fn new(cell: RenderCellRecord, state: CellCommitState) -> Self {
        Self {
            cell: Box::new(cell),
            state,
            last_access: Cell::new(0),
        }
    }

    fn pending(cell: RenderCellRecord) -> Self {
        Self::new(cell, CellCommitState::Pending)
    }

    const fn is_history_evictable(&self) -> bool {
        matches!(self.state, CellCommitState::Committed)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct TranscriptCellsSignature {
    pub(crate) cell_count: usize,
    pub(crate) cells_hash: u64,
}

#[derive(Debug, Clone, Default)]
struct TranscriptCellSignatureCache {
    fingerprints: Vec<u64>,
    cells_hash: u64,
    valid: bool,
    #[cfg(test)]
    rebuilds: u64,
}

impl TranscriptCellSignatureCache {
    fn from_entries(cells: &[RenderCellEntry]) -> Self {
        let mut cache = Self {
            fingerprints: cells.iter().map(cell_fingerprint).collect(),
            cells_hash: 0,
            valid: true,
            #[cfg(test)]
            rebuilds: 1,
        };
        cache.recompute_hash();
        cache
    }

    fn signature(&mut self, cells: &[RenderCellEntry]) -> TranscriptCellsSignature {
        if !self.valid || self.fingerprints.len() != cells.len() {
            self.rebuild_from_entries(cells);
        }
        TranscriptCellsSignature {
            cell_count: self.fingerprints.len(),
            cells_hash: self.cells_hash,
        }
    }

    fn push(&mut self, entry: &RenderCellEntry) {
        if !self.valid {
            return;
        }
        let fingerprint = cell_fingerprint(entry);
        let index = self.fingerprints.len();
        self.fingerprints.push(fingerprint);
        self.cells_hash = self
            .cells_hash
            .wrapping_add(indexed_fingerprint(index, fingerprint));
    }

    fn replace(&mut self, index: usize, entry: &RenderCellEntry) {
        if !self.valid {
            return;
        }
        let Some(slot) = self.fingerprints.get_mut(index) else {
            self.valid = false;
            return;
        };
        let previous = *slot;
        let next = cell_fingerprint(entry);
        *slot = next;
        self.cells_hash = self
            .cells_hash
            .wrapping_sub(indexed_fingerprint(index, previous))
            .wrapping_add(indexed_fingerprint(index, next));
    }

    fn rebuild_from_entries(&mut self, cells: &[RenderCellEntry]) {
        #[cfg(test)]
        let rebuilds = self.rebuilds.saturating_add(1);
        *self = Self::from_entries(cells);
        #[cfg(test)]
        {
            self.rebuilds = rebuilds;
        }
    }

    fn clear(&mut self) {
        self.fingerprints.clear();
        self.cells_hash = 0;
        self.valid = true;
    }

    fn invalidate(&mut self) {
        self.valid = false;
    }

    fn recompute_hash(&mut self) {
        self.cells_hash =
            self.fingerprints.iter().copied().enumerate().fold(
                0u64,
                |hash, (index, fingerprint)| {
                    hash.wrapping_add(indexed_fingerprint(index, fingerprint))
                },
            );
    }
}

fn cell_fingerprint(entry: &RenderCellEntry) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entry.cell.revision().hash(&mut hasher);
    entry.cell.transcript_key().hash(&mut hasher);
    entry.state.hash(&mut hasher);
    if let Some(record) = render_cell_record(entry.cell.as_ref()) {
        record.part_id().hash(&mut hasher);
        record.text.len().hash(&mut hasher);
    } else {
        entry.cell.kind().hash(&mut hasher);
    }
    hasher.finish()
}

fn indexed_fingerprint(index: usize, fingerprint: u64) -> u64 {
    fingerprint ^ ((index as u64).wrapping_mul(TRANSCRIPT_SIGNATURE_INDEX_FACTOR))
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RenderCellList {
    cells: Vec<RenderCellEntry>,
    signature: RefCell<TranscriptCellSignatureCache>,
    access_epoch: Cell<u64>,
    hot: CellHotTable,
    part_slots_by_id: HashMap<String, crate::surface::runtime_cells::PartSlot>,
    part_indices_by_id: HashMap<String, usize>,
    next_part_slot: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CellHotTable {
    pub(crate) kind: Vec<&'static str>,
    pub(crate) revision: Vec<u64>,
    pub(crate) commit_state: Vec<CellCommitState>,
    pub(crate) part_slot: Vec<Option<crate::surface::runtime_cells::PartSlot>>,
    pub(crate) transcript_key: Vec<u64>,
    #[cfg(test)]
    rebuilds: u64,
}

impl CellHotTable {
    fn clear(&mut self) {
        self.kind.clear();
        self.revision.clear();
        self.commit_state.clear();
        self.part_slot.clear();
        self.transcript_key.clear();
    }

    fn remove(&mut self, index: usize) {
        self.kind.remove(index);
        self.revision.remove(index);
        self.commit_state.remove(index);
        self.part_slot.remove(index);
        self.transcript_key.remove(index);
    }
}

impl RenderCellList {
    pub(crate) fn from_records(records: Vec<RenderCellRecord>) -> Self {
        let cells = records
            .into_iter()
            .map(|record| RenderCellEntry::new(record, CellCommitState::Committed))
            .collect::<Vec<_>>();
        let signature = TranscriptCellSignatureCache::from_entries(&cells);
        let mut list = Self {
            cells,
            signature: RefCell::new(signature),
            access_epoch: Cell::new(0),
            hot: CellHotTable::default(),
            part_slots_by_id: HashMap::new(),
            part_indices_by_id: HashMap::new(),
            next_part_slot: 0,
        };
        list.rebuild_hot_table();
        list
    }

    pub(crate) fn insert_cell(&mut self, cell: RenderCellRecord) -> usize {
        let index = self.cells.len();
        self.cells.push(RenderCellEntry::pending(cell));
        self.update_signature_for_index(index);
        self.append_hot_entry(index);
        self.index_part_id(index);
        index
    }

    pub(crate) fn push(&mut self, cell: RenderCellRecord) {
        let index = self.insert_cell(cell);
        // Invariant: insert_cell returns the index it just appended.
        self.commit_cell(index)
            .expect(CELL_COMMIT_STATE_MISSING_ERROR);
    }

    pub(crate) fn push_active(&mut self, cell: RenderCellRecord) -> usize {
        let index = self.insert_cell(cell);
        self.cells[index].state = CellCommitState::Active;
        self.touch_index(index);
        self.update_signature_for_index(index);
        self.refresh_hot_entry(index);
        index
    }

    pub(crate) fn extend<I>(&mut self, cells: I)
    where
        I: IntoIterator<Item = RenderCellRecord>,
    {
        for cell in cells {
            self.cells
                .push(RenderCellEntry::new(cell, CellCommitState::Committed));
            self.update_signature_for_index(self.cells.len() - 1);
        }
        self.rebuild_hot_table();
    }

    pub(crate) fn splice_one(&mut self, range: std::ops::Range<usize>, cell: RenderCellRecord) {
        self.cells.splice(
            range,
            [RenderCellEntry::new(cell, CellCommitState::Committed)],
        );
        self.rebuild_signature_cache();
        self.rebuild_hot_table();
    }

    pub(crate) fn replace_cell(
        &mut self,
        index: usize,
        cell: RenderCellRecord,
    ) -> Result<(), CellStoreError> {
        let previous_part_id = self.get(index).map(RenderCellRecord::part_id);
        let Some(entry) = self.cells.get_mut(index) else {
            return Err(CellStoreError::Missing);
        };
        let Some(existing) = render_cell_record_mut(entry.cell.as_mut()) else {
            return Err(CellStoreError::NotRecord);
        };
        existing.replace_with(cell);
        self.touch_index(index);
        self.update_signature_for_index(index);
        self.refresh_hot_entry(index);
        self.reindex_changed_part_id(index, previous_part_id);
        Ok(())
    }

    pub(crate) fn replace_active_cell(
        &mut self,
        index: usize,
        cell: RenderCellRecord,
    ) -> Option<RenderCellRecord> {
        let previous_part_id = self.get(index).map(RenderCellRecord::part_id);
        let Some(entry) = self.cells.get_mut(index) else {
            return Some(cell);
        };
        let state = self
            .hot
            .commit_state
            .get(index)
            .copied()
            .unwrap_or(entry.state);
        if state != CellCommitState::Active {
            return Some(cell);
        }
        let Some(existing) = render_cell_record_mut(entry.cell.as_mut()) else {
            return Some(cell);
        };
        existing.replace_with(cell);
        self.touch_index(index);
        self.update_signature_for_index(index);
        self.refresh_hot_entry(index);
        self.reindex_changed_part_id(index, previous_part_id);
        None
    }

    pub(crate) fn mutate_cell<F>(&mut self, index: usize, mutate: F) -> Result<(), CellStoreError>
    where
        F: FnOnce(&mut RenderCellRecord),
    {
        let previous_part_id = self.get(index).map(RenderCellRecord::part_id);
        let Some(entry) = self.cells.get_mut(index) else {
            return Err(CellStoreError::Missing);
        };
        let state = self
            .hot
            .commit_state
            .get(index)
            .copied()
            .unwrap_or(entry.state);
        if state != CellCommitState::Active {
            return Err(CellStoreError::Inactive { state });
        }
        let Some(record) = render_cell_record_mut(entry.cell.as_mut()) else {
            return Err(CellStoreError::NotRecord);
        };
        mutate(record);
        self.update_signature_for_index(index);
        self.refresh_hot_entry(index);
        self.reindex_changed_part_id(index, previous_part_id);
        Ok(())
    }

    pub(crate) fn commit_cell(&mut self, index: usize) -> Result<(), CellStoreError> {
        let Some(entry) = self.cells.get_mut(index) else {
            return Err(CellStoreError::Missing);
        };
        entry.state = CellCommitState::Committed;
        self.touch_index(index);
        self.refresh_hot_entry(index);
        self.update_signature_for_index(index);
        Ok(())
    }

    pub(crate) fn evict_cell(&mut self, index: usize) -> Result<RenderCellRecord, CellStoreError> {
        let Some(entry) = self.cells.get(index) else {
            return Err(CellStoreError::Missing);
        };
        let record = render_cell_record(entry.cell.as_ref())
            .ok_or(CellStoreError::NotRecord)?
            .clone();
        self.discard_cell(index)?;
        Ok(record)
    }

    pub(crate) fn evict_oldest_history_cells_to(
        &mut self,
        target_len: usize,
    ) -> Vec<(usize, CellId)> {
        let remaining = self.cells.len().saturating_sub(target_len);
        if remaining == 0 {
            return Vec::new();
        }

        let mut evicted = Vec::with_capacity(remaining);
        let mut retained = Vec::with_capacity(self.cells.len().saturating_sub(remaining));
        let mut candidates = self
            .cells
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.is_history_evictable() && render_cell_record(entry.cell.as_ref()).is_some()
            })
            .map(|(index, entry)| (index, entry.last_access.get()))
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(index, access)| (*access, *index));
        let evict_indices = candidates
            .into_iter()
            .take(remaining)
            .map(|(index, _)| index)
            .collect::<std::collections::HashSet<_>>();
        for (index, mut entry) in self.cells.drain(..).enumerate() {
            if evict_indices.contains(&index) {
                entry.state = CellCommitState::Evicted;
                evicted.push((index, entry.cell.id()));
            } else {
                retained.push(entry);
            }
        }
        self.cells = retained;

        if !evicted.is_empty() {
            self.prune_part_slots();
            self.rebuild_signature_cache();
            self.rebuild_hot_table();
        }
        evicted
    }

    fn discard_cell(&mut self, index: usize) -> Result<CellId, CellStoreError> {
        let Some(entry) = self.cells.get(index) else {
            return Err(CellStoreError::Missing);
        };
        let cell_id = entry.cell.id();
        render_cell_record(entry.cell.as_ref()).ok_or(CellStoreError::NotRecord)?;
        let Some(entry) = self.cells.get_mut(index) else {
            return Err(CellStoreError::Missing);
        };
        entry.state = CellCommitState::Evicted;
        self.cells.remove(index);
        self.prune_part_slots();
        self.rebuild_part_indices();
        self.rebuild_signature_cache();
        self.hot.remove(index);
        Ok(cell_id)
    }

    pub(crate) fn remove(&mut self, index: usize) -> RenderCellRecord {
        // Invariant: remove preserves Vec-like indexing semantics for callers that already
        // validated the transcript index.
        self.evict_cell(index)
            .expect(CELL_COMMIT_STATE_MISSING_ERROR)
    }

    pub(crate) fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&RenderCellRecord) -> bool,
    {
        self.cells
            .retain(|entry| render_cell_record(entry.cell.as_ref()).is_some_and(&mut keep));
        self.prune_part_slots();
        self.rebuild_signature_cache();
        self.rebuild_hot_table();
    }

    pub(crate) fn clear(&mut self) {
        self.cells.clear();
        self.signature.borrow_mut().clear();
        self.access_epoch.set(0);
        self.hot.clear();
        self.part_slots_by_id.clear();
        self.part_indices_by_id.clear();
        self.next_part_slot = 0;
    }

    pub(crate) fn len(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn exceeds_history_budget(&self) -> bool {
        self.cells.len() > CELL_HISTORY_MAX_CELLS
    }

    pub(crate) fn history_gc_target_len(&self) -> usize {
        CELL_HISTORY_GC_TARGET_CELLS.min(self.cells.len())
    }

    pub(crate) fn get(&self, index: usize) -> Option<&RenderCellRecord> {
        self.get_cell(index).and_then(render_cell_record)
    }

    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut RenderCellRecord> {
        self.invalidate_signature_cache();
        self.get_cell_mut(index).and_then(render_cell_record_mut)
    }

    pub(crate) fn get_cell(&self, index: usize) -> Option<&dyn RenderCell> {
        let entry = self.cells.get(index)?;
        self.touch_entry(entry);
        Some(entry.cell.as_ref())
    }

    pub(crate) fn get_cell_mut(&mut self, index: usize) -> Option<&mut dyn RenderCell> {
        self.invalidate_signature_cache();
        self.cells.get_mut(index).map(|entry| entry.cell.as_mut())
    }

    #[cfg(test)]
    pub(crate) fn last(&self) -> Option<&RenderCellRecord> {
        self.last_cell().and_then(render_cell_record)
    }

    #[cfg(test)]
    pub(crate) fn last_cell(&self) -> Option<&dyn RenderCell> {
        self.cells.last().map(|entry| entry.cell.as_ref())
    }

    pub(crate) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = &RenderCellRecord> + ExactSizeIterator {
        self.cells.iter().map(|entry| {
            // Invariant: RenderCellList::iter is the record-only iterator; mixed cell render paths
            // use iter_cells.
            render_cell_record(entry.cell.as_ref()).expect(RENDER_CELL_RECORD_CONTRACT_ERROR)
        })
    }

    pub(crate) fn iter_cells(&self) -> impl Iterator<Item = &dyn RenderCell> {
        self.cells.iter().map(|entry| entry.cell.as_ref())
    }

    pub(crate) fn to_record_vec(&self) -> Vec<RenderCellRecord> {
        self.iter().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn replace_with_records(&mut self, records: Vec<RenderCellRecord>) {
        *self = Self::from_records(records);
    }

    pub(crate) fn transcript_signature(&self) -> TranscriptCellsSignature {
        self.signature.borrow_mut().signature(&self.cells)
    }

    pub(crate) fn transcript_revision(&self) -> u64 {
        self.transcript_signature().cells_hash
    }

    #[cfg(test)]
    pub(crate) fn transcript_signature_rebuilds(&self) -> u64 {
        self.signature.borrow().rebuilds
    }

    fn update_signature_for_index(&self, index: usize) {
        let Some(entry) = self.cells.get(index) else {
            self.invalidate_signature_cache();
            return;
        };
        let mut signature = self.signature.borrow_mut();
        if index == signature.fingerprints.len() {
            signature.push(entry);
        } else {
            signature.replace(index, entry);
        }
    }

    fn rebuild_signature_cache(&self) {
        self.signature
            .borrow_mut()
            .rebuild_from_entries(&self.cells);
    }

    fn invalidate_signature_cache(&self) {
        self.signature.borrow_mut().invalidate();
    }

    fn touch_index(&mut self, index: usize) {
        let access = self.next_access_epoch();
        if let Some(entry) = self.cells.get(index) {
            entry.last_access.set(access);
        }
    }

    fn touch_entry(&self, entry: &RenderCellEntry) {
        entry.last_access.set(self.next_access_epoch());
    }

    fn next_access_epoch(&self) -> u64 {
        let epoch = self.access_epoch.get().wrapping_add(1);
        self.access_epoch.set(epoch);
        epoch
    }

    fn rebuild_hot_table(&mut self) {
        self.hot.clear();
        self.part_indices_by_id.clear();
        for index in 0..self.cells.len() {
            self.append_hot_entry(index);
            self.index_part_id(index);
        }
        #[cfg(test)]
        {
            self.hot.rebuilds = self.hot.rebuilds.saturating_add(1);
        }
    }

    fn rebuild_part_indices(&mut self) {
        self.part_indices_by_id.clear();
        for index in 0..self.cells.len() {
            self.index_part_id(index);
        }
    }

    fn index_part_id(&mut self, index: usize) {
        if let Some(part_id) = self
            .cells
            .get(index)
            .and_then(|entry| render_cell_record(entry.cell.as_ref()))
            .map(RenderCellRecord::part_id)
        {
            self.part_indices_by_id.insert(part_id, index);
        }
    }

    fn reindex_changed_part_id(&mut self, index: usize, previous_part_id: Option<String>) {
        let current_part_id = self
            .cells
            .get(index)
            .and_then(|entry| render_cell_record(entry.cell.as_ref()))
            .map(RenderCellRecord::part_id);
        if previous_part_id != current_part_id {
            if let Some(previous) = previous_part_id {
                self.part_indices_by_id.remove(&previous);
            }
            if let Some(current) = current_part_id {
                self.part_indices_by_id.insert(current, index);
            }
        }
    }

    fn prune_part_slots(&mut self) {
        let live_ids = self
            .cells
            .iter()
            .filter_map(|entry| render_cell_record(entry.cell.as_ref()))
            .map(RenderCellRecord::part_id)
            .collect::<std::collections::HashSet<_>>();
        self.part_slots_by_id
            .retain(|part_id, _| live_ids.contains(part_id));
    }

    fn append_hot_entry(&mut self, index: usize) {
        debug_assert_eq!(index, self.hot.kind.len());
        let (kind, revision, state, part_id, transcript_key) = self.hot_entry_data(index);
        let part_slot = part_id
            .as_deref()
            .map(|part_id| self.part_slot_for_id(part_id));
        self.hot.kind.push(kind);
        self.hot.revision.push(revision);
        self.hot.commit_state.push(state);
        self.hot.part_slot.push(part_slot);
        self.hot.transcript_key.push(transcript_key);
    }

    fn refresh_hot_entry(&mut self, index: usize) {
        if index >= self.hot.kind.len() {
            self.rebuild_hot_table();
            return;
        }
        let (kind, revision, state, part_id, transcript_key) = self.hot_entry_data(index);
        let part_slot = part_id
            .as_deref()
            .map(|part_id| self.part_slot_for_id(part_id));
        self.hot.kind[index] = kind;
        self.hot.revision[index] = revision;
        self.hot.commit_state[index] = state;
        self.hot.part_slot[index] = part_slot;
        self.hot.transcript_key[index] = transcript_key;
    }

    fn hot_entry_data(
        &self,
        index: usize,
    ) -> (&'static str, u64, CellCommitState, Option<String>, u64) {
        let entry = &self.cells[index];
        let part_id = render_cell_record(entry.cell.as_ref()).map(RenderCellRecord::part_id);
        (
            entry.cell.kind(),
            entry.cell.revision(),
            entry.state,
            part_id,
            entry.cell.transcript_key(),
        )
    }

    fn part_slot_for_id(&mut self, part_id: &str) -> crate::surface::runtime_cells::PartSlot {
        if let Some(slot) = self.part_slots_by_id.get(part_id).copied() {
            return slot;
        }
        let slot = crate::surface::runtime_cells::PartSlot(self.next_part_slot);
        self.next_part_slot = self.next_part_slot.wrapping_add(1);
        self.part_slots_by_id.insert(part_id.to_owned(), slot);
        slot
    }

    #[cfg(test)]
    fn hot_table_rebuilds(&self) -> u64 {
        self.hot.rebuilds
    }
}

#[cfg(test)]
#[path = "cell_store_tests.rs"]
mod tests;
