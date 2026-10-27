//! 配置取值边界，唯一定义处。
//!
//! `validate()` 只引用本模块的常量，禁止在函数体里写裸数字。

/// 并发上限，本期固定 1。
pub const CONCURRENT_LIMIT: u32 = 1;
/// 允许的语言提示。
pub const LANGUAGES: [&str; 3] = ["zh", "en", "auto"];
/// 预卷时长。
pub const PRE_ROLL_MS: (u32, u32) = (600, 3_000);
/// 端点静音。
pub const ENDPOINT_SILENCE_MS: (u32, u32) = (300, 1_500);
/// VAD 起始帧数。
pub const VAD_START_FRAMES: (u32, u32) = (1, 5);
/// 打断判决块。
pub const BARGE_BLOCK_MS: (u32, u32) = (20, 40);
/// 底噪采样窗。
pub const BARGE_FLOOR_WINDOW_MS: (u32, u32) = (200, 1_000);
/// 播放宽限。
pub const BARGE_GRACE_MS: (u32, u32) = (0, 1_000);
/// 判决窗。
pub const BARGE_WINDOW_MS: (u32, u32) = (200, 600);
/// 判决比例。
pub const BARGE_WINDOW_RATIO: (f32, f32) = (0.6, 1.0);
/// 底噪倍数。
pub const BARGE_FLOOR_MULTIPLIER: (f32, f32) = (2.0, 6.0);
/// 播放中阈值下限。
pub const BARGE_MIN_TRIGGER: (u32, u32) = (800, 3_000);
/// 阈值上限。
pub const BARGE_MAX_TRIGGER: (u32, u32) = (3_000, 8_000);
/// 分句最小字数。
pub const SENTENCE_MIN_CHARS: (u32, u32) = (1, 40);
/// ack 触发阈值。
pub const ACK_AFTER_MS: (u32, u32) = (200, 2_000);
/// 思考音触发阈值。
pub const THINKING_AFTER_MS: (u32, u32) = (100, 2_000);
/// ack 冷却。
pub const ACK_COOLDOWN_MS: (u32, u32) = (0, 60_000);
/// 宿主空闲退出。
pub const IDLE_TIMEOUT_SECONDS: (u32, u32) = (60, 3_600);
/// 宿主重启上限。
pub const MAX_RESTART: (u32, u32) = (0, 10);
