//! 记忆代谢（M6）：merge-only 整合器与后置校验。
//!
//! 设计定稿：整合器**只允许**两种动作——
//! * `merge`：保留较新的一条，把同主题旧条归档并写入 `memory_supersessions` 追溯边；
//! * `downgrade`：下调 `importance_basis_points`，下限 `MIN_IMPORTANCE_BP`。
//!
//! 模型输出若出现「新建 / 插入」意图，整批丢弃并返回拒绝原因，库不变。

mod lesson;
mod person;

pub use lesson::{
    LESSON_NORMAL_MAX_DAYS, LESSON_STALE_MAX_DAYS, LESSON_STALE_WEIGHT_BP, LessonFreshness,
    effective_importance_bp, is_lesson_kind, lesson_freshness, should_inject_actively,
};
pub use person::{PERSON_CARD_TAG, is_person_card_like, profile_inference_inputs};

use axon_domain::{AxonError, AxonResult};

use crate::MemorySqliteStore;

/// 单批整合的最大条数。
pub const CONSOLIDATE_BATCH_LIMIT: usize = 20;
/// 降级后的重要度下限。
pub const MIN_IMPORTANCE_BP: u16 = 1_000;
/// 记忆整合的默认状态：默认开启。
///
/// 需要停用时把该常量改为 `false` 并重编，或在调用点显式传入 `false`。
pub const CONSOLIDATE_ENABLED_DEFAULT: bool = true;

/// 一条整合动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsolidationOp {
    /// 合并：保留 `keep`，吸收 `absorb` 中的旧条。
    Merge {
        /// 保留的记忆 ID。
        keep: u64,
        /// 被吸收（归档并建立替代边）的旧记忆 ID。
        absorb: Vec<u64>,
    },
    /// 降级：下调目标记忆的重要度。
    Downgrade {
        /// 目标记忆 ID。
        target: u64,
    },
}

/// 一批整合动作。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidationPlan {
    /// 动作列表。
    pub ops: Vec<ConsolidationOp>,
}

/// 计划被拒绝的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanRejection {
    /// 模型输出含新建/插入意图。
    NewMemoryIntent,
    /// 结构不合法。
    Malformed(String),
    /// 没有任何有效动作。
    Empty,
    /// 动作引用了本批之外的记忆。
    UnknownTarget(u64),
}

/// 整合执行结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    /// 合并组数。
    pub merged_groups: usize,
    /// 被吸收的旧记忆条数。
    pub absorbed: usize,
    /// 被降级的条数。
    pub downgraded: usize,
}

/// 解析模型给出的整合计划，并做后置校验。
///
/// 只要出现 `new` / `insert` / `create` 动作，整批丢弃（`NewMemoryIntent`）。
pub fn parse_plan(raw: &str, allowed_ids: &[u64]) -> Result<ConsolidationPlan, PlanRejection> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| PlanRejection::Malformed(error.to_string()))?;
    let raw_ops = value
        .get("ops")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| PlanRejection::Malformed("缺少 ops 数组".to_owned()))?;
    let mut ops = Vec::new();
    for raw_op in raw_ops {
        let action = raw_op
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match action.as_str() {
            "merge" => {
                let keep = raw_op
                    .get("keep")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| PlanRejection::Malformed("merge 缺少 keep".to_owned()))?;
                let absorb = raw_op
                    .get("absorb")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| PlanRejection::Malformed("merge 缺少 absorb".to_owned()))?
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .filter(|id| *id != keep)
                    .collect::<Vec<_>>();
                if !allowed_ids.contains(&keep) {
                    return Err(PlanRejection::UnknownTarget(keep));
                }
                for id in &absorb {
                    if !allowed_ids.contains(id) {
                        return Err(PlanRejection::UnknownTarget(*id));
                    }
                }
                if !absorb.is_empty() {
                    ops.push(ConsolidationOp::Merge { keep, absorb });
                }
            }
            "downgrade" => {
                let target = raw_op
                    .get("target")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| PlanRejection::Malformed("downgrade 缺少 target".to_owned()))?;
                if !allowed_ids.contains(&target) {
                    return Err(PlanRejection::UnknownTarget(target));
                }
                ops.push(ConsolidationOp::Downgrade { target });
            }
            "new" | "insert" | "create" => return Err(PlanRejection::NewMemoryIntent),
            other => {
                return Err(PlanRejection::Malformed(format!("未知动作 {other}")));
            }
        }
    }
    if ops.is_empty() {
        return Err(PlanRejection::Empty);
    }
    Ok(ConsolidationPlan { ops })
}

