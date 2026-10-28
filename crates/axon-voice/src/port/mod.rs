//! 端口：识别、合成、朗读渲染与时钟。
//!
//! 端口只描述能力，不含实现；本机模型已移除，识别与合成由云端提供方适配器
//! （阿里百炼、火山引擎）实现，协议与播放在 daemon/desktop 中实现。

use crate::config::AsrConfig;
use crate::domain::{SpeechProfile, VoiceFailure, cue::CueKind};

/// 协议固定的采样率。
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// 单调时钟。
pub trait Clock {
    /// 当前毫秒。
    fn now_ms(&self) -> u64;
}

/// 识别事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEvent {
    /// 中间结果，可被覆盖。
    Partial {
        /// 句序号，会话内从 1 起。
        utterance_seq: u32,
        /// 文本。
        text: String,
    },
    /// 定稿。
    Final {
        /// 句序号。
        utterance_seq: u32,
        /// 文本。
        text: String,
        /// 端点判定时刻（宿主单调毫秒）。
        end_ms: u64,
        /// 定稿产出时刻（宿主单调毫秒）。
        finalize_ms: u64,
    },
}

/// 一段 16 kHz 单声道 PCM。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmChunk {
    samples: Vec<i16>,
}

impl PcmChunk {
    /// 构造 PCM 片段。
    ///
    /// # Errors
    ///
    /// 样本为空时返回 [`VoiceFailure::Internal`]；采样率由协议固定为 16 kHz。
    pub fn new(samples: Vec<i16>) -> Result<Self, VoiceFailure> {
        if samples.is_empty() {
            return Err(VoiceFailure::Internal {
                detail: "pcm chunk is empty".to_owned(),
            });
        }
        Ok(Self { samples })
    }

    /// 采样率，恒为 16 kHz。
    #[must_use]
    pub const fn sample_rate_hz(&self) -> u32 {
        SAMPLE_RATE_HZ
    }

    /// 只读样本。
    #[must_use]
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// 时长（毫秒，向下取整）。
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        (self.samples.len() as u64) * 1_000 / u64::from(SAMPLE_RATE_HZ)
    }
}

/// 流式识别端口。
pub trait SpeechToText {
    /// 打开一句的识别会话。
    ///
    /// # Errors
    ///
    /// 模型缺失或不可用时返回对应失败。
    fn open(&self, cfg: &AsrConfig) -> Result<Box<dyn SttSession>, VoiceFailure>;
}

/// 单句识别会话。
pub trait SttSession {
    /// 送入一帧 16 kHz 单声道样本。
    ///
    /// # Errors
    ///
    /// 队列溢出或引擎不可用时返回对应失败。
    fn push(&mut self, samples: &[i16]) -> Result<Vec<SttEvent>, VoiceFailure>;

    /// 最近一次中间结果。
    fn partial(&self) -> Option<&str>;

    /// 结束并定稿。
    ///
    /// # Errors
    ///
    /// 定稿失败时返回对应失败。
    fn finalize(&mut self) -> Result<SttEvent, VoiceFailure>;
}

/// 合成端口。
pub trait TextToSpeech {
    /// 合成一段已渲染的口播文本。
    ///
    /// # Errors
    ///
    /// 模型缺失、文本超限或合成失败时返回对应失败。
    fn synthesize(&self, text: &str, profile: &SpeechProfile) -> Result<PcmChunk, VoiceFailure>;

    /// 合成一条 cue。
    ///
    /// # Errors
    ///
    /// cue 不可用时返回 [`VoiceFailure::CueUnavailable`]。
    fn synthesize_cue(&self, kind: CueKind, text: &str) -> Result<PcmChunk, VoiceFailure>;
}

/// 朗读稿渲染端口。
pub trait SpeechRenderer {
    /// 送入增量文本，返回已可朗读的整句。
    fn feed(&mut self, delta: &str) -> Vec<String>;

    /// 冲刷尾部。
    fn flush(&mut self) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::{PcmChunk, SAMPLE_RATE_HZ};
    use crate::domain::VoiceFailure;

    #[test]
    fn empty_chunk_is_rejected() {
        assert!(matches!(
            PcmChunk::new(Vec::new()),
            Err(VoiceFailure::Internal { .. })
        ));
    }

    #[test]
    fn duration_is_derived_from_samples() {
        let chunk = PcmChunk::new(vec![0; 16_000]).expect("构造应成功");
        assert_eq!(chunk.sample_rate_hz(), SAMPLE_RATE_HZ);
        assert_eq!(chunk.duration_ms(), 1_000);
    }
}
