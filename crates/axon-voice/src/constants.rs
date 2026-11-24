//! 音色与韵律的硬边界。
//!
//! 这些数值是跨进程契约的一部分（`presetApplied` 与预设清单都按此校验），
//! 只允许在此处定义一次，其它模块必须引用本文件。

/// 语速下限（倍率）。
pub const RATE_MIN: f32 = 0.85;
/// 语速上限（倍率）。
pub const RATE_MAX: f32 = 1.15;
/// 音高下限（半音）。
pub const PITCH_MIN_SEMITONES: f32 = -3.0;
/// 音高上限（半音）。
pub const PITCH_MAX_SEMITONES: f32 = 3.0;
/// 句间停顿缩放下限。
pub const PAUSE_SCALE_MIN: f32 = 0.8;
/// 句间停顿缩放上限。
pub const PAUSE_SCALE_MAX: f32 = 1.4;
/// 音量增益下限（dB）。
pub const ENERGY_DB_MIN: f32 = -3.0;
/// 音量增益上限（dB）。
pub const ENERGY_DB_MAX: f32 = 1.0;
/// 句尾音高偏移下限（半音）。
pub const TAIL_MIN_SEMITONES: f32 = -1.0;
/// 句尾音高偏移上限（半音）。
pub const TAIL_MAX_SEMITONES: f32 = 1.0;
/// 打断判决块大小（毫秒），由 20 ms 采集帧聚成。
pub const BARGE_BLOCK_MS: u32 = 30;
/// 单条 cue 的文本上限（字符）。
pub const CUE_TEXT_MAX_CHARS: usize = 24;
