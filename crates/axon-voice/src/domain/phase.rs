//! 语音会话的 9 相位与 32 条合法迁移。
//!
//! 边集与 `ARCHITECTURE.md` §3 的表逐行对应；任何未列出的迁移都是协议错误，
//! 由 [`Phase::apply`] 返回 [`PhaseError::IllegalTransition`]。

/// 相位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// 待机，不占状态行。
    Idle,
    /// 正在准备：校验模型、打开设备。
    Arming,
    /// 正在采集。
    Listening,
    /// 已检测到端点，等待定稿。
    Endpointing,
    /// 已提交，等待模型与首包音频。
    Thinking,
    /// 正在播放。
    Speaking,
    /// 被打断。
    Interrupted,
    /// 失败，等待用户重试或关闭。
    Error,
    /// 会话结束。
    Closed,
}

impl Phase {
    /// 全部相位，按迁移表出现顺序排列。
    pub const ALL: [Self; 9] = [
        Self::Idle,
        Self::Arming,
        Self::Listening,
        Self::Endpointing,
        Self::Thinking,
        Self::Speaking,
        Self::Interrupted,
        Self::Error,
        Self::Closed,
    ];

    /// 是否为终态。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed)
    }

    /// 是否处于采集或播放的活动期。
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Arming | Self::Listening | Self::Endpointing | Self::Thinking | Self::Speaking
        )
    }

    /// 是否占用状态行。
    #[must_use]
    pub const fn occupies_status_row(self) -> bool {
        !matches!(self, Self::Idle | Self::Closed)
    }

    /// 按触发事件迁移。
    ///
    /// # Errors
    ///
    /// 当前相位不存在该触发对应的边时返回 [`PhaseError::IllegalTransition`]。
    pub fn apply(self, trigger: TransitionTrigger) -> Result<Self, PhaseError> {
        for (from, to, edge_trigger) in LEGAL_EDGES {
            if from == self && edge_trigger == trigger {
                return Ok(to);
            }
        }
        Err(PhaseError::IllegalTransition {
            from: self,
            trigger,
        })
    }

    /// 是否存在 `self → to` 的合法边。
    #[must_use]
    pub fn can_reach(self, to: Self) -> bool {
        LEGAL_EDGES
            .iter()
            .any(|(from, target, _)| *from == self && *target == to)
    }
}

/// 迁移触发。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransitionTrigger {
    /// 客户端发起会话。
    Start,
    /// 宿主预热完成且设备就绪。
    Warm,
    /// 准备阶段失败（模型缺失、模型不可用、设备不可用、宿主不可用）。
    ArmingFailed,
    /// 静音达阈值且已有语音，或推键松手且本句有语音。
    SpeechEnd,
    /// 推键松手且本句没有语音。
    StopWithoutSpeech,
    /// 用户主动取消。
    UserCancel,
    /// 识别过载或识别引擎崩溃。
    AsrFailure,
    /// 定稿并提交成功。
    FinalSubmitted,
    /// 定稿为空（噪声）。
    EmptyFinal,
    /// 服务端判定打断。
    Interrupt,
    /// 用户停止。
    Stop,
    /// 宿主或回合的不可恢复失败。
    WorkerFailure,
    /// 首个可朗读句子与首包音频。
    SpeakStarted,
    /// 回复为空，或本轮不需要播放。
    NoPlaybackNeeded,
    /// 提交被拒或回合失败。
    TurnFailure,
    /// 播放自然结束且收到排空回执。
    PlaybackDrained,
    /// 播放失败且没有内容可播。
    PlaybackFailure,
    /// 用户重试。
    Retry,
    /// 用户关闭入口。
    CloseEntry,
    /// 会话收口（客户端关闭、空闲超时、建连超时、不可恢复错误）。
    SessionClose,
}

/// 迁移错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseError {
    /// 当前相位没有该触发对应的边。
    IllegalTransition {
        /// 起始相位。
        from: Phase,
        /// 触发。
        trigger: TransitionTrigger,
    },
}

