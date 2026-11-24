use crate::surface::{
    cell::{RenderCellData, RenderCellRecord},
    cell_event_data::{AgentRenderExecCell, AgentRenderMcpCell},
};

pub(super) fn exec_cell_data(line: &RenderCellRecord) -> Option<&AgentRenderExecCell> {
    match &line.data {
        Some(RenderCellData::Exec(cell)) => Some(cell),
        _ => None,
    }
}

pub(super) fn mcp_cell_data(line: &RenderCellRecord) -> Option<&AgentRenderMcpCell> {
    match &line.data {
        Some(RenderCellData::Mcp(cell)) => Some(cell),
        _ => None,
    }
}

pub(super) fn exec_cell_data_mut(line: &mut RenderCellRecord) -> Option<&mut AgentRenderExecCell> {
    match &mut line.data {
        Some(RenderCellData::Exec(cell)) => Some(cell),
        _ => None,
    }
}

pub(super) fn mcp_cell_data_mut(line: &mut RenderCellRecord) -> Option<&mut AgentRenderMcpCell> {
    match &mut line.data {
        Some(RenderCellData::Mcp(cell)) => Some(cell),
        _ => None,
    }
}
