//! 语音回合的编排状态：epoch 分配、朗读句子、停播与回执迁移。
//!
//! 本模块只做纯状态与帧构造：提交、订阅与网络 IO 留在会话循环里，因此
//! 「提交成功才分配 epoch」「`submit_rejected` 不消耗 epoch」「停播后必须等
//! `playbackDrained` 才回到聆听」都能用无 IO 的单测钉住。

use axon_voice::render::{SpeechStream, render_sentences};
use axon_voice::{Verbosity, VoiceFailure};
use std::time::{Duration, Instant};

/// 一句待下发的朗读稿，字段与宿主管道 `speak` 一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpeakFrame {
    /// 回合标识。
    pub(super) turn_id: String,
    /// 轮次。
    pub(super) epoch: u32,
    /// 句序号，每轮从 1 起。
    pub(super) seq: u32,
    /// 已渲染的朗读稿。
    pub(super) text: String,
}

/// 会话相位；只取与语音回合有关的三个值，其余相位由既有连线负责。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnPhase {
    /// 聆听。
    Listening,
    /// 已提交，等待助手增量。
    Thinking,
    /// 正在朗读。
    Speaking,
}

impl TurnPhase {
    /// `state` 帧里的相位字面量（PROTOCOL §2.3）。
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
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

/// 停播指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StopAudio {
    /// 作用于哪一轮。
    pub(super) epoch: u32,
    /// `barge` / `user_cancel` / `turn_end`。
    pub(super) reason: &'static str,
}

/// 当前轮。
#[derive(Debug)]
struct ActiveTurn {
    epoch: u32,
    turn_id: String,
    /// 已下发过 `stopAudio`：不再接受新的朗读与音频。
    stopped: bool,
    seq: u32,
}

/// 等待客户端 `playbackDrained` 的已停播轮次。
#[derive(Debug)]
struct AwaitingDrain {
    epoch: u32,
    deadline: Instant,
}

/// `playbackDrained` 的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DrainedOutcome {
    /// 回执命中当前停播轮次，已迁移回聆听。
    Accepted,
    /// 回执属于旧轮次，丢弃。
    Stale,
    /// 当前没有在等回执。
    Idle,
}

/// 一路语音的回合编排。
#[derive(Debug)]
pub(super) struct VoiceTurn {
    drain_timeout: Duration,
    next_epoch: u32,
    active: Option<ActiveTurn>,
    awaiting: Option<AwaitingDrain>,
    phase: TurnPhase,
    render: SpeechStream,
    spoken_any: bool,
    /// 本会话是否朗读助手回复（`start.tts`，只影响表达不影响提交）。
    read_aloud: bool,
}

impl VoiceTurn {
    /// 新建；`drain_timeout` 是停播到回执的上限（生产 10 s，测试可注入更短值）。
    pub(super) fn new(drain_timeout: Duration) -> Self {
        Self {
            drain_timeout,
            next_epoch: 1,
            active: None,
            awaiting: None,
            phase: TurnPhase::Listening,
            render: SpeechStream::new(),
            spoken_any: false,
            read_aloud: true,
        }
    }

    /// 设置本会话是否朗读；`start` 之后、首个回合之前调用。
    pub(super) fn set_read_aloud(&mut self, read_aloud: bool) {
        self.read_aloud = read_aloud;
    }

    /// 当前相位。
    pub(super) fn phase(&self) -> TurnPhase {
        self.phase
    }

    /// 当前轮次；没有正在进行的轮次时为 0（协议不使用 0）。
    pub(super) fn epoch(&self) -> u32 {
        self.active.as_ref().map_or(0, |active| active.epoch)
    }

    /// 提交成功：分配 epoch 并进入思考。
    pub(super) fn submitted(&mut self, interaction_id: &str) -> AssignedTurn {
        let epoch = self.next_epoch;
        self.next_epoch = self.next_epoch.saturating_add(1);
        let turn_id = format!("{interaction_id}:{epoch}");
        self.active = Some(ActiveTurn {
            epoch,
            turn_id: turn_id.clone(),
            stopped: false,
            seq: 0,
        });
        self.awaiting = None;
        self.render = SpeechStream::new();
        self.spoken_any = false;
        self.phase = TurnPhase::Thinking;
        AssignedTurn { epoch, turn_id }
    }

    /// 提交失败：不消耗 epoch，会话保持。
    pub(super) fn rejected(&mut self) {
        if self.active.is_none() {
            self.phase = TurnPhase::Listening;
        }
    }

