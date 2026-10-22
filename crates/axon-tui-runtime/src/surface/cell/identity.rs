//! Render cell 的稳定语义身份。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::surface::cell::{
    RENDER_CELL_REGISTRY, RenderCellData, RenderCellKind, RenderCellMode, RenderCellRecord,
    RenderCellRegistryEntry,
};

const LINE_PART_PREFIX: &str = "line:text";
const STARTUP_PART_PREFIX: &str = "startup:text";

pub(crate) fn provider_call_id(data: Option<&RenderCellData>) -> Option<&str> {
    match data {
        Some(RenderCellData::Exec(cell)) => cell.provider_call_id.as_deref(),
        _ => None,
    }
}

pub(crate) fn part_id(record: &RenderCellRecord) -> String {
    if let Some(data) = &record.data {
        return typed_part_id(data);
    }
    let prefix = match record.mode {
        RenderCellMode::Startup => STARTUP_PART_PREFIX,
        RenderCellMode::Line | RenderCellMode::Cell(_) => LINE_PART_PREFIX,
    };
    fallback_text_part_id(prefix, record.role, &record.text)
}

pub(crate) fn semantic_part_id(record: &RenderCellRecord) -> Option<String> {
    record.data.as_ref().and_then(typed_semantic_part_id)
}

pub(crate) fn render_cell_registry_entry(
    kind: RenderCellKind,
) -> Option<&'static RenderCellRegistryEntry> {
    RENDER_CELL_REGISTRY.iter().find(|entry| entry.kind == kind)
}

pub(crate) fn is_primary_runtime_cell_kind(kind: RenderCellKind) -> bool {
    render_cell_registry_entry(kind).is_some_and(|entry| entry.runtime_selectable)
}

fn typed_part_id(data: &RenderCellData) -> String {
    match data {
        RenderCellData::Diff(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Command(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Exec(cell) => required_cell_part_id(&cell.id),
        RenderCellData::History(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Hook(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Mcp(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Message(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Plan(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Review(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Error(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Status(cell) => required_cell_part_id(&cell.id),
        RenderCellData::GenerationStatus(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Reasoning(cell) => required_cell_part_id(&cell.id),
        RenderCellData::TurnSummary(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Usage(cell) => required_cell_part_id(&cell.id),
        RenderCellData::Question(cell) => required_cell_part_id(&cell.id),
    }
}

fn typed_semantic_part_id(data: &RenderCellData) -> Option<String> {
    match data {
        RenderCellData::Diff(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Command(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Exec(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::History(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Hook(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Mcp(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Message(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Plan(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Review(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Error(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Status(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::GenerationStatus(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Reasoning(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::TurnSummary(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Usage(cell) => optional_cell_part_id(&cell.id),
        RenderCellData::Question(cell) => optional_cell_part_id(&cell.id),
    }
}

fn required_cell_part_id(id: &str) -> String {
    // Invariant: typed render cells are constructed with a non-empty payload-owned id.
    optional_cell_part_id(id).expect("structured render cell must carry a payload-owned part id")
}

fn optional_cell_part_id(id: &str) -> Option<String> {
    Some(id.trim().to_owned()).filter(|value| !value.is_empty())
}

fn fallback_text_part_id(prefix: &str, role: &str, text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    role.hash(&mut hasher);
    text.hash(&mut hasher);
    format!("{prefix}:{:016x}", hasher.finish())
}

pub(crate) fn render_cell_record(
    cell: &dyn crate::surface::cell::RenderCell,
) -> Option<&RenderCellRecord> {
    cell.as_any().downcast_ref::<RenderCellRecord>()
}

pub(crate) fn render_cell_record_mut(
    cell: &mut dyn crate::surface::cell::RenderCell,
) -> Option<&mut RenderCellRecord> {
    cell.as_any_mut().downcast_mut::<RenderCellRecord>()
}
