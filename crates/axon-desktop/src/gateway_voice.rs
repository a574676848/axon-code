//! 语音合成与试听的网关调用。
//!
//! 与 [`crate::gateway`] 的读写请求分开：这里的两条路由各自带独立超时（合成要等模型
//! 冷启动，试听要等一次短合成），失败分类也各自成域（409 与 503 是「提供方没就绪」，
//! 不是「请求错了」）。混在通用 HTTP 客户端里，两套口径会互相污染。

use std::time::Duration;

use crate::connection::endpoint;
use crate::gateway::{
    agent_with_timeout, api_token, endpoint, gateway_status_message, get_json,
    transport_failure_message,
};

/// 朗读合成的总超时。
///
/// 本地语音合成是命令行逐次加载模型，长句在冷启动时要十几秒；用通用的 15 秒预算
/// 会把「正在合成」误报成「网关不可达」。朗读本身也按句分段，单段很少接近这个上限。
const VOICE_TTS_TIMEOUT: Duration = Duration::from_secs(60);

/// 试听合成的总超时；首次试听要加载模型，给足冷启动余量。
const VOICE_PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);


/// 语音合成失败分类：409 归为运行时不可用，其余归为请求失败。
#[derive(Debug)]
pub enum VoiceTtsError {
    /// 语音提供方或音色未就绪。
    Unavailable(String),
    /// 其它失败。
    Failed(String),
}

impl std::fmt::Display for VoiceTtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) | Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// 调用网关合成一段朗读音频，返回 WAV 字节。
///
/// # Errors
///
/// 网关不可达、鉴权失败、语音提供方未就绪或合成失败。
pub fn post_voice_tts(
    text: &str,
    preset_id: Option<&str>,
    persona: Option<&serde_json::Value>,
) -> Result<Vec<u8>, VoiceTtsError> {
    let url = endpoint("/voice/tts").map_err(VoiceTtsError::Failed)?;
    let mut request = agent_with_timeout(VOICE_TTS_TIMEOUT)
        .post(&url)
        .header("accept", "audio/wav")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let payload = serde_json::json!({
        "text": text,
        "presetId": preset_id.unwrap_or(""),
        "persona": persona,
    });
    let response = match request.send(payload.to_string()) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(status)) => {
            return Err(classify_voice_status(status, String::new()));
        }
        Err(ureq::Error::Timeout(_)) => {
            return Err(VoiceTtsError::Failed(
                "朗读合成超时：这段文字较长，稍后重试或缩短内容".to_owned(),
            ));
        }
        Err(error) => return Err(VoiceTtsError::Failed(transport_failure_message(&error))),
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.into_body().read_to_string().unwrap_or_default();
        return Err(classify_voice_status(status, body));
    }
    let mut body = response.into_body();
    body.read_to_vec()
        .map_err(|error| VoiceTtsError::Failed(format!("读取音频失败：{error}")))
}

fn classify_voice_status(status: u16, body: String) -> VoiceTtsError {
    let reason = gateway_status_message(status, &body);
    if status == 409 || status == 503 {
        VoiceTtsError::Unavailable(reason)
    } else {
        VoiceTtsError::Failed(reason)
    }
}

/// 读取音色预设清单。
///
/// # Errors
///
/// 网关不可达、鉴权失败或响应不是合法 JSON。
pub fn get_voice_presets() -> Result<serde_json::Value, String> {
    get_json("/voice/presets")
}

/// 读取语音提供方状态：是否可用、降级原因与预设数量。
///
/// 与 `/api/v1/voice` 建连时给出的 `ready` 共用同一判据，因此「点麦克风之前」
/// 就能知道本地语音是否装好，不必先等一次冷启动失败。
///
/// # Errors
///
/// 网关不可达、鉴权失败或响应不是合法 JSON。
pub fn get_voice_runtime_status() -> Result<serde_json::Value, String> {
    get_json("/voice/status")
}

/// 试听音频：WAV 字节与网关实际采用的音色。
pub struct VoicePreviewAudio {
    /// WAV 字节。
    pub bytes: Vec<u8>,
    /// 网关响应头给出的实际音色；缺失时为空串。
    pub preset_id: String,
}

