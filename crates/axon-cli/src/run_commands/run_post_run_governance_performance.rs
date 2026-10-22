//! Post-run governance 请求准备阶段遥测。

use std::sync::Arc;

use axon_session_runtime::{
    AtomicTurnPerformanceRecorder, TurnPerformanceRecorder, TurnPerformanceStage,
    first_token_telemetry_enabled,
};

const GOVERNANCE_PERFORMANCE_LOG_PREFIX: &str = "governance_first_token_telemetry=";

pub(super) fn start_governance_performance() -> Option<Arc<dyn TurnPerformanceRecorder>> {
    first_token_telemetry_enabled().then(|| {
        let recorder: Arc<dyn TurnPerformanceRecorder> =
            Arc::new(AtomicTurnPerformanceRecorder::new());
        recorder.record(TurnPerformanceStage::WorkerClaimed);
        recorder
    })
}

pub(super) fn record_governance_config_ready(performance: Option<&dyn TurnPerformanceRecorder>) {
    record_stage(performance, TurnPerformanceStage::ConfigReady);
}

pub(super) fn record_governance_startup_ready(performance: Option<&dyn TurnPerformanceRecorder>) {
    record_stage(performance, TurnPerformanceStage::StartupReady);
}

pub(super) fn log_governance_performance(performance: Option<&dyn TurnPerformanceRecorder>) {
    let Some(performance) = performance else {
        return;
    };
    match serde_json::to_string(&performance.snapshot()) {
        Ok(snapshot) => {
            axon_logger::debug(&format!("{GOVERNANCE_PERFORMANCE_LOG_PREFIX}{snapshot}"))
        }
        Err(error) => axon_logger::debug(&format!("治理首字遥测序列化失败: {error}")),
    }
}

fn record_stage(performance: Option<&dyn TurnPerformanceRecorder>, stage: TurnPerformanceStage) {
    if let Some(performance) = performance {
        performance.record(stage);
    }
}
