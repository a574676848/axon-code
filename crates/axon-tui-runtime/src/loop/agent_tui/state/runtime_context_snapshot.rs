//! 会话级上下文快照的读入与刷新。
//!
//! TUI 不自己按用量帧累加上下文占用：占用与换窗读数都来自服务端会话级快照，这里是
//! 唯一的落点。刷新由主循环按节流节奏触发，宿主的读取端口由 `AgentTuiRuntimeAdapter`
//! 提供；宿主没有读取能力时快照保持未知，界面显示未知。

use crate::app_shell::context_snapshot::AgentTuiContextSnapshot;

use super::AgentTuiState;

impl AgentTuiState {
    /// 当前生效的会话标识：优先会话摘要，其次启动时的显式会话。
    pub(in crate::r#loop::agent_tui) fn context_snapshot_session_id(&self) -> Option<&str> {
        self.header.session_summary.session_id.as_deref().or(self
            .header
            .overrides
            .session
            .as_deref())
    }

    /// 落一份新读到的快照；读数变化时返回 `true`，调用方据此请求重绘。
    ///
    /// 未知同样是一次有效更新：帧里没有读数时快照就是未知，不能因为「新的等于空」就
    /// 保留旧值。
    pub(in crate::r#loop::agent_tui) fn apply_context_snapshot(
        &mut self,
        snapshot: AgentTuiContextSnapshot,
    ) -> bool {
        if self.header.context_snapshot == snapshot {
            return false;
        }
        self.header.context_snapshot = snapshot;
        true
    }

    /// 当前快照读数。
    pub(in crate::r#loop::agent_tui) fn context_snapshot(&self) -> &AgentTuiContextSnapshot {
        &self.header.context_snapshot
    }
}
