//! 子代理 Provider 前阶段遥测。

use std::sync::Arc;

use axon_session_runtime::{
    AtomicTurnPerformanceRecorder, TurnPerformanceRecorder, TurnPerformanceStage,
};

const SUBAGENT_PERFORMANCE_LOG_PREFIX: &str = "subagent_first_token_telemetry=";

/// 子代理的轮次性能采集：无条件开启。
///
/// 首字与速率是产品指标，不是诊断开关：受 trace 开关控制会让 `/turn_metrics` 里
/// 这两项恒为空，轮次分割线因此读不出首字与速率。trace 开关只决定是否写逐阶段明细日志。
pub(super) fn start_subagent_performance() -> Option<Arc<dyn TurnPerformanceRecorder>> {
    let recorder: Arc<dyn TurnPerformanceRecorder> = Arc::new(AtomicTurnPerformanceRecorder::new());
    recorder.record(TurnPerformanceStage::WorkerClaimed);
    Some(recorder)
}

pub(super) fn record_subagent_history_ready(performance: Option<&dyn TurnPerformanceRecorder>) {
    if let Some(performance) = performance {
        performance.record(TurnPerformanceStage::HistoryReady);
    }
}

pub(super) fn log_subagent_performance(performance: Option<&dyn TurnPerformanceRecorder>) {
    let Some(performance) = performance else {
        return;
    };
    match serde_json::to_string(&performance.snapshot()) {
        Ok(snapshot) => axon_logger::debug(&format!("{SUBAGENT_PERFORMANCE_LOG_PREFIX}{snapshot}")),
        Err(error) => axon_logger::debug(&format!("子代理首字遥测序列化失败: {error}")),
    }
}
