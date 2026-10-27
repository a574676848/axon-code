//! 失败分类：17 个变体，与 `PROTOCOL.md` §4.2 的 21 个错误码唯一映射。
//!
//! 改码必须同时改本文件与 `PROTOCOL.md` §4.2；`code()` 与 `user_message()`
//! 是运行时唯一来源，其它层不得另写一份文案。`default_stage()` 只在失败本身
//! 决定阶段时给出值，码表写「发生时的阶段值」的码返回 `None`，由调用方补实际阶段。

use crate::domain::phase::PhaseError;

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

/// 阶段枚举，与 `PROTOCOL.md` §4.1 的 8 个值一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// 建连与准入。
    Connect,
    /// 准备模型与设备。
    Arming,
    /// 采集与识别。
    Listening,
    /// 等待模型与提交。
    Thinking,
    /// 合成与播放。
    Speaking,
    /// 客户端播放队列。
    Playback,
    /// 宿主进程。
    Worker,
    /// 收口。
    Shutdown,
}

impl Stage {
    /// 全部阶段。
    pub const ALL: [Self; 8] = [
        Self::Connect,
        Self::Arming,
        Self::Listening,
        Self::Thinking,
        Self::Speaking,
        Self::Playback,
        Self::Worker,
        Self::Shutdown,
    ];

    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Arming => "arming",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Playback => "playback",
            Self::Worker => "worker",
            Self::Shutdown => "shutdown",
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

    /// 面向用户的中文原因；合规拒绝按 §4.3 不展示原因。
    #[must_use]
    pub const fn user_reason(self) -> &'static str {
        match self {
            Self::MissingModel => "需要先安装对应模型",
            Self::SpeakerOutOfRange => "音色参数超出模型支持范围",
            Self::FileInvalid => "音色清单无法解析",
            Self::PolicyRejected => "",
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

    /// 由错误码还原失败变体；未知码返回 `None`。
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        let failure = match code {
            "unauthorized" => Self::Unauthorized,
            "voice_busy" => Self::Busy,
            "protocol_version" => Self::Protocol {
                reason: ProtocolReason::Version,
            },
            "protocol_unknown_field" => Self::Protocol {
                reason: ProtocolReason::UnknownField,
            },
            "protocol_sequence" => Self::Protocol {
                reason: ProtocolReason::Sequence,
            },
            "frame_oversize" => Self::Protocol {
                reason: ProtocolReason::Oversize,
            },
            "model_missing" => Self::ModelMissing {
                family: ModelFamily::Asr,
            },
            "model_unusable" => Self::ModelUnusable {
                family: ModelFamily::Asr,
            },
            "device_unavailable" => Self::DeviceUnavailable {
                detail: String::new(),
            },
            "worker_unavailable" => Self::WorkerUnavailable,
            "worker_crashed" => Self::WorkerCrashed {
                sub_stage: WorkerSubStage::Warmup,
                exit_code: 0,
            },
            "backpressure" => Self::Backpressure { dropped_frames: 0 },
            "asr_overloaded" => Self::Overloaded,
            "tts_failed" => Self::TtsFailed,
            "submit_rejected" => Self::TurnRejected {
                code: String::new(),
            },
            "idle_timeout" => Self::IdleTimeout {
                phase: IdlePhase::Connect,
            },
            "session_idle" => Self::IdleTimeout {
                phase: IdlePhase::Session,
            },
            "internal" => Self::Internal {
                detail: String::new(),
            },
            "preset_unavailable" => Self::PresetUnavailable {
                preset_id: String::new(),
                reason: PresetReason::MissingModel,
            },
            "preview_timeout" => Self::PreviewTimeout,
            "cue_unavailable" => Self::CueUnavailable,
            _ => return None,
        };
        Some(failure)
    }
    /// 是否可重试。
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Unauthorized | Self::Busy => true,
            Self::Protocol { reason } => matches!(reason, ProtocolReason::Oversize),
            Self::ModelUnusable { .. }
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

    /// 失败本身能确定的阶段；码表写「发生时的阶段值」时返回 `None`。
    #[must_use]
    pub const fn default_stage(&self) -> Option<Stage> {
        match self {
            Self::Unauthorized
            | Self::Busy
            | Self::Protocol {
                reason:
                    ProtocolReason::Version | ProtocolReason::UnknownField | ProtocolReason::Sequence,
            } => Some(Stage::Connect),
            Self::ModelMissing { .. }
            | Self::ModelUnusable { .. }
            | Self::DeviceUnavailable { .. }
            | Self::WorkerUnavailable => Some(Stage::Arming),
            Self::WorkerCrashed { .. } => Some(Stage::Worker),
            Self::Backpressure { .. } | Self::Overloaded => Some(Stage::Listening),
            Self::TtsFailed => Some(Stage::Speaking),
            Self::TurnRejected { .. } => Some(Stage::Thinking),
            Self::IdleTimeout {
                phase: IdlePhase::Connect,
            } => Some(Stage::Connect),
            Self::IdleTimeout {
                phase: IdlePhase::Session,
            } => Some(Stage::Shutdown),
            Self::PreviewTimeout => Some(Stage::Playback),
            Self::Protocol {
                reason: ProtocolReason::Oversize,
            }
            | Self::Internal { .. }
            | Self::PresetUnavailable { .. }
            | Self::CueUnavailable => None,
        }
    }

    /// 结合实际发生阶段得到上报值。
    #[must_use]
    pub fn stage_or(&self, observed: Stage) -> Stage {
        self.default_stage().unwrap_or(observed)
    }

    /// 可直接展示的中文文案，与 `PROTOCOL.md` §4.2 逐字一致（仅拼装已脱敏细节）。
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
            Self::ModelMissing { .. } => "本地语音模型未安装，点此查看安装指引".to_owned(),
            Self::ModelUnusable { .. } => "模型与运行时版本不匹配，请重新安装模型".to_owned(),
            Self::DeviceUnavailable { .. } => "麦克风不可用，请检查系统隐私设置或换设备".to_owned(),
            Self::WorkerUnavailable => "语音引擎未启动，请重试".to_owned(),
            Self::WorkerCrashed { .. } => "语音引擎异常退出，已自动重启（至多 3 次）".to_owned(),
            Self::Backpressure { .. } => "网络或处理跟不上，已丢弃部分音频".to_owned(),
            Self::Overloaded => "识别跟不上，请放慢语速或改用推键".to_owned(),
            Self::TtsFailed => "朗读失败，已静音继续".to_owned(),
            Self::TurnRejected { code } => format!("本轮未能提交：{code}"),
            Self::IdleTimeout { phase } => match phase {
                IdlePhase::Connect => "长时间未开始，已结束".to_owned(),
                IdlePhase::Session => "长时间无输入，已结束".to_owned(),
            },
            Self::Internal { .. } => "出现内部错误，请重试".to_owned(),
            Self::PresetUnavailable { reason, .. } => {
                let suffix = reason.user_reason();
                if suffix.is_empty() {
                    "该音色不可用".to_owned()
                } else {
                    format!("该音色不可用：{suffix}")
                }
            }
            Self::PreviewTimeout => "试听超时，请重试或换一个音色".to_owned(),
            Self::CueUnavailable => "语音提示音不可用，已关闭".to_owned(),
        }
    }
}

impl From<PhaseError> for VoiceFailure {
    fn from(_: PhaseError) -> Self {
        Self::Protocol {
            reason: ProtocolReason::Sequence,
        }
    }
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;