    /// 收到助手文本增量：渲染成可朗读句子，`seq` 每轮从 1 起。
    pub(super) fn on_assistant_delta(&mut self, delta: &str) -> Vec<SpeakFrame> {
        if self.active.as_ref().is_none_or(|active| active.stopped) {
            return Vec::new();
        }
        let sentences = self.render.feed(delta);
        self.speak_sentences(sentences)
    }

    /// 本轮终态：补发最后一句并停播。
    ///
    /// 增量里没有产生任何朗读稿时（例如 provider 只给最终消息），用
    /// `turn_completed.response` 兜底渲染，避免整轮没有声音。
    pub(super) fn on_turn_completed(
        &mut self,
        now: Instant,
        response: Option<&str>,
    ) -> (Vec<SpeakFrame>, Option<StopAudio>) {
        if self.active.as_ref().is_none_or(|active| active.stopped) {
            return (Vec::new(), None);
        }
        let mut sentences = self.render.flush();
        if !self.spoken_any
            && sentences.is_empty()
            && let Some(text) = response
        {
            sentences = render_sentences(text, Verbosity::Concise);
        }
        let speaks = self.speak_sentences(sentences);
        let Some(stop) = self.begin_stop(now, "turn_end") else {
            return (speaks, None);
        };
        (speaks, Some(stop))
    }

    /// 客户端取消：朗读期立即停播并进入回执闭环。
    pub(super) fn on_client_cancel(&mut self, now: Instant) -> Option<StopAudio> {
        self.begin_stop(now, "user_cancel")
    }

    /// 收到 `playbackDrained`；只有命中当前停播轮次才算数。
    pub(super) fn on_playback_drained(&mut self, epoch: u32) -> DrainedOutcome {
        let Some(awaiting) = self.awaiting.as_ref() else {
            return DrainedOutcome::Idle;
        };
        if awaiting.epoch != epoch {
            return DrainedOutcome::Stale;
        }
        self.awaiting = None;
        self.active = None;
        self.phase = TurnPhase::Listening;
        DrainedOutcome::Accepted
    }

    /// 停播回执截止时刻；没有等待时为空。
    pub(super) fn drain_deadline(&self) -> Option<Instant> {
        self.awaiting.as_ref().map(|awaiting| awaiting.deadline)
    }

    /// 回执超时：记 `internal` 并强制清理。
    pub(super) fn on_drain_timeout(&mut self, now: Instant) -> bool {
        let Some(awaiting) = self.awaiting.as_ref() else {
            return false;
        };
        if now < awaiting.deadline {
            return false;
        }
        self.awaiting = None;
        self.active = None;
        self.phase = TurnPhase::Listening;
        true
    }

    /// 该 epoch 的音频是否还应下行。
    pub(super) fn accepts_audio(&self, epoch: u32) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.epoch == epoch && !active.stopped)
    }

    fn begin_stop(&mut self, now: Instant, reason: &'static str) -> Option<StopAudio> {
        let active = self.active.as_mut()?;
        if active.stopped {
            return None;
        }
        active.stopped = true;
        let epoch = active.epoch;
        self.awaiting = Some(AwaitingDrain {
            epoch,
            deadline: now + self.drain_timeout,
        });
        Some(StopAudio { epoch, reason })
    }

    fn speak_sentences(&mut self, sentences: Vec<String>) -> Vec<SpeakFrame> {
        if !self.read_aloud {
            return Vec::new();
        }
        let Some(active) = self.active.as_mut() else {
            return Vec::new();
        };
        if active.stopped {
            return Vec::new();
        }
        let mut speaks = Vec::with_capacity(sentences.len());
        for sentence in sentences {
            let text = sentence.trim();
            if text.is_empty() {
                continue;
            }
            active.seq = active.seq.saturating_add(1);
            speaks.push(SpeakFrame {
                turn_id: active.turn_id.clone(),
                epoch: active.epoch,
                seq: active.seq,
                text: text.to_owned(),
            });
        }
        if !speaks.is_empty() {
            self.phase = TurnPhase::Speaking;
            self.spoken_any = true;
        }
        speaks
    }
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

/// `stopAudio` 帧。
pub(super) fn stop_audio_json(stop: &StopAudio) -> String {
    serde_json::json!({
        "type": "stopAudio",
        "epoch": stop.epoch,
        "reason": stop.reason,
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

/// 停播回执超时的 `internal` 错误帧；阶段是 `playback`（PROTOCOL §4.2）。
pub(super) fn drain_timeout_json() -> String {
    let failure = VoiceFailure::Internal {
        detail: "停播回执超时".to_owned(),
    };
    serde_json::json!({
        "type": "error",
        "code": failure.code(),
        "stage": "playback",
        "retryable": failure.retryable(),
        "message": failure.user_message(),
    })
    .to_string()
}
