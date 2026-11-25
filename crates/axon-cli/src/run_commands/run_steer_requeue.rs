//! 收口回投：把「已结算却没被取走」的追加逐条交还 durable 入站表重新排队。
//!
//! 单独成文件：`run_steer` 已顶到单文件行数上限；这条路径只在非边界出口触发，
//! 但决定消息会不会丢，值得独立成一处可读的实现。

use axon_domain::InteractionId;

use super::{SteerRequeueHook, now_millis};

/// 逐条交还重投；失败只告警，绝不阻断收口。
///
/// 只在收口时调用，且不在任何锁内：SQLite 写事务的等待不得拖住队列本身。
pub(super) fn requeue_leftovers(hook: Option<&SteerRequeueHook>, leftovers: &[InteractionId]) {
    if leftovers.is_empty() {
        return;
    }
    let Some(hook) = hook else {
        axon_logger::warning(&format!(
            "主 turn 收口时仍有 {} 条已结算的追加未取走，且未配置重投落点：它们不会自动重跑（session 级 CLI 直跑没有 durable 入站表）",
            leftovers.len()
        ));
        return;
    };
    let now_ms = now_millis();
    let mut failed = 0_usize;
    for interaction_id in leftovers {
        if let Err(error) = hook(interaction_id, now_ms) {
            failed = failed.saturating_add(1);
            axon_logger::warning(&format!(
                "重投运行中追加失败：interaction={}，原因={error}",
                interaction_id.as_str()
            ));
        }
    }
    if failed > 0 {
        axon_logger::warning(&format!(
            "主 turn 收口时有 {} 条已结算追加未取走，其中 {failed} 条重投失败，需要人工重发",
            leftovers.len()
        ));
    }
}
