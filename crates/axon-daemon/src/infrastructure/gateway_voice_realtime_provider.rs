//! Realtime 车道的提供方协议适配器。
//!
//! 车道在构造期按 `channel.protocol` 选定实现（分派点只在 `spawn_realtime` 一处），
//! 会话循环用泛型单态化，热路径既没有 trait 对象分派，也没有厂商字符串匹配。
//! 厂商差异全部收在实现内部：握手参数、`session.update` 字段、上下行编解码。

use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::protocol::Message;

/// 上行控制事件：与提供方无关的四种语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RealtimeControl {
    /// 手动断句；服务端 VAD 车道下通常被忽略。
    Commit,
    /// 丢弃尚未提交的上行音频。
    Clear,
    /// 取消当前回复。
    Cancel,
    /// 正常收线。
    Finish,
}

/// 下行规范化事件：会话循环只认这些形状。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum RealtimeEvent {
    /// 连接建立，携带会话标识。
    SessionCreated,
    /// 配置生效；到这一刻才能开始送音频。
    SessionUpdated,
    /// 服务端 VAD 判定语音开始。
    SpeechStarted,
    /// 服务端 VAD 判定语音结束。
    SpeechStopped,
    /// 输入转写增量：`text` 是已确定前缀，`stash` 是未确定尾部。
    InputTranscriptDelta {
        /// 已确定前缀。
        text: String,
        /// 未确定尾部。
        stash: String,
    },
    /// 输入转写定稿，权威结果。
    InputTranscriptCompleted {
        /// 定稿文本。
        transcript: String,
    },
    /// 一轮回复开始；新一轮播放从这里分配轮次。
    ResponseCreated,
    /// 助手文字增量。
    OutputTranscriptDelta {
        /// 增量文本。
        text: String,
    },
    /// 助手文字定稿；服务端没给时为空。
    OutputTranscriptDone {
        /// 定稿文本。
        transcript: String,
    },
    /// 助手音频分片，提供方**原生采样率**的 s16le。
    AudioDelta {
        /// 未重采样的 PCM。
        pcm: Vec<u8>,
    },
    /// 一轮回复结束。
    ResponseDone,
    /// 服务端错误。
    Error {
        /// 稳定错误码。
        code: String,
        /// 服务端给出的说明。
        message: String,
    },
    /// 不需要消费的事件。
    Ignored,
}

/// 提供方协议适配器。
///
/// 只在构造期做一次性分派；会话循环持有具体类型，每帧走静态调用。
pub(super) trait RealtimeProvider: Send + Sync {
    /// 握手请求：端点、鉴权头与查询参数。
    ///
    /// # Errors
    ///
    /// 端点非法或鉴权头无法构造时返回稳定错误码。
    fn handshake_request(&self) -> Result<Request, String>;

    /// `session.update` 事件文本。
    fn session_update(&self) -> String;

    /// 上行 PCM 按协议分片编码，追加到 `out`。
    ///
    /// 调用方复用同一个 `Vec`，实现内部不新建集合，避免每帧分配。
    fn encode_audio(&self, pcm: &[u8], out: &mut Vec<Message>);

    /// 控制事件编码。
    fn encode_control(&self, control: RealtimeControl) -> Message;

    /// 下行文本帧解码为规范化事件；脏帧一律 `Ignored`。
    fn decode_event(&self, text: &str) -> RealtimeEvent;

    /// 下行原生 PCM 换算到 16 kHz s16le。
    ///
    /// 有状态：跨分片的余量留在实现里，分片边界不产生接缝。
    fn resample_downlink(&mut self, pcm: &[u8]) -> Vec<u8>;
}
