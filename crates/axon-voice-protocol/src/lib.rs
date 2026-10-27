//! daemon 与语音宿主之间的管道协议。
//!
//! 分帧为 `[u8 kind][u32be len][payload]`；控制帧是 JSON（≤128 KiB，拒绝未知字段），
//! 音频帧是裸 s16le 或带轮次/序号头。字段与 `PROTOCOL.md` §3 逐项对应。
//!
//! 本 crate 只做编解码，不含 IO 循环与业务状态。

pub mod control;
pub mod frame;

pub use control::{
    Cancel, Control, CueSpeak, CueStatus, ErrorDetail, ErrorMsg, Final, Heartbeat, Hello, Partial,
    Ready, SessionEnd, SessionStart, Speak, Warm, Warmup,
};
pub use frame::{
    Frame, FrameError, decode_frame, encode_audio_in, encode_audio_out, encode_cue_audio,
    read_frame, write_bytes, write_frame,
};

/// 协议版本，固定 1。
pub const PROTOCOL_VERSION: u32 = 1;
/// 单个控制帧的字节上限。
pub const MAX_FRAME_BYTES: usize = 128 * 1024;
/// 控制帧 kind。
pub const KIND_CONTROL: u8 = 0x10;
/// 上行音频 kind。
pub const KIND_AUDIO_IN: u8 = 0x03;
/// 对话音频 kind。
pub const KIND_AUDIO_OUT: u8 = 0x04;
/// cue 音频 kind。
pub const KIND_CUE_AUDIO: u8 = 0x05;

/// 控制消息类型名。
pub mod type_name {
    /// 父进程问候。
    pub const HELLO: &str = "hello";
    /// 宿主就绪。
    pub const READY: &str = "ready";
    /// 预热请求。
    pub const WARMUP: &str = "warmup";
    /// 预热完成。
    pub const WARM: &str = "warm";
    /// 开始会话。
    pub const SESSION_START: &str = "sessionStart";
    /// 结束会话。
    pub const SESSION_END: &str = "sessionEnd";
    /// 中间识别结果。
    pub const PARTIAL: &str = "partial";
    /// 定稿。
    pub const FINAL: &str = "final";
    /// 合成播报。
    pub const SPEAK: &str = "speak";
    /// 合成 cue。
    pub const CUE_SPEAK: &str = "cueSpeak";
    /// cue 缓存状态。
    pub const CUE_STATUS: &str = "cueStatus";
    /// 取消。
    pub const CANCEL: &str = "cancel";
    /// 心跳。
    pub const HEARTBEAT: &str = "heartbeat";
    /// 错误。
    pub const ERROR: &str = "error";
    /// 关闭请求。
    pub const CLOSE: &str = "close";
    /// 关闭确认。
    pub const CLOSED: &str = "closed";
}
