//! 容量快照：并行宽度的唯一事实源（PWC-1 `I-5`）。
//!
//! 数值来自 `limits.yaml` 的内部预设，不进入用户配置；提示词渲染、准入判定与
//! 遥测都读同一份快照，禁止各自计算或硬编码。

/// 单批工具调用并发宽度（`limits.yaml` 的 `scheduler.call_width`）。
const PRESET_CALL_WIDTH: usize = 8;
/// 扇出单元宽度预设（与 `scheduler.call_width` 同值，预留）。
const PRESET_UNIT_WIDTH: usize = 8;
/// 子代理线程上限（`agent.max_subagent_threads.preset`）。
const PRESET_THREAD_LIMIT: usize = 16;
/// 子代理活跃 turn 上限（`agent.max_subagent_active_turns.preset`）。
const PRESET_ACTIVE_TURN_LIMIT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CapacitySnapshot {
    pub(super) call_width: usize,
    pub(super) unit_width: usize,
    pub(super) thread_limit: usize,
    pub(super) active_turn_limit: usize,
}

impl CapacitySnapshot {
    pub(super) const fn preset() -> Self {
        Self {
            call_width: PRESET_CALL_WIDTH,
            unit_width: PRESET_UNIT_WIDTH,
            thread_limit: PRESET_THREAD_LIMIT,
            active_turn_limit: PRESET_ACTIVE_TURN_LIMIT,
        }
    }
}

impl Default for CapacitySnapshot {
    fn default() -> Self {
        Self::preset()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_capacity_stays_inside_declared_bounds() {
        let capacity = CapacitySnapshot::preset();
        assert!((1..=32).contains(&capacity.call_width));
        assert!((1..=16).contains(&capacity.unit_width));
        assert!((1..=64).contains(&capacity.thread_limit));
        assert!((1..=64).contains(&capacity.active_turn_limit));
        assert_eq!(capacity, CapacitySnapshot::default());
    }
}
