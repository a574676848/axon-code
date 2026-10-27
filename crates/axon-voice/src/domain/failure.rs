//! 失败分类：17 个变体，与 `PROTOCOL.md` §4.2 的 21 个错误码唯一映射。
//!
//! 改码必须同时改本文件与 `PROTOCOL.md` §4.2；`code()` 与 `user_message()`
//! 是运行时唯一来源，其它层不得另写一份文案。

/// 模型族。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelFamily {
    /// 端点检测。
    Vad,
    /// 识别。
    Asr,
    /// 合成。
    Tts,
}

impl ModelFamily {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Vad => "vad",
            Self::Asr => "asr",
            Self::Tts => "tts",
        }
    }
}

/// 协议失败细分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolReason {
    /// 协议版本不一致。
    Version,
    /// 帧内出现未知字段。
    UnknownField,
    /// 控制序列非法。
    Sequence,
    /// 帧超限。
    Oversize,
}

impl ProtocolReason {
    /// 对应的错误码。
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Version => "protocol_version",
            Self::UnknownField => "protocol_unknown_field",
            Self::Sequence => "protocol_sequence",
            Self::Oversize => "frame_oversize",
        }
    }
}

/// 空闲超时发生的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdlePhase {
    /// 建连后未开始。
    Connect,
    /// 会话中无输入。
    Session,
}

/// 预设不可用的原因，与 `PROTOCOL.md` §4.3 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresetReason {
    /// 所需模型未安装。
    MissingModel,
    /// 说话人超出模型声明范围。
    SpeakerOutOfRange,
    /// 用户清单解析失败。
    FileInvalid,
    /// 未通过合规审查。
    PolicyRejected,
}

impl PresetReason {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::MissingModel => "missing_model",
            Self::SpeakerOutOfRange => "speaker_out_of_range",
            Self::FileInvalid => "file_invalid",
            Self::PolicyRejected => "policy_rejected",
        }
    }
}

/// 宿主崩溃的子阶段，只用于 `detail.reason`，不参与阶段枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerSubStage {
    /// 预热。
    Warmup,
    /// 识别。
    Asr,
    /// 合成。
    Tts,
    /// 音频 IO。
    Io,
    /// 播放。
    Playback,
}

impl WorkerSubStage {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Asr => "asr",
            Self::Tts => "tts",
            Self::Io => "io",
            Self::Playback => "playback",
        }
    }
}

/// 语音失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceFailure {
    /// 网关凭据失效。
    Unauthorized,
    /// 并发名额已满。
    Busy,
    /// 协议错误。
    Protocol {
        /// 细分原因。
        reason: ProtocolReason,
    },
    /// 模型缺失。
    ModelMissing {
        /// 模型族。
        family: ModelFamily,
    },
    /// 模型存在但运行时不可用。
    ModelUnusable {
        /// 模型族。
        family: ModelFamily,
    },
    /// 设备不可用。
    DeviceUnavailable {
        /// 已脱敏的细节。
        detail: String,
    },
    /// 宿主未启动或启动超时。
    WorkerUnavailable,
    /// 宿主异常退出。
    WorkerCrashed {
        /// 崩溃时所在子阶段。
        sub_stage: WorkerSubStage,
        /// 退出码。
        exit_code: i32,
    },
    /// 上行背压丢帧。
    Backpressure {
        /// 已丢弃帧数。
        dropped_frames: u64,
    },
    /// 识别过载。
    Overloaded,
    /// 合成失败。
    TtsFailed,
    /// 提交被拒。
    TurnRejected {
        /// 提交层给出的稳定原因码。
        code: String,
    },
    /// 空闲超时。
    IdleTimeout {
        /// 发生阶段。
        phase: IdlePhase,
    },
    /// 未归类异常。
    Internal {
        /// 已脱敏细节。
        detail: String,
    },
    /// 预设不可用。
    PresetUnavailable {
        /// 预设标识。
        preset_id: String,
        /// 不可用原因。
        reason: PresetReason,
    },
    /// 试听超时。
    PreviewTimeout,
    /// cue 不可用。
    CueUnavailable,
}

