//! 轮次事件收口判定与首字耗时计算：从事件写入器拆出的纯函数。

use axon_session_runtime::{AtomicTurnPerformanceRecorder, RuntimeEvent, TurnPerformanceStage};
use serde_json::Value;

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
/// 只有 `Durable` 帧会走到这里（每轮几十条），因此可以再解析一次载荷；
/// 热路径上的正文增量与用量永远不经过本函数。
pub(super) fn is_turn_terminal(event: &RuntimeEvent) -> bool {
    const TURN_COMPLETED: &str = "turn_completed";
    if event.event_type == TURN_COMPLETED {
        return true;
    }
    let Ok(envelope) = serde_json::from_str::<Value>(&event.payload_json) else {
        return false;
    };
    let Some(kind) = envelope.get("kind") else {
        return false;
    };
    if kind.get("type").and_then(Value::as_str) != Some("turnLifecycle") {
        return false;
    }
    matches!(
        kind.get("payload")
            .and_then(|payload| payload.get("status"))
            .and_then(Value::as_str),
        Some("completed" | "failed" | "cancelled" | "aborted")
    )
}
