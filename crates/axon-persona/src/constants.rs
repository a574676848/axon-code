//! 人格系统的全部阈值与常量。
//!
//! 所有数值集中于此，禁止在业务代码中硬编码，也不新增配置键。

/// `SOUL.md` 允许的最大字节数。
pub const SOUL_DOCUMENT_MAX_BYTES: usize = 64 * 1024;

/// 当前支持的 `SOUL.md` 格式标识。
pub const SOUL_SCHEMA_V1: &str = "axon-soul/v1";

/// 稳定段渲染允许的最大字节数。
pub const STABLE_RENDER_MAX_BYTES: usize = 6144;

/// 稳定段软门槛：越线即预警，硬上限仍是 [STABLE_RENDER_MAX_BYTES]。
pub const STABLE_RENDER_SOFT_BYTES: usize = 4096;

/// 动态段渲染允许的最大字节数。
pub const DYNAMIC_RENDER_MAX_BYTES: usize = 1024;

/// 温度偏移的绝对值上限，避免状态无限漂移。
pub const WARMTH_OFFSET_CLAMP: i8 = 20;

/// 温度偏移的衰减半衰期，单位毫秒，取 6 小时。
pub const WARMTH_HALF_LIFE_MS: u64 = 6 * 60 * 60 * 1_000;

/// 衰减最多折算的次数上限，避免超大间隔下无界循环。
pub const WARMTH_DECAY_MAX_HALVINGS: u32 = 16;

/// 熟悉度上限。
pub const FAMILIARITY_MAX: u8 = 100;

/// 连续失败或中断达到该次数即进入 incident 模式。
pub const INCIDENT_FAILURE_STREAK: u8 = 2;

/// 表达漂移触发下一轮纠偏的阈值，单位 basis point。
pub const PERSONA_DRIFT_GUARD_BP: u16 = 7_000;

/// 用户反馈积极时的温度增量。
pub const WARMTH_DELTA_POSITIVE: i8 = 5;

/// 用户受挫时的温度增量，先致意再解决问题。
pub const WARMTH_DELTA_FRUSTRATED: i8 = 3;

/// 视为「久别」的空闲间隔，单位毫秒，取 72 小时。
pub const REUNION_IDLE_MS: u64 = 72 * 60 * 60 * 1_000;

/// 允许久别致意所需的最低熟悉度。
pub const REUNION_MIN_FAMILIARITY: u8 = 20;

/// 群聊场景下的正式度加成。
pub const GROUP_FORMALITY_BONUS: u8 = 20;

/// 稳定段渲染允许的已发布习得风格条数上限。
pub const MAX_PUBLISHED_STYLE: usize = 16;

/// 单条习得风格的字符上限。
pub const MAX_LEARNED_STYLE_CHARS: usize = 120;

/// 习得风格两次发布之间的最小间隔，单位毫秒，取 24 小时。
pub const STYLE_PUBLISH_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;

/// 每个 owner 保留的人格转移日志条数上限。
pub const TRANSITION_RETENTION: u32 = 500;

/// 结算写入的并发冲突重试上限。
pub const SETTLEMENT_RETRY_LIMIT: u32 = 3;

/// 人格装配的降级阈值，单位毫秒；超过即放弃动态段。
pub const PERSONA_ASSEMBLY_BUDGET_MS: u64 = 50;

/// 轮前只读查询的 SQLite busy 等待上限；远小于结算路径，避免拖住首字。
pub const PERSONA_READ_BUSY_TIMEOUT_MS: u64 = 200;

/// 结算写路径的 SQLite busy 等待上限；轮前只读路径见 `PERSONA_READ_BUSY_TIMEOUT_MS`。
pub const PERSONA_WRITE_BUSY_TIMEOUT_MS: u64 = 2_000;