/// 执行整合计划；`enabled = false` 时不做任何写入。
pub fn apply_plan(
    store: &MemorySqliteStore,
    plan: &ConsolidationPlan,
    now_ms: u64,
    enabled: bool,
) -> AxonResult<ConsolidationReport> {
    if !enabled {
        return Ok(ConsolidationReport::default());
    }
    let mut report = ConsolidationReport::default();
    for op in &plan.ops {
        match op {
            ConsolidationOp::Merge { keep, absorb } => {
                for old_id in absorb {
                    store.archive_memory(*old_id, now_ms)?;
                    store.insert_supersession(*old_id, *keep, now_ms)?;
                    report.absorbed += 1;
                }
                report.merged_groups += 1;
            }
            ConsolidationOp::Downgrade { target } => {
                if store.downgrade_importance(*target)? {
                    report.downgraded += 1;
                }
            }
        }
    }
    Ok(report)
}

impl MemorySqliteStore {
    /// 归档一条记忆（幂等：已归档则返回 `false`）。
    pub fn archive_memory(&self, memory_id: u64, now_ms: u64) -> AxonResult<bool> {
        let changed = self
            .connection
            .execute(
                "UPDATE memory_records SET archived_at_ms = ?2 \
                 WHERE memory_id = ?1 AND archived_at_ms IS NULL",
                rusqlite::params![
                    i64::try_from(memory_id).unwrap_or(i64::MAX),
                    i64::try_from(now_ms).unwrap_or(i64::MAX)
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 归档失败"))?;
        Ok(changed > 0)
    }

    /// 写入一条「旧 → 新」替代边（幂等）。
    pub fn insert_supersession(
        &self,
        predecessor: u64,
        successor: u64,
        now_ms: u64,
    ) -> AxonResult<()> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO memory_supersessions \
                 (predecessor_memory_id, successor_memory_id, created_at_ms) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    i64::try_from(predecessor).unwrap_or(i64::MAX),
                    i64::try_from(successor).unwrap_or(i64::MAX),
                    i64::try_from(now_ms).unwrap_or(i64::MAX)
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 替代边写入失败"))?;
        Ok(())
    }

    /// 下调重要度，下限 `MIN_IMPORTANCE_BP`；返回是否发生变化。
    pub fn downgrade_importance(&self, memory_id: u64) -> AxonResult<bool> {
        let changed = self
            .connection
            .execute(
                "UPDATE memory_records SET importance_basis_points = \
                 MAX(importance_basis_points / 2, ?2) \
                 WHERE memory_id = ?1 AND importance_basis_points > ?2",
                rusqlite::params![
                    i64::try_from(memory_id).unwrap_or(i64::MAX),
                    i64::from(MIN_IMPORTANCE_BP)
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 重要度降级失败"))?;
        Ok(changed > 0)
    }

    /// 命名空间内仍在生效（未归档）的记忆条数。
    pub fn active_memory_count(&self, namespace_ref: &str) -> AxonResult<usize> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM memory_records \
                 WHERE namespace_ref = ?1 AND archived_at_ms IS NULL",
                rusqlite::params![namespace_ref],
                |row| row.get(0),
            )
            .map_err(|_| AxonError::memory_persistence("Memory 生效条数统计失败"))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// 某条记忆的替代前驱列表（用于追溯校验）。
    pub fn supersession_predecessors(&self, successor: u64) -> AxonResult<Vec<u64>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT predecessor_memory_id FROM memory_supersessions \
                 WHERE successor_memory_id = ?1 ORDER BY predecessor_memory_id ASC",
            )
            .map_err(|_| AxonError::memory_persistence("Memory 替代边读取失败"))?;
        let rows = statement
            .query_map(
                rusqlite::params![i64::try_from(successor).unwrap_or(i64::MAX)],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| AxonError::memory_persistence("Memory 替代边读取失败"))?;
        rows.map(|row| {
            row.map(|value| u64::try_from(value).unwrap_or(0))
                .map_err(|_| AxonError::memory_persistence("Memory 替代边读取失败"))
        })
        .collect()
    }
}

#[cfg(test)]
#[path = "consolidate_tests.rs"]
mod tests;
