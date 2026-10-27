//! 语音运行态探测与连接级 `ready`：模型、宿主与降级字段的唯一事实来源。
//!
//! 只做文件存在性探测，不读模型内容，避免建连被 0.5 GB 哈希拖住；`ready` 与
//! `/api/v1/voice/status` 共用同一判据，不允许「建连说可用、status 说不可用」。

use std::path::PathBuf;

/// 默认请求的识别模型名，与合同示例一致。
pub(super) const DEFAULT_ASR_MODEL: &str = "sherpa-streaming-zh-en";

/// 模型根：优先运行态根，否则回落到用户目录下的安装位置。
pub(super) fn model_root() -> PathBuf {
    if let Ok(root) = std::env::var("AXON_RUNTIME_ROOT") {
        return PathBuf::from(root).join("voice").join("models");
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".axon").join("voice").join("models")
}

/// 连接级 `ready`：模型与降级字段必须来自真实探测，不能写占位。
///
/// `workspaceId`/`replyDestination`/`sessionId`/`epoch`/`phase`/`tts` 是**会话级**
/// 数据，建连时还没有 `start`，按 PROTOCOL §2.5 保留初始值，`start` 之后由
/// `state`/`turn` 帧承载真实值。
pub(super) fn ready_json() -> String {
    let (asr_ready, tts_ready) = runtime_probe();
    let asr_model = if asr_ready {
        DEFAULT_ASR_MODEL
    } else {
        "unavailable"
    };
    let tts_model = if tts_ready {
        axon_voice_host::inference::TTS_MODEL
    } else {
        "none"
    };
    let detail = degraded_detail(asr_ready, tts_ready);
    // 严格按 PROTOCOL §2.1：`degraded` 表达「降级到哪个替代方案」，不是可用性开关。
    // `asr` 只允许 null；`tts` 的 "system"/"none" 需要真实的系统音色回退决策，当前
    // 尚未接入系统音色，因此两族一律 null，具体原因统一进 `detail`（中文），不再
    // 一族写 null、另一族写 "none" 的两套词汇。
    serde_json::json!({
        "type": "ready",
        "asrModel": asr_model,
        "ttsModel": tts_model,
        "sampleRate": 16_000,
        "workspaceId": "",
        "replyDestination": "",
        "sessionId": "",
        "epoch": 0,
        "phase": "idle",
        "tts": tts_ready,
        "degraded": {
            "asr": serde_json::Value::Null,
            "tts": serde_json::Value::Null,
            "detail": detail,
        },
        "cues": false
    })
    .to_string()
}

/// 降级原因：可用性一律写进 `detail`，不占用 `degraded.asr`/`degraded.tts` 的取值。
fn degraded_detail(asr_ready: bool, tts_ready: bool) -> Option<String> {
    match (asr_ready, tts_ready) {
        (true, true) => None,
        (false, true) => Some("本地识别模型未安装".to_owned()),
        (true, false) => Some("本地合成模型未安装".to_owned()),
        (false, false) => Some("本地语音模型未安装".to_owned()),
    }
}

/// 识别可用性：模型清单与运行时都在才算可用。
pub(super) fn asr_ready() -> bool {
    let root = model_root();
    let has_model = root
        .join("asr")
        .join(DEFAULT_ASR_MODEL)
        .join("manifest.json")
        .is_file();
    let has_runtime = axon_voice_host::inference::Inference::discover(&root).is_some();
    has_model && has_runtime
}

fn tts_ready() -> bool {
    model_root()
        .join("tts")
        .join(axon_voice_host::inference::TTS_MODEL)
        .join("manifest.json")
        .is_file()
}

fn runtime_probe() -> (bool, bool) {
    (asr_ready(), tts_ready())
}

#[cfg(test)]
mod tests {
    use super::{degraded_detail, ready_json};

    #[test]
    fn degraded_detail_is_chinese_for_every_unready_combination() {
        assert_eq!(degraded_detail(true, true), None);
        for (asr_ready, tts_ready) in [(false, true), (true, false), (false, false)] {
            let detail = degraded_detail(asr_ready, tts_ready).expect("应有原因");
            assert!(!detail.is_ascii(), "原因须为中文：{detail}");
        }
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
    fn ready_has_contract_fields() {
        let value: serde_json::Value = serde_json::from_str(&ready_json()).expect("合法 JSON");
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
