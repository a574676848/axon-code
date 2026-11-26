//! 语音运行态与连接级 `ready`：提供方可用性与降级字段的唯一事实来源。
//!
//! 两条能力分开判定，不合并成一个布尔：**建会话**取决于 realtime 车道，
//! **朗读**取决于合成渠道。任一缺失都如实报不可用并给出稳定原因，不假装就绪，
//! 也不编造模型名。`ready` 帧与 `/api/v1/voice/status` 共用同一判据，
//! 不允许「建连说可用、status 说不可用」。

use super::gateway_voice_provider::VoiceRuntime;

/// 端到端车道的模型标识；不可用时为空。
fn asr_model(runtime: &VoiceRuntime) -> Option<String> {
    runtime.realtime_model()
}

/// 合成渠道的音色标识；不可用时为空。
fn tts_model(runtime: &VoiceRuntime) -> Option<String> {
    runtime.tts().map(|config| config.speaker.clone())
}

/// 能否建立语音会话。
pub(super) fn provider_ready(runtime: &VoiceRuntime) -> bool {
    asr_model(runtime).is_some()
}

/// 能否朗读：合成渠道与凭据齐备即可，与会话车道无关。
pub(super) fn tts_ready(runtime: &VoiceRuntime) -> bool {
    tts_model(runtime).is_some()
}

/// 不可用时的统一原因；可用时为空。
pub(super) fn degraded_detail(ready: bool) -> Option<&'static str> {
    (!ready).then_some("尚未接入云端语音提供方（阿里百炼或火山引擎）")
}

/// 朗读不可用时的原因：合成渠道单独缺配置。
pub(super) fn tts_degraded_detail(ready: bool) -> Option<&'static str> {
    (!ready).then_some("尚未配置合成渠道与凭据（[voice] 或 ARK_KEY）")
}

/// 连接级 `ready`：可用性与降级字段来自真实配置，不写占位。
///
/// `workspaceId`/`replyDestination`/`sessionId`/`epoch`/`phase`/`tts` 是**会话级**
/// 数据，建连时还没有 `start`，按协议保留初始值，`start` 之后由 `state`/`turn`
/// 帧承载真实值。
pub(super) fn ready_json(runtime: &VoiceRuntime) -> String {
    let asr = asr_model(runtime);
    let tts = tts_model(runtime);
    let session_ready = asr.is_some();
    let speak_ready = tts.is_some();
    // 严格按协议：`degraded` 表达「降级到哪个替代方案」，不是可用性开关；
    // 当前没有系统音色兜底，两族一律 null，具体原因统一进 `detail`（中文）。
    serde_json::json!({
        "type": "ready",
        "asrModel": asr.as_deref().unwrap_or("unavailable"),
        "ttsModel": tts.as_deref().unwrap_or("none"),
        "sampleRate": 16_000,
        "workspaceId": "",
        "replyDestination": "",
        "sessionId": "",
        "epoch": 0,
        "phase": "idle",
        "tts": speak_ready,
        "degraded": {
            "asr": serde_json::Value::Null,
            "tts": serde_json::Value::Null,
            "detail": degraded_detail(session_ready),
            "ttsDetail": tts_degraded_detail(speak_ready),
        },
        "cues": speak_ready
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        degraded_detail, provider_ready, ready_json, tts_degraded_detail, tts_ready,
    };
    use super::super::gateway_voice_provider::VoiceRuntime;
    use super::super::voice_tts_volc::VolcTtsConfig;

    fn tts_only() -> VoiceRuntime {
        VoiceRuntime::new(
            None,
            Some(VolcTtsConfig {
                endpoint: "wss://example.invalid/tts".to_owned(),
                api_key: "key".to_owned(),
                resource_id: "seed-tts-2.0".to_owned(),
                speaker: "speaker".to_owned(),
                speech_rate: 0,
            }),
        )
    }

    #[test]
    fn unavailable_carries_a_chinese_reason() {
        let detail = degraded_detail(false).expect("不可用时必须有原因");
        assert!(!detail.is_ascii(), "原因须为中文：{detail}");
        assert_eq!(degraded_detail(true), None);
        let tts_detail = tts_degraded_detail(false).expect("合成不可用也要有原因");
        assert!(!tts_detail.is_ascii(), "原因须为中文：{tts_detail}");
    }

    #[test]
    fn session_and_speech_readiness_are_independent() {
        let runtime = tts_only();
        assert!(!provider_ready(&runtime), "没有 realtime 车道就不能建会话");
        assert!(tts_ready(&runtime), "有合成渠道就能朗读");
        let empty = VoiceRuntime::new(None, None);
        assert!(!provider_ready(&empty));
        assert!(!tts_ready(&empty));
    }

    #[test]
    fn ready_reports_tts_available_while_session_is_not() {
        let value: serde_json::Value =
            serde_json::from_str(&ready_json(&tts_only())).expect("合法 JSON");
        assert_eq!(value["asrModel"], "unavailable");
        assert_eq!(value["ttsModel"], "speaker");
        assert_eq!(value["tts"], true);
        assert_eq!(value["cues"], true);
        assert!(value["degraded"]["detail"].is_string(), "会话不可用要说明原因");
        assert!(value["degraded"]["ttsDetail"].is_null(), "合成可用时不报降级");
    }

    #[test]
    fn degraded_fields_never_carry_a_second_vocabulary() {
        let value: serde_json::Value =
            serde_json::from_str(&ready_json(&tts_only())).expect("合法 JSON");
        assert!(value["degraded"]["asr"].is_null(), "asr 只允许 null");
        assert!(
            value["degraded"]["tts"].is_null(),
            "未接入系统音色时 tts 只能为 null，原因走 detail"
        );
    }

    #[test]
    fn ready_keeps_every_protocol_field() {
        let value: serde_json::Value =
            serde_json::from_str(&ready_json(&VoiceRuntime::new(None, None))).expect("合法 JSON");
        assert_eq!(value["type"], "ready");
        assert_eq!(value["asrModel"], "unavailable");
        assert_eq!(value["ttsModel"], "none");
        assert_eq!(value["tts"], false);
        assert_eq!(value["cues"], false);
        for field in [
            "asrModel",
            "ttsModel",
            "sampleRate",
            "workspaceId",
            "replyDestination",
            "sessionId",
            "epoch",
            "phase",
            "tts",
            "degraded",
            "cues",
        ] {
            assert!(value.get(field).is_some(), "ready 缺字段 {field}");
        }
        assert_eq!(value["sampleRate"], 16_000);
    }
}
