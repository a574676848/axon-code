//! 轮次事件收口判定与首字耗时计算：从事件写入器拆出的纯函数。

use axon_session_runtime::{AtomicTurnPerformanceRecorder, RuntimeEvent, TurnPerformanceStage};

/// 整轮终态事件：`turn_metrics` 只在这一帧上产出。
const TURN_COMPLETED: &str = "turn_completed";

pub(super) fn compute_application_ttft_ms(recorder: &AtomicTurnPerformanceRecorder) -> Option<u64> {
    let snapshot = recorder.snapshot();
    let start = snapshot
        .stages
        .iter()
        .find(|s| s.stage == TurnPerformanceStage::ProviderRequestStarted)?;
    let first = snapshot
        .stages
        .iter()
        .find(|s| s.stage == TurnPerformanceStage::ProviderFirstDeltaParsed)?;
    if first.elapsed_us >= start.elapsed_us {
        Some((first.elapsed_us - start.elapsed_us) / 1000)
    } else {
        None
    }
}

/// 这一帧是否代表本轮收口。
///
/// 只有 `turn_completed` 才算：一条 provider 流收口（`StreamingCompleted`）同样是
/// `status=completed` 的 `turnLifecycle` 帧，把它当轮收口会让 `turn_metrics` 在第一段
/// 就定稿，而每轮只写一条，同轮后续所有 provider 流的用量再也进不了指标，会话累计
/// 随之少记一个量级。
pub(super) fn is_turn_terminal(event: &RuntimeEvent) -> bool {
    event.event_type == TURN_COMPLETED
}
