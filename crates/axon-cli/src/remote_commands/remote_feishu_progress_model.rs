//! 飞书回合投影的折叠态模型。
//!
//! 归并规则本身已抽到共享 crate `axon-turn-view`：正文按 `part_id` 归段、`delta` 追加、
//! 整段提交覆写；工具按 `tool_call_id` 归行，结果只补状态与读数；思考按 `part_id` 只留
//! 耗时；提示按 `key` 覆盖。飞书这边只保留自己的展示策略（8000 字预算、过程行窗口、
//! `MEDIA:` 行隐藏），因此它和 CLI、TUI 共用同一份事实归并，而不是各写一遍。

pub(crate) use axon_turn_view::{
    Cell, NoticeRow, ReasoningRow, RowStatus, ToolRow, TurnEvent, TurnView,
};
