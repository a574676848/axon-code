//! 通用 Daemon 与持久 Session Runtime 的纯领域模型。

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! text_id {
    ($name:ident, $label:literal) => {
        #[doc = $label]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// 从非空文本创建 ID。
            pub fn new(value: impl Into<String>) -> Result<Self, DaemonDomainError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(DaemonDomainError::EmptyId($label));
                }
                Ok(Self(value))
            }

            /// 返回 ID 文本。
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

text_id!(DaemonSessionId, "后台 Session 稳定 ID。");
text_id!(RunSessionId, "Agent rollout/transcript ID。");
text_id!(SessionActionId, "Session Action 幂等 ID。");
text_id!(RuntimeHostId, "Daemon Host 实例 ID。");

/// attach/replay 内部续读使用的 Host generation 和事件序号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerCursor {
    /// Host generation。
    pub generation: String,
    /// Session 内单调递增的事件序号。
    pub sequence: u64,
}

impl ConsumerCursor {
    /// 创建新的内部续读游标。
    #[must_use]
    pub fn new(generation: impl Into<String>, sequence: u64) -> Self {
        Self {
            generation: generation.into(),
            sequence,
        }
    }

    /// 判断当前游标是否严格晚于另一个持久游标，允许跨 Host generation 续读。
    ///
    /// Session 事件序号在持久化边界内单调递增，generation 只标识写入该事件的 Host。
    #[must_use]
    pub fn advances_durable_from(&self, previous: &Self) -> bool {
        self.sequence > previous.sequence
    }

    /// 判断当前游标是否是同一 generation 下的严格后继。
    #[must_use]
    pub fn advances_from(&self, previous: &Self) -> bool {
        self.generation == previous.generation && self.sequence > previous.sequence
    }
}

/// 持久运行时租约；所有写入都必须携带该代际。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLease {
    /// 所属 Host。
    pub owner: RuntimeHostId,
    /// claim 时递增的 fencing generation。
    pub generation: String,
    /// 同一 Host 内的尝试次数。
    pub attempt: u32,
    /// 过期时间戳。
    pub expires_at_ms: u64,
}

impl RuntimeLease {
    /// 判断租约是否仍在有效窗口内。
    #[must_use]
    pub const fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms < self.expires_at_ms
    }
}

/// Session Runtime 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRuntimeStatus {
    /// 已创建但尚未执行。
    Created,
    /// 当前没有 active action。
    Idle,
    /// 正在执行 action。
    Running,
    /// 正在压缩上下文。
    Compacting,
    /// 等待用户、审批或外部输入。
    Waiting,
    /// 正在从 checkpoint 恢复。
    Recovering,
    /// 已进入升级/关闭排空。
    Draining,
    /// 已完成。
    Completed,
    /// 已失败且等待人工处理。
    Failed,
}

impl SessionRuntimeStatus {
    /// 返回稳定存储值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Compacting => "compacting",
            Self::Waiting => "waiting",
            Self::Recovering => "recovering",
            Self::Draining => "draining",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// Action 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionActionStatus {
    /// 等待 pump 领取。
    Queued,
    /// 等待外部输入，不能被普通 Worker 领取。
    Waiting,
    /// 已由某个 Worker claim。
    Claimed,
    /// 已开始执行。
    Running,
    /// 已提交结果。
    Committed,
    /// 被显式取消。
    Cancelled,
    /// 超过重试策略，进入死信。
    DeadLetter,
}

impl SessionActionStatus {
    /// 返回稳定存储值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Waiting => "waiting",
            Self::Claimed => "claimed",
            Self::Running => "running",
            Self::Committed => "committed",
            Self::Cancelled => "cancelled",
            Self::DeadLetter => "dead_letter",
        }
    }

    /// 判断是否为终态。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::Cancelled | Self::DeadLetter)
    }
}

/// 调度策略；只定义语义，不解析具体 cron 方言。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SchedulePolicy {
    /// 只触发一次。
    Once {
        /// 触发时间戳（毫秒）。
        at_ms: u64,
    },
    /// 固定间隔触发。
    Interval {
        /// 触发间隔（毫秒）。
        every_ms: u64,
    },
    /// 外部 cron parser 负责解释表达式。
    Cron {
        /// 外部 cron 表达式。
        expression: String,
    },
}

impl SchedulePolicy {
    /// 计算下一次触发时间；没有下一次时返回 None。
    #[must_use]
    pub fn next_run_at_ms(&self, fired_at_ms: u64) -> Option<u64> {
        match self {
            Self::Once { .. } => None,
            Self::Interval { every_ms } if *every_ms > 0 => {
                Some(fired_at_ms.saturating_add(*every_ms))
            }
            Self::Interval { .. } | Self::Cron { .. } => None,
        }
    }
}

/// 领域层错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DaemonDomainError {
    /// ID 为空。
    #[error("{0} 不能为空")]
    EmptyId(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumer_cursor_requires_monotonic_sequence_within_generation() {
        let previous = ConsumerCursor::new("generation-a", 4);
        assert!(ConsumerCursor::new("generation-a", 5).advances_from(&previous));
        assert!(!ConsumerCursor::new("generation-a", 4).advances_from(&previous));
        assert!(!ConsumerCursor::new("generation-a", 3).advances_from(&previous));
        assert!(!ConsumerCursor::new("generation-b", 5).advances_from(&previous));
    }
}
