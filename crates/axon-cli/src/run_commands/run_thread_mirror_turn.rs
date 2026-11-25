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
        /*
         * 起跑时就把本轮的轮序定下来。
         *
         * 序号一旦写进 thread 就是事实：换窗边界、`trace:<session>:<sequence>` 与按序号
         * 定位都直接引用它，不再依赖「本轮收口后会不会拿到这个号」。旧实现让占位轮恒为 0、
         * 收口时才补定，换窗因此只能预测一个尚未落地的未来序号；那轮一旦被中断或进程被杀，
         * 预测值就悬空，恢复时按它过滤会把整条历史清空，只剩上一个窗口的摘要。
         */
        let sequence = next_sequence(detail, None);
        return service
            .start_turn(
                root,
                StartTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms: record.created_at_ms,
                    turn: Turn { sequence, ..turn },
                },
            )
            .map(|()| None);
    }
    // 只认「本轮自己的」active 占位轮次：历史里可能躺着更早的残留轮次，按
    // `find(Active)` 取第一个会把本轮终态写到别人头上，那条残留随后永远 Active，
    // 同一会话的新 turn 于是被永久拒绝。
    if let Some(active) = detail
        .turns
        .iter()
        .find(|value| value.status == TurnStatus::Active && value.turn_id == turn.turn_id)
    {
        /*
         * 新快照从起跑起就带着真实轮序，收口直接沿用；只有旧快照里的占位轮（序号恒为 0
         * 且不是 thread 的第一轮）才需要在这里补定，否则会与 Turn 0 撞号，
         * `trace:<session>:<sequence>`、`card_id` 与「按序号定位轮次」随即全轮撞车。
         */
        let sequence = if active.sequence > 0 || detail.turns.len() <= 1 {
            active.sequence
        } else {
            next_sequence(detail, Some(active.turn_id.as_str()))
        };
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
///
/// 窗口轮转的边界（`compaction.through_sequence`）复用同一口径：起跑镜像写入的占位轮次
/// 序号恒为 0，只有本轮收口后才会拿到这里的值，因此边界不能取占位轮对象上的 sequence。
pub(super) fn next_sequence(detail: &ThreadDetail, active_turn_id: Option<&str>) -> u64 {
    detail
        .turns
        .iter()
        .filter(|turn| Some(turn.turn_id.as_str()) != active_turn_id)
        .map(|turn| turn.sequence)
        .max()
        .map_or(0, |last| last.saturating_add(1))
}
