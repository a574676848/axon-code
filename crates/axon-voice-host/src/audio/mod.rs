//! 音频输入：WAV 解析与规范化。

pub mod wav;

pub use wav::{WavClip, WavError, parse};
