//! 云端语音提供方端口：阿里百炼与火山引擎 ASR/TTS 的唯一接入点。
//!
//! 识别与合成由云端提供方承担，daemon 侧只保留这个端口：协议帧、桥状态机、
//! 回合编排与停播闭环都不依赖具体提供方。接入提供方时，只要在这里返回一个
//! 实现了 [`HostTransport`] 的传输，并给出预热要请求的族与模型标识，
//! daemon 其余模块不需要改动。

use super::gateway_voice_worker::BoxedTransport;

/// 一次会话的提供方通道：传输、预热请求的族与模型标识。
pub(super) struct ProviderSession {
    /// 与提供方之间的帧传输。
    pub(super) transport: BoxedTransport,
    /// 预热请求的模型族（`vad`/`asr`/`tts`/`cue`）。
    pub(super) family: &'static str,
    /// 预热请求的模型标识，由提供方配置给出。
    pub(super) model: String,
}

/// 打开一次会话的提供方通道。
///
/// # Errors
///
/// 返回对外错误码。当前没有配置任何云端提供方，恒为 `provider_unavailable`。
pub(super) fn open() -> Result<ProviderSession, &'static str> {
    Err("provider_unavailable")
}
