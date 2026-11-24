//! 模型历史快照：Agent 到 Provider 瞬时链路的共享只读视图。
//!
//! 快照以 `Arc<Vec<...>>` 承载消息，只读规划层 clone 仅增加引用计数；
//! 需要追加或替换消息时通过 copy-on-write 写入，不改变持久化消息合同。

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use super::AgentModelMessage;

/// Agent 到 Provider 瞬时链路共享的模型历史快照。
///
/// 只读规划层 clone 仅增加引用计数；需要追加或替换消息时通过
/// [`Self::make_mut`] 执行 copy-on-write，不改变持久化消息合同。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentModelHistorySnapshot {
    messages: Arc<Vec<AgentModelMessage>>,
    /// 产生该历史的记忆窗口 ID；用于渲染 provider 可见的稳定 item id 标记。
    history_window_id: Option<String>,
}

impl AgentModelHistorySnapshot {
    /// 返回只读消息切片。
    #[must_use]
    pub fn as_slice(&self) -> &[AgentModelMessage] {
        self.messages.as_slice()
    }

    /// 返回产生该历史的记忆窗口 ID。
    #[must_use]
    pub fn history_window_id(&self) -> Option<&str> {
        self.history_window_id.as_deref()
    }

    /// 绑定记忆窗口 ID；空白值视为未绑定。
    #[must_use]
    pub fn with_history_window_id(mut self, window_id: Option<String>) -> Self {
        self.history_window_id = window_id.filter(|value| !value.trim().is_empty());
        self
    }

    /// 返回最近 N 个真实用户 turn 的起始消息下标。
    #[must_use]
    pub fn first_index_for_last_user_turns(&self, turns: usize) -> usize {
        if turns == 0 {
            return self.messages.len();
        }
        let total_turns = self
            .messages
            .iter()
            .filter(|message| message.is_real_user_turn())
            .count();
        if turns >= total_turns {
            return 0;
        }
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.is_real_user_turn())
            .nth(total_turns - turns)
            .map_or(0, |(index, _)| index)
    }

    /// 删除最近 N 个真实用户 turn；没有可删除 turn 时保持共享存储不变。
    pub fn drop_last_n_user_turns(&mut self, turns: usize) -> bool {
        if turns == 0 {
            return false;
        }
        let total_turns = self
            .messages
            .iter()
            .filter(|message| message.is_real_user_turn())
            .count();
        if total_turns == 0 {
            return false;
        }
        let retained_turns = total_turns.saturating_sub(turns);
        let cut_index = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.is_real_user_turn())
            .nth(retained_turns)
            .map_or(self.messages.len(), |(index, _)| index);
        self.make_mut().truncate(cut_index);
        true
    }

    /// 获取可变消息向量；存在其他快照时才复制底层历史。
    pub fn make_mut(&mut self) -> &mut Vec<AgentModelMessage> {
        Arc::make_mut(&mut self.messages)
    }

    /// 消费快照并回收消息向量；存在其他引用时才执行深拷贝。
    #[must_use]
    pub fn into_messages(self) -> Vec<AgentModelMessage> {
        Arc::unwrap_or_clone(self.messages)
    }

    /// 判断两个快照是否共享同一份底层历史。
    #[must_use]
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.messages, &other.messages)
    }
}

impl From<Vec<AgentModelMessage>> for AgentModelHistorySnapshot {
    fn from(messages: Vec<AgentModelMessage>) -> Self {
        Self {
            messages: Arc::new(messages),
            history_window_id: None,
        }
    }
}

impl Deref for AgentModelHistorySnapshot {
    type Target = Vec<AgentModelMessage>;

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

impl DerefMut for AgentModelHistorySnapshot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.make_mut()
    }
}
