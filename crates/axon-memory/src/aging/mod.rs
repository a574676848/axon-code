//! 记忆代谢策略层：时效衰减与保质期的唯一判定出口。
//!
//! 本模块只做纯函数判定，不读写存储、不持有状态。调用方按用途取用：
//! - 检索候选排序用 [`injection_rank`]；
//! - 定时治理用 [`should_archive`]；
//! - 各处"是否可召回"统一用 [`is_active_recallable`]。

mod decay;
mod lesson;

pub use decay::{MAX_BASIS_POINTS, MEMORY_DEFAULT_HALF_LIFE_DAYS, decayed_score_basis_points};
pub use lesson::{
    LESSON_NORMAL_MAX_DAYS, LESSON_STALE_MAX_DAYS, LessonFreshness, is_lesson_kind,
    lesson_freshness,
};

use axon_domain::MemoryState;

use crate::MemorySqliteRecord;

/// 低活性记录归档阈值，单位 basis point。
pub const MEMORY_ARCHIVE_SCORE_THRESHOLD_BASIS_POINTS: u16 = 1_500;

/// 记录是否仍活跃且可被召回。
///
/// 这是全仓唯一实现：定时治理、质量统计与检索候选都以此为准，禁止各自复制一份。
#[must_use]
pub fn is_active_recallable(record: &MemorySqliteRecord) -> bool {
    record.archived_at_ms.is_none()
        && matches!(record.record.state, MemoryState::Weak | MemoryState::Strong)
        && record
            .record
            .content_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}

/// 综合时效分数：活跃度衰减后，再按 kind 保质期折价。
#[must_use]
pub fn aging_score_basis_points(record: &MemorySqliteRecord, now_ms: u64) -> u16 {
    let decayed = decayed_score_basis_points(record, now_ms);
    match lesson_freshness(record.memory_kind, record.updated_at_ms, now_ms) {
        LessonFreshness::Fresh => decayed,
        LessonFreshness::HalfWeight => decayed / 2,
        LessonFreshness::Expired => 0,
    }
}

/// 检索候选排序键：综合时效分数优先，其次更新时间与 id，保证全序稳定。
#[must_use]
pub fn injection_rank(record: &MemorySqliteRecord, now_ms: u64) -> (u16, u64, u64) {
    (
        aging_score_basis_points(record, now_ms),
        record.updated_at_ms,
        record.record.id.0,
    )
}

/// 定时治理是否应归档：仅在记录仍活跃时判定，避免对已归档记录重复记账。
///
/// 判定口径与衰减分数一致，不含 kind 保质期折价，保证治理行为可预测。
#[must_use]
pub fn should_archive(record: &MemorySqliteRecord, now_ms: u64) -> bool {
    is_active_recallable(record)
        && decayed_score_basis_points(record, now_ms) < MEMORY_ARCHIVE_SCORE_THRESHOLD_BASIS_POINTS
}

#[cfg(test)]
#[path = "aging_tests.rs"]
mod tests;