impl VoiceFailure {
    /// 稳定错误码。
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Busy => "voice_busy",
            Self::Protocol { reason } => reason.code(),
            Self::ModelMissing { .. } => "model_missing",
            Self::ModelUnusable { .. } => "model_unusable",
            Self::DeviceUnavailable { .. } => "device_unavailable",
            Self::WorkerUnavailable => "worker_unavailable",
            Self::WorkerCrashed { .. } => "worker_crashed",
            Self::Backpressure { .. } => "backpressure",
            Self::Overloaded => "asr_overloaded",
            Self::TtsFailed => "tts_failed",
            Self::TurnRejected { .. } => "submit_rejected",
            Self::IdleTimeout { phase } => match phase {
                IdlePhase::Connect => "idle_timeout",
                IdlePhase::Session => "session_idle",
            },
            Self::Internal { .. } => "internal",
            Self::PresetUnavailable { .. } => "preset_unavailable",
            Self::PreviewTimeout => "preview_timeout",
            Self::CueUnavailable => "cue_unavailable",
        }
    }

    /// 是否可重试。
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Unauthorized
            | Self::Busy
            | Self::Protocol { .. }
            | Self::ModelUnusable { .. }
            | Self::TurnRejected { .. }
            | Self::PresetUnavailable { .. }
            | Self::CueUnavailable => false,
            Self::ModelMissing { .. }
            | Self::DeviceUnavailable { .. }
            | Self::WorkerUnavailable
            | Self::WorkerCrashed { .. }
            | Self::Backpressure { .. }
            | Self::Overloaded
            | Self::TtsFailed
            | Self::IdleTimeout { .. }
            | Self::Internal { .. }
            | Self::PreviewTimeout => true,
        }
    }

    /// 是否由服务端关闭连接。
    #[must_use]
    pub const fn closes_connection(&self) -> bool {
        matches!(
            self,
            Self::Unauthorized | Self::Busy | Self::Protocol { .. } | Self::IdleTimeout { .. }
        )
    }

    /// 可直接展示的中文文案，与 `PROTOCOL.md` §4.2 逐字一致。
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::Unauthorized => "网关凭据失效，请重新连接".to_owned(),
            Self::Busy => "一次只能进行一路语音，请先结束当前这一路".to_owned(),
            Self::Protocol { reason } => match reason {
                ProtocolReason::Version => "版本不一致，请升级 Axon".to_owned(),
                ProtocolReason::UnknownField => "协议字段不被识别，已断开".to_owned(),
                ProtocolReason::Sequence => "控制序列非法，已断开".to_owned(),
                ProtocolReason::Oversize => "音频包过大，已断开并重连".to_owned(),
            },
            Self::ModelMissing { family } => {
                format!(
                    "本地语音模型未安装（{}），点此查看安装指引",
                    family.as_token()
                )
            }
            Self::ModelUnusable { family } => {
                format!(
                    "模型与运行时版本不匹配（{}），请重新安装",
                    family.as_token()
                )
            }
            Self::DeviceUnavailable { .. } => "麦克风不可用，请检查系统隐私设置或换设备".to_owned(),
            Self::WorkerUnavailable => "语音引擎未启动，请重试".to_owned(),
            Self::WorkerCrashed { .. } => "语音引擎异常退出，已自动重启".to_owned(),
            Self::Backpressure { dropped_frames } => {
                format!("网络或处理跟不上，已丢弃 {dropped_frames} 帧音频")
            }
            Self::Overloaded => "识别跟不上，请放慢语速或改用推键".to_owned(),
            Self::TtsFailed => "朗读失败，已静音继续".to_owned(),
            Self::TurnRejected { code } => format!("本轮未能提交：{code}"),
            Self::IdleTimeout { phase } => match phase {
                IdlePhase::Connect => "长时间未开始，已结束".to_owned(),
                IdlePhase::Session => "长时间无输入，已结束".to_owned(),
            },
            Self::Internal { .. } => "出现内部错误，请重试".to_owned(),
            Self::PresetUnavailable { preset_id, reason } => {
                format!("音色 {preset_id} 不可用：{}", reason.as_token())
            }
            Self::PreviewTimeout => "试听超时，请重试或换一个音色".to_owned(),
            Self::CueUnavailable => "语音提示音不可用，已关闭".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{IdlePhase, ModelFamily, PresetReason, ProtocolReason, VoiceFailure};
    use std::collections::HashSet;

    fn all_variants() -> Vec<VoiceFailure> {
        vec![
            VoiceFailure::Unauthorized,
            VoiceFailure::Busy,
            VoiceFailure::Protocol {
                reason: ProtocolReason::Version,
            },
            VoiceFailure::Protocol {
                reason: ProtocolReason::UnknownField,
            },
            VoiceFailure::Protocol {
                reason: ProtocolReason::Sequence,
            },
            VoiceFailure::Protocol {
                reason: ProtocolReason::Oversize,
            },
            VoiceFailure::ModelMissing {
                family: ModelFamily::Asr,
            },
            VoiceFailure::ModelUnusable {
                family: ModelFamily::Tts,
            },
            VoiceFailure::DeviceUnavailable {
                detail: "busy".to_owned(),
            },
            VoiceFailure::WorkerUnavailable,
            VoiceFailure::WorkerCrashed {
                sub_stage: super::WorkerSubStage::Asr,
                exit_code: 13,
            },
            VoiceFailure::Backpressure { dropped_frames: 3 },
            VoiceFailure::Overloaded,
            VoiceFailure::TtsFailed,
            VoiceFailure::TurnRejected {
                code: "lane_closed".to_owned(),
            },
            VoiceFailure::IdleTimeout {
                phase: IdlePhase::Connect,
            },
            VoiceFailure::IdleTimeout {
                phase: IdlePhase::Session,
            },
            VoiceFailure::Internal {
                detail: "unexpected".to_owned(),
            },
            VoiceFailure::PresetUnavailable {
                preset_id: "calm-elegant".to_owned(),
                reason: PresetReason::MissingModel,
            },
            VoiceFailure::PreviewTimeout,
            VoiceFailure::CueUnavailable,
        ]
    }

    #[test]
    fn every_protocol_code_is_covered() {
        let expected = [
            "unauthorized",
            "voice_busy",
            "protocol_version",
            "protocol_unknown_field",
            "protocol_sequence",
            "frame_oversize",
            "model_missing",
            "model_unusable",
            "device_unavailable",
            "worker_unavailable",
            "worker_crashed",
            "backpressure",
            "asr_overloaded",
            "tts_failed",
            "submit_rejected",
            "idle_timeout",
            "session_idle",
            "internal",
            "preset_unavailable",
            "preview_timeout",
            "cue_unavailable",
        ];
        let actual: HashSet<&str> = all_variants().iter().map(VoiceFailure::code).collect();
        assert_eq!(actual.len(), expected.len());
        for code in expected {
            assert!(actual.contains(code), "缺少错误码 {code}");
        }
    }

    #[test]
    fn messages_are_non_empty_and_do_not_leak_detail() {
        for failure in all_variants() {
            let message = failure.user_message();
            assert!(!message.trim().is_empty(), "{:?} 文案为空", failure.code());
            assert!(
                !message.contains("unexpected") && !message.contains("busy"),
                "{:?} 文案泄露了内部细节：{message}",
                failure.code()
            );
        }
    }

    #[test]
    fn retry_and_close_flags_match_protocol_table() {
        assert!(!VoiceFailure::Unauthorized.retryable());
        assert!(VoiceFailure::Unauthorized.closes_connection());
        assert!(
            VoiceFailure::DeviceUnavailable {
                detail: String::new()
            }
            .retryable()
        );
        assert!(
            !VoiceFailure::TurnRejected {
                code: "x".to_owned()
            }
            .closes_connection()
        );
        assert!(
            VoiceFailure::IdleTimeout {
                phase: IdlePhase::Session
            }
            .closes_connection()
        );
        assert!(!VoiceFailure::CueUnavailable.retryable());
    }
}
