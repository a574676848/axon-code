//! 恢复前的轮次修复：给崩溃/中断留下的未定序轮次补号并收口。

use std::collections::HashSet;
use std::path::Path;

use axon_session::{
    AppendTurnCommand, JsonlThreadEventStore, ThreadDetail, ThreadService, TurnStatus,
};

use super::mirror_turn;

/// 给崩溃/中断留下的未定序轮次补号，并把这些轮折叠成终态。
///
/// 起跑镜像现在一落盘就带真实轮序，但历史里仍有旧写入留下的 `sequence == 0` 残留
/// （首轮之外的第二条及以后）。它们不参与 `next_sequence` 的 max 计算，换窗边界就会
/// 一直比真实轮次差一拍：边界写成 max+1，恢复时越过全部轮次，触发越界回退并把上一轮
/// 整段重放，估算随即被这段重复内容推高，于是又换一次窗（真机现场：边界 10、最大轮序 9，
/// 两次归档条数 132 与 134 几乎相同）。
///
/// 这里按出现顺序补号，并把仍在 Active 的轮收成 Interrupted。失败只报告，不阻断恢复：
/// 补号是为了让边界对齐，不是恢复的必要条件。
pub(super) fn number_unnumbered_turns(
    service: &ThreadService<JsonlThreadEventStore>,
    root: &Path,
    thread_id: &str,
    detail: &ThreadDetail,
) -> Result<usize, String> {
    if detail.turns.len() <= 1 {
        return Ok(0);
    }
    let Some(first_zero) = detail.turns.iter().position(|turn| turn.sequence == 0) else {
        return Ok(0);
    };
    let mut seen = HashSet::new();
    let mut next = mirror_turn::next_sequence(detail, None);
    let mut numbered = 0_usize;
    for turn in detail.turns.iter().skip(first_zero.saturating_add(1)) {
        if turn.sequence != 0 || !seen.insert(turn.turn_id.clone()) {
            continue;
        }
        let mut replacement = turn.clone();
        replacement.sequence = next;
        if replacement.status == TurnStatus::Active {
            replacement.status = TurnStatus::Interrupted;
            replacement.interrupted_at_ms = Some(axon_time::unix_now_ms());
        }
        let recorded_at_ms = replacement
            .interrupted_at_ms
            .or(replacement.completed_at_ms)
            .unwrap_or(replacement.started_at_ms);
        service
            .append_turn_at_tail(
                root,
                AppendTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms,
                    turn: replacement,
                },
            )
            .map_err(|error| format!("未定序轮次补号失败：{error}"))?;
        next = next.saturating_add(1);
        numbered = numbered.saturating_add(1);
    }
    Ok(numbered)
}
