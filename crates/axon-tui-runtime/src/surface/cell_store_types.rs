use std::{collections::HashMap, fmt};

use crate::surface::{
    cell::RenderCellRecord, runtime_cells::ActiveRuntimeCells,
    tool_output_buffer::ToolOutputBuffer, transcript_surfaces::TranscriptSurfaceCells,
};

use super::RenderCellList;

pub(super) const RENDER_CELL_RECORD_CONTRACT_ERROR: &str =
    "RenderCellRecord 访问 API 收到了非 record cell，请改用 RenderCell trait API";
pub(super) const CELL_COMMIT_STATE_ACTIVE_REQUIRED_ERROR: &str =
    "mutate_cell 只能修改 Active cell，请先确认 cell 提交状态";
pub(super) const CELL_COMMIT_STATE_MISSING_ERROR: &str =
    "cell 提交状态 API 收到了不存在的 transcript index";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CellCommitState {
    Pending,
    Active,
    Committed,
    Evicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CellStoreError {
    Missing,
    Inactive { state: CellCommitState },
    NotRecord,
}

impl fmt::Display for CellStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str(CELL_COMMIT_STATE_MISSING_ERROR),
            Self::Inactive { .. } => formatter.write_str(CELL_COMMIT_STATE_ACTIVE_REQUIRED_ERROR),
            Self::NotRecord => formatter.write_str(RENDER_CELL_RECORD_CONTRACT_ERROR),
        }
    }
}

impl std::error::Error for CellStoreError {}

/// Transcript cell、活动 runtime cell 与按 transcript part id 归档的工具输出缓存。
#[derive(Debug, Clone)]
pub(crate) struct CellStore {
    pub(crate) transcript: RenderCellList,
    pub(crate) active_runtime_cells: ActiveRuntimeCells,
    pub(crate) transcript_surfaces: TranscriptSurfaceCells,
    pub(crate) tool_outputs_by_part_id: HashMap<String, ToolOutputBuffer>,
}

impl CellStore {
    pub(crate) fn new(transcript: Vec<RenderCellRecord>) -> Self {
        Self {
            transcript: RenderCellList::from_records(transcript),
            active_runtime_cells: ActiveRuntimeCells::default(),
            transcript_surfaces: TranscriptSurfaceCells::default(),
            tool_outputs_by_part_id: HashMap::new(),
        }
    }
}
