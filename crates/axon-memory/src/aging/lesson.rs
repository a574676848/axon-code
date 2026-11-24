//! 教训类记忆保质期。
//!
//! 检索与主动注入按 `updated_at_ms` 分级：不超过 180 天正常；180 到 365 天重要度折半；
//! 超过 365 天不再主动注入（仍可被 `memory_search` 检索到）。

use axon_domain::MemoryKind;

/// 正常注入的上限天数。
pub const LESSON_NORMAL_MAX_DAYS: i64 = 180;
/// 半权重注入的上限天数。
pub const LESSON_STALE_MAX_DAYS: i64 = 365;

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
    fn four_hundred_day_old_lesson_is_expired() {
        let now = 1_000 * DAY;
        let updated = now - 400 * DAY;
        assert_eq!(
            lesson_freshness(MemoryKind::Procedure, updated, now),
            LessonFreshness::Expired
        );
    }

    #[test]
    fn non_lesson_kind_is_never_expired() {
        let now = 1_000 * DAY;
        assert_eq!(
            lesson_freshness(MemoryKind::Knowledge, now - 900 * DAY, now),
            LessonFreshness::Fresh
        );
        assert_eq!(
            lesson_freshness(MemoryKind::Preference, now - 900 * DAY, now),
            LessonFreshness::Fresh
        );
    }
}
