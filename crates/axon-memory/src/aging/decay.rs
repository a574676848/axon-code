//! 记忆衰减纯函数。
//!
//! 只依赖记录自身的置信度、重要度与最近活跃时间，不读写存储，可在任意调用点重复求值。

use crate::MemorySqliteRecord;

/// 记忆衰减半衰期，单位天；治理、合并与界面展示共用同一口径。
pub const MEMORY_DEFAULT_HALF_LIFE_DAYS: u64 = 30;

/// basis point 满分。
pub const MAX_BASIS_POINTS: u16 = 10_000;

const SECONDS_PER_DAY: u64 = 86_400;

/// 计算衰减后的有效权重（basis point）：置信度与重要度之积按半衰期指数衰减。
#[must_use]
pub fn decayed_score_basis_points(record: &MemorySqliteRecord, now_ms: u64) -> u16 {
    let elapsed_ms =
        std::time::Duration::from_millis(now_ms.saturating_sub(record.last_active_at_ms))
            .as_secs_f64()
            * 1_000.0;
    let half_life_ms = std::time::Duration::from_secs(
        MEMORY_DEFAULT_HALF_LIFE_DAYS.saturating_mul(SECONDS_PER_DAY),
    )
    .as_secs_f64()
        * 1_000.0;
    let base = f64::from(record.confidence_basis_points)
        * f64::from(record.importance_basis_points)
        / f64::from(MAX_BASIS_POINTS);
    let decay = (-std::f64::consts::LN_2 * elapsed_ms / half_life_ms).exp();
    rounded_basis_points(base * decay)
}

fn rounded_basis_points(score: f64) -> u16 {
    let target = score.round().clamp(0.0, f64::from(MAX_BASIS_POINTS));
    let mut lower = 0_u16;
    let mut upper = MAX_BASIS_POINTS;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        if f64::from(middle) < target {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }
    lower
}
