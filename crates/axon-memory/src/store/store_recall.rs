//! SQLite Memory 召回候选池查询扩展。

use axon_domain::{AxonError, AxonResult, MemorySourceOrigin, MemoryState};
use rusqlite::params;

use crate::store::{
    MemorySqliteRecord, MemorySqliteStore, i64_from_usize, origin_code, state_code,
};

impl MemorySqliteStore {
    /// 按 Personal scope 读取可发布的稳定个人上下文，并按摘要内容确定性排序。
    pub fn personal_context_records_by_scope(
        &self,
        user_ref: &str,
        namespace_ref: &str,
        now_ms: u64,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if user_ref.trim().is_empty() || namespace_ref.trim().is_empty() || limit == 0 {
            return Err(AxonError::memory_persistence(
                "Personal Memory 快照查询参数无效",
            ));
        }
        self.query_limited_records(
            "SELECT * FROM memory_records WHERE user_ref = ?1 AND namespace_ref = ?2 AND source_origin IN (?3, ?4) AND state IN (?5, ?6) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?7) AND memory_summary IS NOT NULL AND TRIM(memory_summary) <> '' ORDER BY TRIM(memory_summary) ASC, memory_id ASC LIMIT ?8",
            params![
                user_ref,
                namespace_ref,
                origin_code(MemorySourceOrigin::AgentUserContext),
                origin_code(MemorySourceOrigin::ExplicitUserPreference),
                state_code(MemoryState::Weak),
                state_code(MemoryState::Strong),
                i64::try_from(now_ms).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?,
                i64_from_usize(limit)?,
            ],
        )
    }

    /// 可召回候选池的超采样倍数。
    ///
    /// SQL 只按活跃度预筛，最终顺序由 `injection_rank` 决定；
    /// 多取一倍，避免预筛把综合时效更高的记录挡在候选池之外。
    const RECALLABLE_SCOPE_OVERSAMPLE: usize = 2;

    /// 按 Personal scope 有界读取可召回记录，避免依赖工作目录或全表扫描。
    ///
    /// `time_range` 存在时下推到 SQL：候选池只装命中区间的记录，
    /// 否则近期记录会占满名额，被后置过滤清空后得到空结果。
    pub fn recallable_records_by_scope(
        &self,
        user_ref: &str,
        namespace_ref: &str,
        now_ms: u64,
        time_range: Option<(i64, i64)>,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if user_ref.trim().is_empty() || namespace_ref.trim().is_empty() || limit == 0 {
            return Err(AxonError::memory_persistence("Memory scope 查询参数无效"));
        }
        let now = i64::try_from(now_ms)
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?;
        let fetch_limit = i64_from_usize(limit.saturating_mul(Self::RECALLABLE_SCOPE_OVERSAMPLE))?;
        let records = match time_range {
            Some((start_ms, end_ms)) => self.query_limited_records(
                "SELECT * FROM memory_records WHERE user_ref = ?1 AND namespace_ref = ?2 AND state IN (?3, ?4) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?5) AND updated_at_ms >= ?6 AND updated_at_ms <= ?7 AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' ORDER BY last_active_at_ms DESC, importance_basis_points DESC, updated_at_ms DESC, memory_id ASC LIMIT ?8",
                params![
                    user_ref,
                    namespace_ref,
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    now,
                    start_ms,
                    end_ms,
                    fetch_limit,
                ],
            )?,
            None => self.query_limited_records(
                "SELECT * FROM memory_records WHERE user_ref = ?1 AND namespace_ref = ?2 AND state IN (?3, ?4) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?5) AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' ORDER BY last_active_at_ms DESC, importance_basis_points DESC, updated_at_ms DESC, memory_id ASC LIMIT ?6",
                params![
                    user_ref,
                    namespace_ref,
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    now,
                    fetch_limit,
                ],
            )?,
        };
        Ok(Self::rank_records_by_aging(records, now_ms, limit))
    }

    /// 按 Workspace namespace 读取可召回记录；Workspace 不绑定 user_ref。
    pub fn recallable_records_by_namespace(
        &self,
        namespace_ref: &str,
        now_ms: u64,
        time_range: Option<(i64, i64)>,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if namespace_ref.trim().is_empty() || limit == 0 {
            return Err(AxonError::memory_persistence(
                "Memory namespace 查询参数无效",
            ));
        }
        let now = i64::try_from(now_ms)
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?;
        let fetch_limit = i64_from_usize(limit.saturating_mul(Self::RECALLABLE_SCOPE_OVERSAMPLE))?;
        let records = match time_range {
            Some((start_ms, end_ms)) => self.query_limited_records(
                "SELECT * FROM memory_records WHERE user_ref IS NULL AND namespace_ref = ?1 AND state IN (?2, ?3) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?4) AND updated_at_ms >= ?5 AND updated_at_ms <= ?6 AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' ORDER BY last_active_at_ms DESC, importance_basis_points DESC, updated_at_ms DESC, memory_id ASC LIMIT ?7",
                params![
                    namespace_ref,
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    now,
                    start_ms,
                    end_ms,
                    fetch_limit,
                ],
            )?,
            None => self.query_limited_records(
                "SELECT * FROM memory_records WHERE user_ref IS NULL AND namespace_ref = ?1 AND state IN (?2, ?3) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?4) AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' ORDER BY last_active_at_ms DESC, importance_basis_points DESC, updated_at_ms DESC, memory_id ASC LIMIT ?5",
                params![
                    namespace_ref,
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    now,
                    fetch_limit,
                ],
            )?,
        };
        Ok(Self::rank_records_by_aging(records, now_ms, limit))
    }

    /// 按综合时效重排候选池，并截断到调用方要求的上限。
    fn rank_records_by_aging(
        mut records: Vec<MemorySqliteRecord>,
        now_ms: u64,
        limit: usize,
    ) -> Vec<MemorySqliteRecord> {
        records.sort_by(|left, right| {
            crate::injection_rank(right, now_ms).cmp(&crate::injection_rank(left, now_ms))
        });
        records.truncate(limit);
        records
    }
}
