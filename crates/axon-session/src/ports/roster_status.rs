//! 名册状态的派生口径。
//!
//! 名册只回答一个问题：这条子代理还占不占着车道。事实源是运行态的轮次头
//! （[`super::InteractionQueryStore::interaction_turn_heads`]），不是 JSONL 索引缓存
//! 里的末轮状态：索引里的 `active` 在子会话收口后仍然会停留，据此判「一直在跑」
//! 是退不出来的。索引侧的末轮状态只保留为对账用的旁证，不参与这里任何一支判断。

use super::InteractionTurnHead;
use axon_domain::InteractionStatus;

/// 名册对外的运行态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RosterStatus {
    /// 最新一轮已接收或正在处理：这一行仍占着车道。
    Active,
    /// 没有任何在途轮次。
    #[default]
    Idle,
}

impl RosterStatus {
    /// 对外取值：界面只认这两个 token。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
        }
    }
}

/// 名册对外的末轮状态：与轮次头的终态一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterLastTurnStatus {
    /// 末轮仍在跑。
    Active,
    /// 末轮正常收口。
    Completed,
    /// 末轮被中断。
    Interrupted,
    /// 末轮失败。
    Failed,
}

impl RosterLastTurnStatus {
    /// 对外取值：界面用它做「名册说收口、行里还说在跑」时的纠正结论。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

/// 名册一行由轮次头派生出来的运行态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RosterTurnStatus {
    /// 这一行还占不占着车道。
    pub status: RosterStatus,
    /// 末轮状态；这条会话一轮都没有时为空，界面读作「不可回看」。
    pub last_turn_status: Option<RosterLastTurnStatus>,
}

impl RosterTurnStatus {
    /// 由轮次头派生名册运行态。
    ///
    /// 只看最新一条：同一会话的在途轮次是串行的（领取按会话排他），所以最新一条就是
    /// 「这一行现在在干什么」。一条都没有即没有可执行的轮次，读作空闲，绝不因为历史
    /// 索引里写着 `active` 就当成还在跑。
    #[must_use]
    pub fn from_turn_heads(heads: &[InteractionTurnHead]) -> Self {
        let Some(latest) = heads.iter().max_by(|left, right| {
            left.received_at_ms
                .cmp(&right.received_at_ms)
                .then_with(|| left.interaction_id.cmp(&right.interaction_id))
        }) else {
            return Self::default();
        };
        match latest.status {
            InteractionStatus::Received | InteractionStatus::Processing => Self {
                status: RosterStatus::Active,
                last_turn_status: Some(RosterLastTurnStatus::Active),
            },
            InteractionStatus::Understood => Self {
                status: RosterStatus::Idle,
                last_turn_status: Some(RosterLastTurnStatus::Completed),
            },
            InteractionStatus::Rejected => Self {
                status: RosterStatus::Idle,
                last_turn_status: Some(RosterLastTurnStatus::Failed),
            },
            InteractionStatus::Interrupted => Self {
                status: RosterStatus::Idle,
                last_turn_status: Some(RosterLastTurnStatus::Interrupted),
            },
        }
    }
}
