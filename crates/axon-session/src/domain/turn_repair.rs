//! 未收口轮次的纯合成逻辑。
//!
//! 对齐 DSH `openTurnClosers` 的语义，但去掉 Axon 不存在的 step 层级：Axon 的工具历史
//! 以批量 `TurnItemsAppended` 落盘，因此「调用是否已有结果」必须扫描 `Turn.history_items`，
//! 而不是像 DSH 那样按 `tool/call` 与 `tool/result` 事件驱动状态机。
//!
//! 本模块是纯函数：不读盘、不取时钟、不写日志。判定依据完全来自日志自身的结构，
//! 因此同一段历史无论何时修复都得到同一份合成结果。

use std::collections::HashSet;

use super::history::{ThreadHistoryItem, ThreadHistoryReceipt, ThreadHistoryStatus};
use super::turn::{Turn, TurnStatus};

/// 工具调用连「开始」都没被记录：可以放心重试。
pub const TOOL_NOT_STARTED: &str = "TOOL_NOT_STARTED";
/// 工具调用已经被接受，但没有落盘结果：是否有副作用未知。
pub const TOOL_OUTCOME_UNKNOWN: &str = "TOOL_OUTCOME_UNKNOWN";

/// 返回未收口的尾部轮次；历史已经平衡时返回 `None`。
///
/// 判据只用结构：最后一轮仍是 `Active`，就说明写它的进程没有走到收口。已终止的三态
/// （`Completed` / `Interrupted` / `Failed`）都视为收口，不做二次判断。
#[must_use]
pub fn open_tail_turn(turns: &[Turn]) -> Option<&Turn> {
    turns
        .last()
        .filter(|turn| turn.status == TurnStatus::Active)
}

/// 为尾部轮次里「有调用、无结果」的 part 合成中断回执。
///
/// 配对键必须是 `part_id`：模型历史装配按 `part_id` 把结果挂回对应调用，用别的键
/// 合成出来的结果永远投不进 provider 请求。合成结果同样必须带 `receipt`，因为装配
/// 侧只认 `receipt: Some(..)` 的结果项。
///
/// 返回顺序按 `history_items` 原序，与 DSH「保持 transcript 顺序」的约束一致。
#[must_use]
pub fn synthetic_tool_results(turn: &Turn) -> Vec<ThreadHistoryItem> {
    let mut settled: HashSet<&str> = HashSet::new();
    let mut next_sequence = 0_u64;
    for item in &turn.history_items {
        next_sequence = next_sequence.max(item.sequence().saturating_add(1));
        if let ThreadHistoryItem::ToolResult { part_id, .. } = item {
            settled.insert(part_id.as_str());
        }
    }

    let mut results = Vec::new();
    for item in &turn.history_items {
        let ThreadHistoryItem::ToolCall {
            provider_turn_id,
            part_id,
            tool_call_id,
            operation_id,
            provider_call_id,
            tool_name,
            status,
            ..
        } = item
        else {
            continue;
        };
        // 已经有结果的调用不再重复补齐；已经终止的调用也不需要合成。
        if *status != ThreadHistoryStatus::InProgress || settled.contains(part_id.as_str()) {
            continue;
        }
        results.push(ThreadHistoryItem::ToolResult {
            sequence: next_sequence,
            provider_turn_id: *provider_turn_id,
            part_id: part_id.clone(),
            tool_call_id: tool_call_id.clone(),
            operation_id: operation_id.clone(),
            provider_call_id: provider_call_id.clone(),
            tool_name: tool_name.clone(),
            content: unknown_outcome_text(),
            status: ThreadHistoryStatus::Interrupted,
            receipt: Some(Box::new(ThreadHistoryReceipt {
                status: ThreadHistoryStatus::Interrupted,
                summary: unknown_outcome_text(),
                metadata: None,
                output_ref: None,
                error_type: Some(TOOL_OUTCOME_UNKNOWN.to_owned()),
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                duration_ms: None,
            })),
            provider_metadata: None,
        });
        next_sequence = next_sequence.saturating_add(1);
    }
    results
}

/// 面向模型的措辞：说清「结果未知」，并给出可执行的重试判据。
fn unknown_outcome_text() -> String {
    "这次工具调用已被接受，但进程在记录结果之前退出，是否真正执行未知。\
     仅当该操作只读或幂等时才可直接重试；如果它可能有副作用，请先核实外部状态再决定，不要盲目重试。"
        .to_owned()
}
