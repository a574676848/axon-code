//! 中途落盘：同一条 active turn 的新增历史 item 直接追加进 Thread 事件流。
//!
//! 一轮可能跨多次落盘（起跑、工具回执结算、终态）。这里只补「新增或内容已变」的 item，
//! 轮次保持 Active：终态由收口路径单独写入，中途落盘不得替它收口。
//!
//! 读侧只走尾部：现有 item 从 thread 尾部投影得到，不整条读历史。尾部没覆盖到本轮
//! 起跑事件（超长轮次或预算不足）时才回退整条读，保证正确性优先。

use std::path::Path;

use axon_session::{JsonlThreadEventStore, ThreadService, TurnStatus};

use super::run_session_store::{RunSessionRecord, resolved_axon_root};

/// 把已有 active turn 的新增历史 item 追加进 Thread。
///
/// Thread 还不存在时退回「起跑」路径：真实链路里起跑一定先于结算，这里只兜底
/// 崩溃恢复场景，不改变正常语义。
pub(super) fn mirror_session_partial(
    config_path: &Path,
    record: &RunSessionRecord,
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let Ok(activation) = service.read_thread_activation(&root, &record.session_id) else {
        return super::run_thread_store::mirror_session_started(config_path, record);
    };
    if !activation.exists {
        return super::run_thread_store::mirror_session_started(config_path, record);
    }
    let turn = super::run_thread_store::turn_from_record(record, 0)?;
    // 本轮还没起跑（新轮次的第一次落盘）：走起跑路径登记 active turn。
    if activation.active_turn_id.as_deref() != Some(turn.turn_id.as_str()) {
        return super::run_thread_store::mirror_session_started(config_path, record);
    }
    let existing = match super::run_thread_tail::active_turn_items(
        config_path,
        &record.session_id,
        &turn.turn_id,
    )? {
        Some(items) => items,
        None => {
            // 尾部不足以覆盖本轮起跑：回退整条读，绝不在缺少现状时盲目追加。
            let detail = service.read_thread(&root, &record.session_id)?;
            let active = detail
                .turns
                .iter()
                .find(|value| value.turn_id == turn.turn_id)
                .ok_or_else(|| "thread 缺少待追加的 active turn".to_owned())?;
            active.history_items.clone()
        }
    };
    let items = super::run_thread_store::pending_turn_items(&existing, &turn.history_items);
    if items.is_empty() {
        return Ok(());
    }
    let mut append = turn;
    append.status = TurnStatus::Active;
    append.completed_at_ms = None;
    append.interrupted_at_ms = None;
    service.append_turn_items_at_sequence(
        &root,
        &record.session_id,
        append.turn_id.clone(),
        items,
        record.created_at_ms,
        activation.next_sequence,
    )
}
