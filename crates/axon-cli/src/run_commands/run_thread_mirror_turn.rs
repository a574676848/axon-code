//! 收口轮次的 thread 镜像：把一轮的终态落进 thread 事件流。
//!
//! 从 `run_thread_store` 拆出：那一层负责时序与会话恢复，本模块只做「把一轮写成
//! thread 事件」这一件事，顺带定下它的轮序。

use super::*;

/// 将 provider child turn 收口到 Thread 终态。
///
/// 返回值是本轮在 thread 里的轮序；调用方随侧车一起落地，读侧因此不必为了一个标量
/// 再读 thread 尾窗。
pub(crate) fn mirror_existing_turn(
    service: &ThreadService<JsonlThreadEventStore>,
    root: &Path,
    thread_id: &str,
    detail: &ThreadDetail,
    turn: Turn,
    record: &RunSessionRecord,
) -> Result<Option<u64>, String> {
    if turn.status == TurnStatus::Active {
        if let Some(active) = detail
            .turns
            .iter()
            .find(|value| value.status == TurnStatus::Active)
        {
            if active.turn_id != turn.turn_id {
                return Err("thread 已存在不同 active turn".to_owned());
            }
            let items = pending_turn_items(&active.history_items, &turn.history_items);
            if items.is_empty() {
                return Ok(None);
            }
            return service
                .append_turn_items(
                    root,
                    AppendTurnItemsCommand {
                        thread_id: thread_id.to_owned(),
                        turn_id: turn.turn_id,
                        items,
                        recorded_at_ms: record.created_at_ms,
                    },
                )
                .map(|()| None);
        }
        return service
            .start_turn(
                root,
                StartTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms: record.created_at_ms,
                    turn,
                },
            )
            .map(|()| None);
    }
    if let Some(active) = detail
        .turns
        .iter()
        .find(|value| value.status == TurnStatus::Active)
    {
        /*
         * 占位轮次的序号一律从 0 起（起跑镜像还不知道它排第几），收口时必须在这里定下
         * 真正的轮序：沿用 `active.sequence` 会让每一轮都拿到 0，`trace:<session>:<sequence>`、
         * `card_id` 与「按序号定位轮次」随即全轮撞车，治理显式信号也全部退化。
         */
        let sequence = next_sequence(detail, Some(active.turn_id.as_str()));
        // detail 已确认过这一轮：追加不再把整条 thread 读第二遍。
        return service
            .append_turn_at_tail(
                root,
                AppendTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms: record.created_at_ms,
                    turn: Turn {
                        turn_id: active.turn_id.clone(),
                        sequence,
                        ..turn
                    },
                },
            )
            .map(|()| Some(sequence));
    }
    /*
     * 下一轮的序号取「当前最大序号 + 1」，不是「轮数」。
     *
     * thread 被治理重写之后，文件里只剩快照之后的窗口，序号从中间开始（例如 5..24），
     * 轮数却是 20；按轮数取号会与保留下来的 `sequence == 20` 那一轮撞号，进而让
     * `trace:<session>:<sequence>` 的标识、按序号的轮次定位与 affect 序列的先后判定
     * 全部指向错的轮次。
     */
    let sequence = next_sequence(detail, None);
    service
        .append_turn_at_tail(
            root,
            AppendTurnCommand {
                thread_id: thread_id.to_owned(),
                recorded_at_ms: record.created_at_ms,
                turn: Turn { sequence, ..turn },
            },
        )
        .map(|()| Some(sequence))
}

/// 下一轮的轮序：除占位轮次之外的最大序号 + 1。
fn next_sequence(detail: &ThreadDetail, active_turn_id: Option<&str>) -> u64 {
    detail
        .turns
        .iter()
        .filter(|turn| Some(turn.turn_id.as_str()) != active_turn_id)
        .map(|turn| turn.sequence)
        .max()
        .map_or(0, |last| last.saturating_add(1))
}
