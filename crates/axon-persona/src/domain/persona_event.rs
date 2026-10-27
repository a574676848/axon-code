//! 人格状态机的输入事件。

use serde::{Deserialize, Serialize};

use crate::domain::persona_state::UserAffect;

/// 本轮终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnFinalStatus {
    /// 正常完成。
    Completed,
    /// 执行失败。
    Failed,
    /// 被用户中断。
    Interrupted,
}

impl TurnFinalStatus {
    /// 是否属于失败或中断。
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Interrupted)
    }
}

/// 轮后结算输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSettledInput {
    /// 本轮终态。
    pub final_status: TurnFinalStatus,
    /// 评估器推断出的用户状态。
    pub user_affect: UserAffect,
    /// 评估器推断出的用户疲劳度。
    pub fatigue_level: crate::domain::persona_state::FatigueLevel,
    /// 助手回复相对人格底色的漂移程度，单位 basis point。
    pub persona_drift_bp: u16,
    /// 漂移原因；达到阈值时必填。
    pub drift_reason: Option<String>,
    /// 结算时刻，Unix 毫秒。
    pub now_ms: u64,
}

/// 人格状态机的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersonaEvent {
    /// 一轮结束后的结算。
    TurnSettled(TurnSettledInput),
    /// 用户手动重置状态；原因不得为空。
    Reset {
        /// 重置原因，写入转移日志。
        reason: String,
        /// 重置时刻，Unix 毫秒。
        now_ms: u64,
    },
}
