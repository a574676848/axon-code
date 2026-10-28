//! 语音下行文本帧的统一构造：状态、错误与背压。
//!
//! 所有文案与错误码都从 [`VoiceFailure`] 取，禁止在调用点另写一套字符串。

use axon_voice::VoiceFailure;

/// `state` 帧。
pub(super) fn state_json(phase: &str) -> String {
    serde_json::json!({ "type": "state", "phase": phase }).to_string()
}

/// 错误帧；未知错误码按 `internal` 文案兜底。
///
/// 旧签名只服务 ASR 语境：`model_missing`/`model_unusable` 的 `detail.family` 固定写
/// `"asr"`。别的模型族必须走 [`error_json_with_family`]，由调用方给出真实 family，
/// 不再由这里替调用方假定。
pub(super) fn error_json(code: &str, stage: &str) -> String {
    let family = matches!(code, "model_missing" | "model_unusable").then_some("asr");
    error_json_with_family(code, stage, family)
}

/// 带 family 的错误帧：`family` 由调用方给出，宿主错误按宿主回执的
/// `detail.family` 原样透传，避免把 tts/vad 的错误都记到 asr 头上。
pub(super) fn error_json_with_family(code: &str, stage: &str, family: Option<&str>) -> String {
    let failure = VoiceFailure::from_code(code);
    let (message, retryable) = match &failure {
        Some(failure) => (failure.user_message(), failure.retryable()),
        None => ("出现内部错误，请重试".to_owned(), true),
    };
    serde_json::json!({
        "type": "error",
        "code": code,
        "stage": stage,
        "retryable": retryable,
        "message": message,
        "detail": { "family": family },
    })
    .to_string()
}

/// `backpressure` 错误帧：不关连接，带累计丢帧数。
pub(super) fn backpressure_json(frames: u64, bytes: u64) -> String {
    serde_json::json!({
        "type": "error",
        "code": "backpressure",
        "stage": "listening",
        "retryable": true,
        "message": message_for("backpressure"),
        "detail": { "droppedFrames": frames, "droppedBytes": bytes },
    })
    .to_string()
}

/// 合同错误码对应的中文文案。
pub(super) fn message_for(code: &str) -> String {
    VoiceFailure::from_code(code)
        .map(|failure| failure.user_message())
        .unwrap_or_else(|| "出现内部错误，请重试".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{backpressure_json, error_json, error_json_with_family};

    #[test]
    fn backpressure_frame_keeps_the_connection_alive() {
        let value: serde_json::Value =
            serde_json::from_str(&backpressure_json(3, 9_600)).expect("合法 JSON");
        assert_eq!(value["code"], "backpressure");
        assert_eq!(value["stage"], "listening");
        assert_eq!(value["retryable"], true);
        assert_eq!(value["detail"]["droppedFrames"], 3);
        assert_eq!(value["detail"]["droppedBytes"], 9_600);
    }

    #[test]
    fn error_frame_uses_contract_message() {
        let value: serde_json::Value =
            serde_json::from_str(&error_json("model_missing", "arming")).expect("合法 JSON");
        assert_eq!(value["code"], "model_missing");
        assert_eq!(value["message"], "所选语音模型不可用，请检查提供方配置");
        assert_eq!(value["detail"]["family"], "asr");
        assert_eq!(value["retryable"], true);
    }

    #[test]
    fn error_frame_carries_the_caller_family() {
        let value: serde_json::Value = serde_json::from_str(&error_json_with_family(
            "model_unusable",
            "arming",
            Some("tts"),
        ))
        .expect("合法 JSON");
        assert_eq!(value["detail"]["family"], "tts");
        assert_eq!(value["code"], "model_unusable");
    }
}
