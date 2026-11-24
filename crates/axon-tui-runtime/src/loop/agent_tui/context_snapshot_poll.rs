//! 会话级上下文快照的节流刷新。
//!
//! 快照是头部占用段与剩余段的唯一来源，但读它要过一遍运行态存储：这里按固定间隔节流，
//! 既不让每一帧都去查库，也让新窗口第一次实测上报后头部能在一个刷新周期内跟上。

use std::time::{Duration, Instant};

use super::runtime_contract::AgentTuiRuntimeAdapter;
use super::state::AgentTuiState;

/// 快照刷新间隔。
pub(super) const CONTEXT_SNAPSHOT_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// 快照刷新节奏持有者；节流状态属于主循环，不进状态机。
pub(super) struct ContextSnapshotPoller {
    last_refresh_at: Option<Instant>,
}

impl ContextSnapshotPoller {
    pub(super) const fn new() -> Self {
        Self {
            last_refresh_at: None,
        }
    }

    /// 到点就读一次快照；读数变化时返回 `true`，调用方据此请求重绘。
    pub(super) fn poll<F>(&mut self, adapter: &F, state: &mut AgentTuiState) -> bool
    where
        F: AgentTuiRuntimeAdapter,
    {
        let now = Instant::now();
        if self.last_refresh_at.is_some_and(|last| {
            now.saturating_duration_since(last) < CONTEXT_SNAPSHOT_REFRESH_INTERVAL
        }) {
            return false;
        }
        self.last_refresh_at = Some(now);
        let session_id = state.context_snapshot_session_id().map(str::to_owned);
        let Some(snapshot) = adapter.context_snapshot(session_id.as_deref()) else {
            return false;
        };
        state.apply_context_snapshot(snapshot)
    }
}
