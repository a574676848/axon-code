//! 桌面端语音：音色试听。
//!
//! 试听与朗读共用网关的语音接口，但试听是一次短请求，失败分类更细：
//! 504 是超时、409 是运行时或音色不可用、400 是请求体不合法，三者给用户的
//! 下一步动作完全不同，因此在宿主侧先分好类，界面只负责展示结论。
//!
//! 音频只在内存里交给前端播放，不落盘；试听不写任何配置，也不影响对话。

use serde::Serialize;

use crate::gateway;

/// 试听回执：base64 音频、MIME 与网关实际采用的音色。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicePreviewPayload {
    /// base64 编码的 WAV 字节。
    pub audio_base64: String,
    /// MIME 类型，固定 `audio/wav`。
    pub mime_type: String,
    /// 网关响应头返回的实际音色；请求跟随人格时也由它给出结论。
    pub preset_id: String,
}

/// 试听指定音色；`preset_id` 为空表示跟随人格。
///
/// 合成在阻塞池里执行：试听最长可等 10 s，占住主线程会让整个窗口卡住。
///
/// # Errors
///
/// 参数为空、网关不可达、语音运行时不可用或合成超时。
#[tauri::command]
pub async fn voice_preview(preset_id: Option<String>) -> Result<VoicePreviewPayload, String> {
    tauri::async_runtime::spawn_blocking(move || preview_blocking(preset_id))
        .await
        .map_err(|error| format!("试听任务失败：{error}"))?
}

/// 试听的同步实现；调试桥与命令通道共用这一份。
pub(crate) fn preview_blocking(preset_id: Option<String>) -> Result<VoicePreviewPayload, String> {
    use base64::Engine as _;

    let requested = preset_id.filter(|id| !id.is_empty());
    let persona = crate::ipc::persona_payload();
    let audio =
        gateway::post_voice_preview(requested.as_deref(), persona.as_ref()).map_err(|error| {
            match error {
                gateway::VoicePreviewError::Timeout => {
                    "preview_timeout: 试听超时，请重试或换一个音色".to_owned()
                }
                gateway::VoicePreviewError::Unavailable(reason) => {
                    format!("voice_unavailable: {reason}")
                }
                gateway::VoicePreviewError::Invalid(reason) => format!("preview_invalid: {reason}"),
                gateway::VoicePreviewError::Failed(reason) => reason,
            }
        })?;
    let preset_id = if audio.preset_id.is_empty() {
        requested.unwrap_or_default()
    } else {
        audio.preset_id
    };
    Ok(VoicePreviewPayload {
        audio_base64: base64::engine::general_purpose::STANDARD.encode(audio.bytes),
        mime_type: "audio/wav".to_owned(),
        preset_id,
    })
}
