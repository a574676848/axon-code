//! 子代理 Provider 前阶段遥测。

use std::sync::Arc;

use axon_session_runtime::{
    AtomicTurnPerformanceRecorder, TurnPerformanceRecorder, TurnPerformanceStage,
    first_token_telemetry_enabled,
};

const SUBAGENT_PERFORMANCE_LOG_PREFIX: &str = "subagent_first_token_telemetry=";

pub(super) fn start_subagent_performance() -> Option<Arc<dyn TurnPerformanceRecorder>> {
    first_token_telemetry_enabled().then(|| {
        let recorder: Arc<dyn TurnPerformanceRecorder> =
            Arc::new(AtomicTurnPerformanceRecorder::new());
        recorder.record(TurnPerformanceStage::WorkerClaimed);
        recorder
    })
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
