//! Primary turn 首字链路的低开销阶段遥测。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

static FIRST_TOKEN_TELEMETRY_ENABLED: AtomicBool = AtomicBool::new(false);
const UNRECORDED_STAGE: u64 = 0;
const STORED_OFFSET: u64 = 1;

/// Primary turn 首字链路阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum TurnPerformanceStage {
    /// Worker 已取得有效 claim。
    WorkerClaimed,
    /// 配置已加载。
    ConfigReady,
    /// 启动快照已构建。
    StartupReady,
    /// Provider history 已恢复。
    HistoryReady,
    /// Workspace 与启动快照已就绪。
    WorkspaceReady,
    /// Memory query embedding 已生成或已同步降级。
    MemoryEmbeddingReady,
    /// Memory recall 已完成或降级。
    MemoryReady,
    /// Skill 与 Soul 已就绪。
    SkillSoulReady,
    /// Ledger 上下文已就绪。
    LedgerReady,
    /// Provider payload 已组装。
    PayloadReady,
    /// 统一 RunRequest 已完成组装。
    RunRequestBuilt,
    /// Provider turn 的 Thread 起始 mirror 已完成。
    ThreadStartMirrorReady,
    /// Provider turn 的 Thread 起始事实已持久化。
    RunStartedPersisted,
    /// Provider-specific 输入与 Prompt Cache 计划已就绪。
    ProviderInputReady,
    /// 单渠道 transport、压缩与 steer 运行状态已就绪。
    ProviderRuntimeReady,
    /// StreamingStarted 状态已投影。
    StreamingStarted,
    /// Provider 网络请求即将发出。
    ProviderRequestStarted,
    /// 首个有效 Provider 输出已解析。
    ProviderFirstDeltaParsed,
    /// 首个可见事件已持久提交。
    FirstEventCommitted,
    /// 首个可见事件已发布给订阅者。
    FirstEventPublished,
}

impl TurnPerformanceStage {
    /// 有序阶段全集。
    pub const ALL: [Self; 20] = [
        Self::WorkerClaimed,
        Self::ConfigReady,
        Self::StartupReady,
        Self::HistoryReady,
        Self::WorkspaceReady,
        Self::MemoryEmbeddingReady,
        Self::MemoryReady,
        Self::SkillSoulReady,
        Self::LedgerReady,
        Self::PayloadReady,
        Self::RunRequestBuilt,
        Self::ThreadStartMirrorReady,
        Self::RunStartedPersisted,
        Self::ProviderInputReady,
        Self::ProviderRuntimeReady,
        Self::StreamingStarted,
        Self::ProviderRequestStarted,
        Self::ProviderFirstDeltaParsed,
        Self::FirstEventCommitted,
        Self::FirstEventPublished,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

/// 单个阶段相对 Worker claim 的耗时。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPerformanceStageTiming {
    /// 阶段。
    pub stage: TurnPerformanceStage,
    /// 相对 recorder 创建时刻的单调时钟微秒数。
    pub elapsed_us: u64,
}

/// 单个 Primary turn 的安全遥测快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPerformanceSnapshot {
    /// 已记录的固定阶段；缺失阶段不会被伪造。
    pub stages: Vec<TurnPerformanceStageTiming>,
    /// Workspace remote Git 子进程次数。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub workspace_git_remote_probe_count: u64,
    /// Workspace status Git 子进程次数。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub workspace_git_status_probe_count: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// 首字阶段记录端口。
pub trait TurnPerformanceRecorder: Send + Sync {
    /// 只记录阶段的第一次到达。
    fn record(&self, stage: TurnPerformanceStage);

    /// 累加本 turn 的 Workspace Git 子进程次数。
    fn record_workspace_git_probes(&self, remote_count: u64, status_count: u64);

    /// 判断阶段是否已经到达。
    fn has_recorded(&self, stage: TurnPerformanceStage) -> bool;

