//! TUI 内部 transcript 事件。
//!
//! 这些事件只在 TUI 状态层内流转，不进入 runtime run output 或跨 crate 协议。

use std::time::Instant;

use crossterm::event::{KeyEvent, MouseEvent};

use crate::surface::cell::RenderCellRecord;

#[derive(Debug, Clone)]
pub(crate) enum AgentTuiEvent {
    ConsolidateAgentMessage {
        cell: RenderCellRecord,
    },
    ConsolidateProposedPlan {
        deferred_cell: Option<RenderCellRecord>,
    },
}

/// TUI 本地事件只描述宿主终端输入，不承载 runtime worker 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UiEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize { width: u16, height: u16 },
    Tick { now: Instant },
}
