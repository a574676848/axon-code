//! 语音回合的 epoch 与 `turn` 帧：提交成功才入账，被拒绝不消耗轮次。
//!
//! 本模块只做纯状态与帧构造：提交、订阅与网络 IO 留在会话循环里。

use axon_voice::VoiceFailure;

/// 会话相位；只取与语音回合有关的两个值，其余相位由既有连线负责。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnPhase {
    /// 聆听。
    Listening,
    /// 已提交，等待助手增量。
    Thinking,
}

impl TurnPhase {
    /// `state` 帧里的相位字面量（PROTOCOL §2.3）。
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Thinking => "thinking",
        }
    }
}

/// 提交成功分配给本轮的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AssignedTurn {
    /// 会话内轮次，从 1 起且只在提交成功时自增。
    pub(super) epoch: u32,
    /// `"<interactionId>:<epoch>"`（PROTOCOL §2.1）。
    pub(super) turn_id: String,
}

/// 一路语音的回合编排。
/// 当前相位。
#[derive(Debug)]
pub(super) struct VoiceTurn {
    next_epoch: u32,
    active: Option<AssignedTurn>,
    phase: TurnPhase,
}

impl VoiceTurn {
    /// 新建。
    pub(super) fn new() -> Self {
        Self {
            next_epoch: 1,
            active: None,
            phase: TurnPhase::Listening,
        }
    }

    /// 当前相位。
    pub(super) fn phase(&self) -> TurnPhase {
        self.phase
    }

    /// 提交成功：分配 epoch 并进入思考。
    pub(super) fn submitted(&mut self, interaction_id: &str) -> AssignedTurn {
        let epoch = self.next_epoch;
        self.next_epoch = self.next_epoch.saturating_add(1);
        let assigned = AssignedTurn {
            epoch,
            turn_id: format!("{interaction_id}:{epoch}"),
        };
        self.active = Some(assigned.clone());
        self.phase = TurnPhase::Thinking;
        assigned
    }

    /// 提交失败：不消耗 epoch，会话保持。
    pub(super) fn rejected(&mut self) {
        if self.active.is_none() {
            self.phase = TurnPhase::Listening;
        }
    }
}

/// `final` 帧：定稿转写先给眼睛，再进入提交。
pub(super) fn final_json(
    utterance_seq: u32,
    text: &str,
    start_ms: u64,
    end_ms: u64,
    finalize_ms: u64,
) -> String {
    serde_json::json!({
        "type": "final",
        "utteranceSeq": utterance_seq,
        "text": text,
        "startMs": start_ms,
        "endMs": end_ms,
        "finalizeMs": finalize_ms,
    })
    .to_string()
}

/// `turn` 帧。
pub(super) fn turn_json(
    interaction_id: &str,
    assigned: &AssignedTurn,
    lane_id: &str,
    session_id: &str,
) -> String {
    serde_json::json!({
        "type": "turn",
        "interactionId": interaction_id,
        "turnId": assigned.turn_id,
        "laneId": lane_id,
        "sessionId": session_id,
        "epoch": assigned.epoch,
    })
    .to_string()
}

/// `submit_rejected` 错误帧：阶段固定 `thinking`，会话保持。
pub(super) fn submit_rejected_json(reason: &str) -> String {
    let failure = VoiceFailure::TurnRejected {
        code: reason.to_owned(),
    };
    serde_json::json!({
        "type": "error",
        "code": failure.code(),
        "stage": "thinking",
        "retryable": failure.retryable(),
        "message": failure.user_message(),
    })
    .to_string()
}
