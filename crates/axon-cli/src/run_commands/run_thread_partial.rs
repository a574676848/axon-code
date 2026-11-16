//! 中途落盘：同一条 active turn 的新增历史 item 直接追加进 Thread 事件流。
//!
//! 一轮可能跨多次落盘（起跑、工具回执结算、终态）。这里只补「新增 item」，
//! 轮次保持 Active：终态由收口路径单独写入，中途落盘不得替它收口。

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
    let Ok(detail) = service.read_thread(&root, &record.session_id) else {
        return super::run_thread_store::mirror_session_started(config_path, record);
    };
    let mut turn = super::run_thread_store::turn_from_record(record, 0)?;
    turn.status = TurnStatus::Active;
    turn.completed_at_ms = None;
    turn.interrupted_at_ms = None;
    super::run_thread_store::mirror_existing_turn(
        &service,
        &root,
        &record.session_id,
        &detail,
        turn,
        record,
    )
}
