//! 教训类记忆保质期（M6-2）。
//!
//! 主动注入按 `updated_at_ms` 分级：≤180d 正常；180–365d 重要度 ×0.5；>365d 不主动注入
//! （仍可被 `memory_search` 检索到）。

use axon_domain::MemoryKind;

/// 正常注入的上限天数。
pub const LESSON_NORMAL_MAX_DAYS: i64 = 180;
/// 半权重注入的上限天数。
pub const LESSON_STALE_MAX_DAYS: i64 = 365;
/// 半权重系数（basis point）。
pub const LESSON_STALE_WEIGHT_BP: u16 = 5_000;

const MS_PER_DAY: i64 = 86_400_000;

/// 教训类记忆的保质期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LessonFreshness {
    /// 正常注入。
    Fresh,
    /// 重要度折半后注入。
    HalfWeight,
    /// 不再主动注入。
    Expired,
}

/// 是否为教训类记忆。
///
/// 说明：仓库实际的 `MemoryKind` 只有 Profile/Preference/Experience/Knowledge/Procedure 五值，
/// 没有 `Constraint` 与 `FailureLesson` 变体；这里以 `Procedure` 与 `Experience` 作为教训类。
#[must_use]
pub const fn is_lesson_kind(kind: MemoryKind) -> bool {
    matches!(kind, MemoryKind::Procedure | MemoryKind::Experience)
}

/// 计算保质期状态。
#[must_use]
pub fn lesson_freshness(kind: MemoryKind, updated_at_ms: u64, now_ms: u64) -> LessonFreshness {
    if !is_lesson_kind(kind) {
        return LessonFreshness::Fresh;
    }
    let age_days = now_ms.saturating_sub(updated_at_ms) as i64 / MS_PER_DAY;
    if age_days <= LESSON_NORMAL_MAX_DAYS {
        LessonFreshness::Fresh
    } else if age_days <= LESSON_STALE_MAX_DAYS {
        LessonFreshness::HalfWeight
    } else {
        LessonFreshness::Expired
    }
}

/// 是否允许主动注入（P2 线索/教训注入用）。
#[must_use]
pub fn should_inject_actively(kind: MemoryKind, updated_at_ms: u64, now_ms: u64) -> bool {
    !matches!(
        lesson_freshness(kind, updated_at_ms, now_ms),
        LessonFreshness::Expired
    )
}

/// 按保质期折算后的重要度。
#[must_use]
pub fn effective_importance_bp(importance_bp: u16, freshness: LessonFreshness) -> u16 {
    match freshness {
        LessonFreshness::Fresh => importance_bp,
        LessonFreshness::HalfWeight => {
            u16::try_from(u32::from(importance_bp) * u32::from(LESSON_STALE_WEIGHT_BP) / 10_000)
                .unwrap_or(0)
        }
        LessonFreshness::Expired => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    #[test]
    fn fresh_lesson_within_normal_window() {
        let now = 1_000 * DAY;
        assert_eq!(
            lesson_freshness(MemoryKind::Procedure, now - 10 * DAY, now),
            LessonFreshness::Fresh
        );
    }

    #[test]
    fn boundary_at_180_days_is_still_fresh() {
        let now = 1_000 * DAY;
        assert_eq!(
            lesson_freshness(MemoryKind::Procedure, now - 180 * DAY, now),
            LessonFreshness::Fresh
        );
    }

    #[test]
    fn lesson_between_180_and_365_days_is_half_weight() {
        let now = 1_000 * DAY;
        assert_eq!(
            lesson_freshness(MemoryKind::Experience, now - 200 * DAY, now),
            LessonFreshness::HalfWeight
        );
        assert_eq!(
            lesson_freshness(MemoryKind::Experience, now - 365 * DAY, now),
            LessonFreshness::HalfWeight
        );
    }

    #[test]
    fn four_hundred_day_old_lesson_is_not_injected_but_still_retrievable() {
        let now = 1_000 * DAY;
        let updated = now - 400 * DAY;
        assert_eq!(
            lesson_freshness(MemoryKind::Procedure, updated, now),
            LessonFreshness::Expired
        );
        assert!(!should_inject_actively(MemoryKind::Procedure, updated, now));
    }

    #[test]
    fn non_lesson_kind_is_never_expired() {
        let now = 1_000 * DAY;
        assert_eq!(
            lesson_freshness(MemoryKind::Knowledge, now - 900 * DAY, now),
            LessonFreshness::Fresh
        );
        assert!(should_inject_actively(
            MemoryKind::Preference,
            now - 900 * DAY,
            now
        ));
    }

    #[test]
    fn half_weight_halves_importance() {
        assert_eq!(
            effective_importance_bp(8_000, LessonFreshness::HalfWeight),
            4_000
        );
        assert_eq!(
            effective_importance_bp(8_000, LessonFreshness::Fresh),
            8_000
        );
        assert_eq!(effective_importance_bp(8_000, LessonFreshness::Expired), 0);
    }
}
