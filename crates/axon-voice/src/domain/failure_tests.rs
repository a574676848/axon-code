//! `VoiceFailure` 的映射与文案测试：单独成文件，保持领域模块在 700 行以内。

use super::{
    IdlePhase, ModelFamily, PresetReason, ProtocolReason, Stage, VoiceFailure, WorkerSubStage,
};
use crate::domain::phase::{Phase, PhaseError, TransitionTrigger};
use std::collections::{HashMap, HashSet};

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
        VoiceFailure::ProviderUnavailable,
        VoiceFailure::WorkerCrashed {
            sub_stage: WorkerSubStage::Asr,
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
fn every_protocol_code_is_covered_both_ways() {
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
        "provider_unavailable",
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
    let mut by_code: HashMap<&str, usize> = HashMap::new();
    for failure in all_variants() {
        *by_code.entry(failure.code()).or_default() += 1;
    }
    for code in expected {
        assert_eq!(by_code.get(code), Some(&1), "错误码 {code} 未唯一映射");
    }
    assert_eq!(by_code.len(), expected.len());
}

#[test]
fn messages_match_protocol_and_never_leak_internal_detail() {
    let exact = [
        (
            VoiceFailure::WorkerCrashed {
                sub_stage: WorkerSubStage::Io,
                exit_code: 9,
            },
            "语音引擎异常退出，已自动重启（至多 3 次）",
        ),
        (
            VoiceFailure::Backpressure { dropped_frames: 12 },
            "网络或处理跟不上，已丢弃部分音频",
        ),
        (
            VoiceFailure::ModelUnusable {
                family: ModelFamily::Asr,
            },
            "语音提供方拒绝了这个模型，请更换模型标识",
        ),
        (
            VoiceFailure::ModelMissing {
                family: ModelFamily::Tts,
            },
            "所选语音模型不可用，请检查提供方配置",
        ),
        (
            VoiceFailure::ProviderUnavailable,
            "尚未接入云端语音提供方，请先配置阿里百炼或火山引擎",
        ),
    ];
    for (failure, message) in exact {
        assert_eq!(failure.user_message(), message, "{:?}", failure.code());
    }
    for failure in all_variants() {
        let message = failure.user_message();
        assert!(!message.contains("unexpected") && !message.contains("busy"));
        assert!(!message.contains("missing_model"));
    }
}

#[test]
fn policy_rejected_hides_reason() {
    let failure = VoiceFailure::PresetUnavailable {
        preset_id: "x".to_owned(),
        reason: PresetReason::PolicyRejected,
    };
    assert_eq!(failure.user_message(), "该音色不可用");
}

#[test]
fn retry_and_close_flags_match_protocol_table() {
    assert!(VoiceFailure::Unauthorized.retryable());
    assert!(VoiceFailure::Busy.retryable());
    assert!(VoiceFailure::Unauthorized.closes_connection());
    assert!(
        !VoiceFailure::Protocol {
            reason: ProtocolReason::Version
        }
        .retryable()
    );
    assert!(
        VoiceFailure::Protocol {
            reason: ProtocolReason::Oversize
        }
        .retryable()
    );
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

#[test]
fn stages_follow_protocol_table() {
    let fixed = [
        (VoiceFailure::Unauthorized, Stage::Connect),
        (
            VoiceFailure::ModelMissing {
                family: ModelFamily::Vad,
            },
            Stage::Arming,
        ),
        (
            VoiceFailure::WorkerCrashed {
                sub_stage: WorkerSubStage::Warmup,
                exit_code: 10,
            },
            Stage::Worker,
        ),
        (VoiceFailure::TtsFailed, Stage::Speaking),
        (
            VoiceFailure::TurnRejected {
                code: "x".to_owned(),
            },
            Stage::Thinking,
        ),
        (
            VoiceFailure::IdleTimeout {
                phase: IdlePhase::Session,
            },
            Stage::Shutdown,
        ),
        (VoiceFailure::PreviewTimeout, Stage::Playback),
    ];
    for (failure, stage) in fixed {
        assert_eq!(failure.default_stage(), Some(stage), "{:?}", failure.code());
    }

    for context_dependent in [
        VoiceFailure::Protocol {
            reason: ProtocolReason::Oversize,
        },
        VoiceFailure::Internal {
            detail: "x".to_owned(),
        },
        VoiceFailure::CueUnavailable,
    ] {
        assert_eq!(context_dependent.default_stage(), None);
        assert_eq!(context_dependent.stage_or(Stage::Speaking), Stage::Speaking);
    }
}

#[test]
fn phase_error_converts_to_protocol_sequence() {
    let failure: VoiceFailure = PhaseError::IllegalTransition {
        from: Phase::Idle,
        trigger: TransitionTrigger::Warm,
    }
    .into();
    assert_eq!(
        failure,
        VoiceFailure::Protocol {
            reason: ProtocolReason::Sequence
        }
    );
}

#[test]
fn from_code_roundtrips_every_code() {
    let codes = [
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
        "provider_unavailable",
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
    for code in codes {
        let failure = VoiceFailure::from_code(code).expect("码表内应有变体");
        assert_eq!(failure.code(), code, "往返失败：{code}");
        assert!(!failure.user_message().is_empty());
    }
    assert!(VoiceFailure::from_code("nope").is_none());
}

#[test]
fn stage_tokens_are_unique() {
    let tokens: HashSet<&str> = Stage::ALL.iter().map(|stage| stage.as_token()).collect();
    assert_eq!(tokens.len(), Stage::ALL.len());
}