/// 32 条合法边，与 `ARCHITECTURE.md` §3 逐行对应。
pub const LEGAL_EDGES: [(Phase, Phase, TransitionTrigger); 32] = [
    (Phase::Idle, Phase::Arming, TransitionTrigger::Start),
    (Phase::Arming, Phase::Listening, TransitionTrigger::Warm),
    (Phase::Arming, Phase::Error, TransitionTrigger::ArmingFailed),
    (
        Phase::Listening,
        Phase::Endpointing,
        TransitionTrigger::SpeechEnd,
    ),
    (
        Phase::Listening,
        Phase::Idle,
        TransitionTrigger::StopWithoutSpeech,
    ),
    (
        Phase::Listening,
        Phase::Interrupted,
        TransitionTrigger::UserCancel,
    ),
    (
        Phase::Listening,
        Phase::Error,
        TransitionTrigger::AsrFailure,
    ),
    (
        Phase::Endpointing,
        Phase::Thinking,
        TransitionTrigger::FinalSubmitted,
    ),
    (
        Phase::Endpointing,
        Phase::Listening,
        TransitionTrigger::EmptyFinal,
    ),
    (
        Phase::Endpointing,
        Phase::Interrupted,
        TransitionTrigger::Interrupt,
    ),
    (Phase::Endpointing, Phase::Idle, TransitionTrigger::Stop),
    (
        Phase::Endpointing,
        Phase::Error,
        TransitionTrigger::WorkerFailure,
    ),
    (
        Phase::Thinking,
        Phase::Speaking,
        TransitionTrigger::SpeakStarted,
    ),
    (
        Phase::Thinking,
        Phase::Listening,
        TransitionTrigger::NoPlaybackNeeded,
    ),
    (
        Phase::Thinking,
        Phase::Interrupted,
        TransitionTrigger::Interrupt,
    ),
    (
        Phase::Thinking,
        Phase::Error,
        TransitionTrigger::TurnFailure,
    ),
    (
        Phase::Speaking,
        Phase::Listening,
        TransitionTrigger::PlaybackDrained,
    ),
    (
        Phase::Speaking,
        Phase::Interrupted,
        TransitionTrigger::Interrupt,
    ),
    (
        Phase::Speaking,
        Phase::Error,
        TransitionTrigger::PlaybackFailure,
    ),
    (
        Phase::Interrupted,
        Phase::Listening,
        TransitionTrigger::PlaybackDrained,
    ),
    (Phase::Interrupted, Phase::Idle, TransitionTrigger::Stop),
    (
        Phase::Interrupted,
        Phase::Error,
        TransitionTrigger::WorkerFailure,
    ),
    (Phase::Error, Phase::Arming, TransitionTrigger::Retry),
    (Phase::Error, Phase::Idle, TransitionTrigger::CloseEntry),
    (Phase::Idle, Phase::Closed, TransitionTrigger::SessionClose),
    (
        Phase::Arming,
        Phase::Closed,
        TransitionTrigger::SessionClose,
    ),
    (
        Phase::Listening,
        Phase::Closed,
        TransitionTrigger::SessionClose,
    ),
    (
        Phase::Endpointing,
        Phase::Closed,
        TransitionTrigger::SessionClose,
    ),
    (
        Phase::Thinking,
        Phase::Closed,
        TransitionTrigger::SessionClose,
    ),
    (
        Phase::Speaking,
        Phase::Closed,
        TransitionTrigger::SessionClose,
    ),
    (
        Phase::Interrupted,
        Phase::Closed,
        TransitionTrigger::SessionClose,
    ),
    (Phase::Error, Phase::Closed, TransitionTrigger::SessionClose),
];

#[cfg(test)]
mod tests {
    use super::{LEGAL_EDGES, Phase, PhaseError, TransitionTrigger};
    use std::collections::HashSet;