    /// 读取当前有界遥测快照。
    fn snapshot(&self) -> TurnPerformanceSnapshot;
}

/// 固定槽位、每 turn 独占的阶段 recorder。
#[derive(Debug)]
pub struct AtomicTurnPerformanceRecorder {
    started_at: Instant,
    stages: [AtomicU64; TurnPerformanceStage::ALL.len()],
    workspace_git_remote_probe_count: AtomicU64,
    workspace_git_status_probe_count: AtomicU64,
}

impl AtomicTurnPerformanceRecorder {
    /// 创建启用的 recorder。
    #[must_use]
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            stages: std::array::from_fn(|_| AtomicU64::new(UNRECORDED_STAGE)),
            workspace_git_remote_probe_count: AtomicU64::new(0),
            workspace_git_status_probe_count: AtomicU64::new(0),
        }
    }

    /// 生成固定上限快照；排序与分配只发生在 turn 收口阶段。
    #[must_use]
    pub fn snapshot(&self) -> TurnPerformanceSnapshot {
        let stages = TurnPerformanceStage::ALL
            .into_iter()
            .filter_map(|stage| {
                let stored = self.stages[stage.index()].load(Ordering::Relaxed);
                (stored != UNRECORDED_STAGE).then_some(TurnPerformanceStageTiming {
                    stage,
                    elapsed_us: stored.saturating_sub(STORED_OFFSET),
                })
            })
            .collect();
        TurnPerformanceSnapshot {
            stages,
            workspace_git_remote_probe_count: self
                .workspace_git_remote_probe_count
                .load(Ordering::Relaxed),
            workspace_git_status_probe_count: self
                .workspace_git_status_probe_count
                .load(Ordering::Relaxed),
        }
    }
}

impl Default for AtomicTurnPerformanceRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnPerformanceRecorder for AtomicTurnPerformanceRecorder {
    fn record(&self, stage: TurnPerformanceStage) {
        let elapsed_us = u64::try_from(self.started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
        let stored = elapsed_us.saturating_add(STORED_OFFSET);
        let _ = self.stages[stage.index()].compare_exchange(
            UNRECORDED_STAGE,
            stored,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    fn record_workspace_git_probes(&self, remote_count: u64, status_count: u64) {
        self.workspace_git_remote_probe_count
            .fetch_add(remote_count, Ordering::Relaxed);
        self.workspace_git_status_probe_count
            .fetch_add(status_count, Ordering::Relaxed);
    }

    fn has_recorded(&self, stage: TurnPerformanceStage) -> bool {
        self.stages[stage.index()].load(Ordering::Relaxed) != UNRECORDED_STAGE
    }

    fn snapshot(&self) -> TurnPerformanceSnapshot {
        AtomicTurnPerformanceRecorder::snapshot(self)
    }
}

/// 在配置初始化边界设置进程级首字遥测策略。
pub fn configure_first_token_telemetry(enabled: bool) {
    FIRST_TOKEN_TELEMETRY_ENABLED.store(enabled, Ordering::Relaxed);
}

/// 返回进程级首字遥测策略；热路径只执行一次原子读取。
#[must_use]
pub fn first_token_telemetry_enabled() -> bool {
    FIRST_TOKEN_TELEMETRY_ENABLED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_is_recorded_only_once() {
        let recorder = AtomicTurnPerformanceRecorder::new();
        recorder.record(TurnPerformanceStage::WorkerClaimed);
        let first = recorder.snapshot();
        recorder.record(TurnPerformanceStage::WorkerClaimed);

        assert_eq!(recorder.snapshot(), first);
    }

    #[test]
    fn missing_stages_are_not_synthesized() {
        let recorder = AtomicTurnPerformanceRecorder::new();
        recorder.record(TurnPerformanceStage::PayloadReady);

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.stages.len(), 1);
        assert_eq!(snapshot.stages[0].stage, TurnPerformanceStage::PayloadReady);
    }

    #[test]
    fn snapshot_contains_only_stage_and_monotonic_duration() {
        let recorder = AtomicTurnPerformanceRecorder::new();
        recorder.record(TurnPerformanceStage::WorkerClaimed);
        recorder.record(TurnPerformanceStage::ProviderRequestStarted);

        let json = serde_json::to_string(&recorder.snapshot()).expect("快照应可序列化");
        assert!(json.contains("worker_claimed"));
        assert!(json.contains("provider_request_started"));
        assert!(!json.contains("prompt"));
        assert!(!json.contains("path"));
        assert!(!json.contains("key"));
        assert!(
            recorder
                .snapshot()
                .stages
                .windows(2)
                .all(|stages| stages[0].elapsed_us <= stages[1].elapsed_us)
        );
    }

    #[test]
    fn workspace_git_probe_counts_are_bounded_and_omitted_when_empty() {
        let recorder = AtomicTurnPerformanceRecorder::new();
        let empty = serde_json::to_value(recorder.snapshot()).expect("空快照应可序列化");
        assert!(empty.get("workspaceGitRemoteProbeCount").is_none());
        assert!(empty.get("workspaceGitStatusProbeCount").is_none());

        recorder.record_workspace_git_probes(1, 1);
        let snapshot = recorder.snapshot();

        assert_eq!(snapshot.workspace_git_remote_probe_count, 1);
        assert_eq!(snapshot.workspace_git_status_probe_count, 1);
    }
}
