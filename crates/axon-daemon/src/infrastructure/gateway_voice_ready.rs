//! 语音运行态与连接级 `ready`：提供方可用性与降级字段的唯一事实来源。
//!
//! 本机模型已整体移除；识别与合成由云端提供方（阿里百炼、火山引擎）承担，
//! 当前尚未接入。这里如实报不可用并给出稳定原因，不假装就绪，也不编造模型名。
//! `ready` 帧与 `/api/v1/voice/status` 共用同一判据，不允许「建连说可用、status 说不可用」。

/// 当前提供方的识别与合成模型名。
///
/// realtime 车道由一个模型同时承担识别与合成，因此两个位置报同一个标识；
/// 没有可用车道时返回 `None`，如实表示尚未接入。
fn provider_models() -> Option<(String, String)> {
    #[cfg(test)]
    {
        // 与 `spawn_voice_worker` 一致：测试恒走级联脚本宿主，不把真实凭据卷进用例。
        None
    }
    #[cfg(not(test))]
    {
        let model = super::gateway_voice_provider::realtime_model()?;
        Some((model.clone(), model))
    }
}

/// 提供方是否可用。
pub(super) fn provider_ready() -> bool {
    provider_models().is_some()
}

/// 不可用时的统一原因；可用时为空。
pub(super) fn degraded_detail(ready: bool) -> Option<&'static str> {
    (!ready).then_some("尚未接入云端语音提供方（阿里百炼或火山引擎）")
}

/// 连接级 `ready`：可用性与降级字段来自真实探测，不写占位。
///
/// `workspaceId`/`replyDestination`/`sessionId`/`epoch`/`phase`/`tts` 是**会话级**
/// 数据，建连时还没有 `start`，按协议保留初始值，`start` 之后由 `state`/`turn`
/// 帧承载真实值。
pub(super) fn ready_json() -> String {
    let models = provider_models();
    let ready = models.is_some();
    // 严格按协议：`degraded` 表达「降级到哪个替代方案」，不是可用性开关；
    // 当前没有系统音色兜底，两族一律 null，具体原因统一进 `detail`（中文）。
    serde_json::json!({
        "type": "ready",
        "asrModel": models
            .as_ref()
            .map_or_else(|| "unavailable".to_owned(), |models| models.0.clone()),
        "ttsModel": models
            .as_ref()
            .map_or_else(|| "none".to_owned(), |models| models.1.clone()),
        "sampleRate": 16_000,
        "workspaceId": "",
        "replyDestination": "",
        "sessionId": "",
        "epoch": 0,
        "phase": "idle",
        "tts": ready,
        "degraded": {
            "asr": serde_json::Value::Null,
            "tts": serde_json::Value::Null,
            "detail": degraded_detail(ready),
        },
        "cues": false
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{degraded_detail, provider_ready, ready_json};

    #[test]
    fn unavailable_carries_a_chinese_reason() {
        assert!(!provider_ready(), "提供方尚未接入，必须如实报不可用");
        let detail = degraded_detail(false).expect("不可用时必须有原因");
        assert!(!detail.is_ascii(), "原因须为中文：{detail}");
        assert_eq!(degraded_detail(true), None);
    }

    #[test]
    fn degraded_fields_never_carry_a_second_vocabulary() {
        let value: serde_json::Value = serde_json::from_str(&ready_json()).expect("合法 JSON");
        assert!(value["degraded"]["asr"].is_null(), "asr 只允许 null");
        assert!(
            value["degraded"]["tts"].is_null(),
            "未接入系统音色时 tts 只能为 null，原因走 detail"
        );
    }

    #[test]
    fn ready_reports_unavailable_models_while_no_provider_is_configured() {
        let value: serde_json::Value = serde_json::from_str(&ready_json()).expect("合法 JSON");
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