    const TRIGGERS: [TransitionTrigger; 20] = [
        TransitionTrigger::Start,
        TransitionTrigger::Warm,
        TransitionTrigger::ArmingFailed,
        TransitionTrigger::SpeechEnd,
        TransitionTrigger::StopWithoutSpeech,
        TransitionTrigger::UserCancel,
        TransitionTrigger::AsrFailure,
        TransitionTrigger::FinalSubmitted,
        TransitionTrigger::EmptyFinal,
        TransitionTrigger::Interrupt,
        TransitionTrigger::Stop,
        TransitionTrigger::WorkerFailure,
        TransitionTrigger::SpeakStarted,
        TransitionTrigger::NoPlaybackNeeded,
        TransitionTrigger::TurnFailure,
        TransitionTrigger::PlaybackDrained,
        TransitionTrigger::PlaybackFailure,
        TransitionTrigger::Retry,
        TransitionTrigger::CloseEntry,
        TransitionTrigger::SessionClose,
    ];

    #[test]
    fn edge_table_has_32_entries() {
        assert_eq!(LEGAL_EDGES.len(), 32);
    }

    #[test]
    fn from_and_trigger_are_unique() {
        let mut seen = HashSet::new();
        for (from, _, trigger) in LEGAL_EDGES {
            assert!(
                seen.insert((from, trigger)),
                "重复的 (from, trigger)：{from:?} {trigger:?}"
            );
        }
    }

    #[test]
    fn every_edge_applies_exactly_once() {
        for (from, to, trigger) in LEGAL_EDGES {
            assert_eq!(from.apply(trigger), Ok(to), "{from:?} + {trigger:?}");
        }
    }

    #[test]
    fn non_table_combinations_are_rejected_and_cover_at_least_30() {
        let table: HashSet<(Phase, TransitionTrigger)> =
            LEGAL_EDGES.iter().map(|(from, _, t)| (*from, *t)).collect();
        let mut illegal = 0;
        for phase in Phase::ALL {
            for trigger in TRIGGERS {
                if table.contains(&(phase, trigger)) {
                    continue;
                }
                illegal += 1;
                assert_eq!(
                    phase.apply(trigger),
                    Err(PhaseError::IllegalTransition {
                        from: phase,
                        trigger
                    }),
                    "{phase:?} + {trigger:?} 应被拒绝"
                );
            }
        }
        assert!(illegal >= 30, "非法组合只覆盖了 {illegal} 条");
    }

    #[test]
    fn every_phase_is_reachable_from_idle() {
        let mut reached = HashSet::from([Phase::Idle]);
        loop {
            let before = reached.len();
            for (from, to, _) in LEGAL_EDGES {
                if reached.contains(&from) {
                    reached.insert(to);
                }
            }
            if reached.len() == before {
                break;
            }
        }
        for phase in Phase::ALL {
            assert!(reached.contains(&phase), "{phase:?} 不可从 idle 到达");
        }
    }

    #[test]
    fn every_phase_can_reach_closed() {
        for phase in Phase::ALL {
            if phase.is_terminal() {
                continue;
            }
            assert!(
                phase.can_reach(Phase::Closed),
                "{phase:?} 没有收口到 closed 的边"
            );
        }
    }

    #[test]
    fn closed_is_terminal_and_never_reenters() {
        assert!(Phase::Closed.is_terminal());
        for trigger in TRIGGERS {
            assert!(
                Phase::Closed.apply(trigger).is_err(),
                "closed 不应接受 {trigger:?}"
            );
        }
    }

    #[test]
    fn status_row_occupancy_matches_ui_contract() {
        assert!(!Phase::Idle.occupies_status_row());
        assert!(!Phase::Closed.occupies_status_row());
        for phase in [
            Phase::Arming,
            Phase::Listening,
            Phase::Endpointing,
            Phase::Thinking,
            Phase::Speaking,
            Phase::Interrupted,
            Phase::Error,
        ] {
            assert!(phase.occupies_status_row(), "{phase:?} 应占状态行");
            assert!(!phase.is_terminal());
        }
    }
}