/// 试听失败分类：超时、不可用、参数非法与其它失败各有各的文案。
#[derive(Debug)]
pub enum VoicePreviewError {
    /// 504：合成超时。
    Timeout,
    /// 409：语音提供方或音色不可用。
    Unavailable(String),
    /// 400：请求体不合法。
    Invalid(String),
    /// 其它失败。
    Failed(String),
}

impl std::fmt::Display for VoicePreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("试听超时，请重试或换一个音色"),
            Self::Unavailable(reason) | Self::Invalid(reason) | Self::Failed(reason) => {
                f.write_str(reason)
            }
        }
    }
}

/// 调用网关合成一句试听，返回 WAV 字节。
///
/// # Errors
///
/// 网关不可达、鉴权失败、语音提供方未就绪或合成超时。
pub fn post_voice_preview(
    preset_id: Option<&str>,
    persona: Option<&serde_json::Value>,
) -> Result<VoicePreviewAudio, VoicePreviewError> {
    let url = endpoint("/voice/preview").map_err(VoicePreviewError::Failed)?;
    let mut request = agent_with_timeout(VOICE_PREVIEW_TIMEOUT)
        .post(&url)
        .header("accept", "audio/wav")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let payload = serde_json::json!({
        "presetId": preset_id.unwrap_or(""),
        "persona": persona,
    });
    let response = match request.send(payload.to_string()) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(status)) => {
            return Err(classify_preview_status(status, ""));
        }
        Err(ureq::Error::Timeout(_)) => return Err(VoicePreviewError::Timeout),
        Err(error) => return Err(VoicePreviewError::Failed(transport_failure_message(&error))),
    };
    let status = response.status().as_u16();
    let preset_id = response
        .headers()
        .get("x-voice-preset")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !(200..300).contains(&status) {
        let body = response.into_body().read_to_string().unwrap_or_default();
        return Err(classify_preview_status(status, &body));
    }
    let mut body = response.into_body();
    let bytes = body
        .read_to_vec()
        .map_err(|error| VoicePreviewError::Failed(format!("读取试听音频失败：{error}")))?;
    Ok(VoicePreviewAudio { bytes, preset_id })
}

/// 把试听接口的非 2xx 响应翻译成可执行结论。
///
/// 409 与 400 的响应体带服务端中文文案，优先原样展示；没有文案时按错误码兜底。
pub(crate) fn classify_preview_status(status: u16, body: &str) -> VoicePreviewError {
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let message = value
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty());
    let fallback = match code {
        "provider_unavailable" => Some("尚未接入云端语音提供方，请先配置阿里百炼或火山引擎"),
        "tts_failed" => Some("朗读失败，请稍后重试"),
        _ => None,
    };
    match status {
        409 => VoicePreviewError::Unavailable(
            message
                .or(fallback)
                .unwrap_or("该音色当前不可用")
                .to_owned(),
        ),
        504 => VoicePreviewError::Timeout,
        400 => {
            VoicePreviewError::Invalid(message.or(fallback).unwrap_or("试听参数不正确").to_owned())
        }
        _ => VoicePreviewError::Failed(gateway_status_message(status, body)),
    }
}

/// 语音 WebSocket 地址：与工作台 HTTP 面同端口，凭据走查询串。
///
/// 浏览器 WebSocket 不能携带握手头，因此语音上行由宿主建立连接；
/// 地址解析与凭据读取集中在这里，命令里不另拼一份。
pub fn voice_websocket_url() -> Result<String, String> {
    let http_base = endpoint::http_base();
    let ws_base = if let Some(rest) = http_base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = http_base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        endpoint::websocket_base()
    };
    let mut url = url::Url::parse(&format!("{ws_base}/api/v1/voice"))
        .map_err(|error| format!("构造语音连接地址失败：{error}"))?;
    if let Some(token) = api_token() {
        url.query_pairs_mut().append_pair("api_token", &token);
    }
    Ok(url.to_string())
}
