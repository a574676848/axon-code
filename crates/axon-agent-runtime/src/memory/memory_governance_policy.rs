//! Memory 生命周期治理共享策略。

/// 低活性记录归档阈值，单位 basis point。
pub(crate) const MEMORY_ARCHIVE_SCORE_THRESHOLD_BASIS_POINTS: u16 = 1_500;
