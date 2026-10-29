//! SQLite Memory 查询扩展。

use axon_domain::{AxonError, AxonResult, MemoryKind, MemorySourceOrigin, MemoryState};
use rusqlite::{OptionalExtension as _, Params, params};

use crate::store::{
    MemoryArchiveStatus, MemoryKindCount, MemoryScopeFacet, MemorySqliteRecord,
    MemorySqliteRecordFilter, MemorySqliteStore, MemorySqliteSummary, MemoryTimelineCursor,
    MemoryVisibility, bool_from_i64, i64_from_u64, i64_from_usize, origin_code, row_to_record,
    state_code, usize_from_i64,
};

impl MemorySqliteStore {
    /// 按类型、scope 与可见范围读取有界时间轴页面（只看未归档记录）。
    ///
    /// 排序固定为 `updated_at_ms DESC, memory_id ASC`，游标在同一毫秒内用 `memory_id`
    /// 继续推进，避免跳页与重复。
    pub fn records_timeline_page(
        &self,
        memory_kind: MemoryKind,
        namespace_ref: Option<&str>,
        visibility: MemoryVisibility<'_>,
        cursor: Option<MemoryTimelineCursor>,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        self.records_timeline_page_with_status(
            memory_kind,
            MemoryArchiveStatus::Active,
            namespace_ref,
            visibility,
            cursor,
            limit,
        )
    }

    /// 按类型、归档状态、scope 与可见范围读取有界时间轴页面。
    ///
    /// 归档状态只接受白名单谓词，因此这里可以安全地拼接进 SQL。
    pub fn records_timeline_page_with_status(
        &self,
        memory_kind: MemoryKind,
        status: MemoryArchiveStatus,
        namespace_ref: Option<&str>,
        visibility: MemoryVisibility<'_>,
        cursor: Option<MemoryTimelineCursor>,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if limit == 0 || namespace_ref.is_some_and(|value| value.trim().is_empty()) {
            return Err(AxonError::memory_persistence("Memory 时间轴查询参数无效"));
        }
        let limit = i64_from_usize(limit)?;
        let kind = memory_kind.as_str();
        let visibility_predicate = visibility.predicate();
        let namespace_predicate = namespace_ref.map_or("1 = 1", |_| "namespace_ref = :namespace");
        let cursor_predicate = cursor.map_or("1 = 1", |_| {
            "(updated_at_ms < :cursor_updated_at_ms OR (updated_at_ms = :cursor_updated_at_ms AND memory_id > :cursor_memory_id))"
        });
        let status_predicate = status.predicate();
        let sql = format!(
            "SELECT * FROM memory_records WHERE memory_kind = :kind AND {status_predicate} AND {visibility_predicate} AND {namespace_predicate} AND {cursor_predicate} ORDER BY updated_at_ms DESC, memory_id ASC LIMIT :limit"
        );
        let cursor_updated_at_ms = i64_from_u64(cursor.map_or(0, |value| value.updated_at_ms))?;
        let cursor_memory_id = i64_from_u64(cursor.map_or(0, |value| value.memory_id))?;
        // 具名参数按需绑定：SQL 里出现的名字必须全部提供，未出现的不得出现。
        let owner_bind = visibility.owner().unwrap_or_default();
        let namespace_bind = namespace_ref.unwrap_or_default();
        let mut binds: Vec<(&str, &dyn rusqlite::ToSql)> =
            vec![(":kind", &kind), (":limit", &limit)];
        if visibility.owner().is_some() {
            binds.push((":owner", &owner_bind));
        }
        if namespace_ref.is_some() {
            binds.push((":namespace", &namespace_bind));
        }
        if cursor.is_some() {
            binds.push((":cursor_updated_at_ms", &cursor_updated_at_ms));
            binds.push((":cursor_memory_id", &cursor_memory_id));
        }
        self.query_limited_records(&sql, &binds[..])
    }

    /// 按类型和治理状态读取有界时间轴列表。
    pub fn records_limited_by_kind_and_state(
        &self,
        memory_kind: MemoryKind,
        filter: Option<MemorySqliteRecordFilter>,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        let limit = i64_from_usize(limit)?;
        if limit == 0 {
            return Err(AxonError::memory_persistence(
                "Memory SQLite 记录列表上限无效",
            ));
        }
        let kind = memory_kind.as_str();
        let query = match filter {
            None => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?2",
                params![kind, limit],
            ),
            Some(MemorySqliteRecordFilter::All) => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?2",
                params![kind, limit],
            ),
            Some(MemorySqliteRecordFilter::Active) => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 AND archived_at_ms IS NULL ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?2",
                params![kind, limit],
            ),
            Some(MemorySqliteRecordFilter::Archived) => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 AND archived_at_ms IS NOT NULL ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?2",
                params![kind, limit],
            ),
            Some(MemorySqliteRecordFilter::Strong) => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 AND state = ?2 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?3",
                params![kind, state_code(MemoryState::Strong), limit],
            ),
            Some(MemorySqliteRecordFilter::Weak) => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 AND state = ?2 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?3",
                params![kind, state_code(MemoryState::Weak), limit],
            ),
            Some(MemorySqliteRecordFilter::Quarantined) => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 AND state = ?2 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?3",
                params![kind, state_code(MemoryState::Quarantined), limit],
            ),
            Some(MemorySqliteRecordFilter::Tombstone) => (
                "SELECT * FROM memory_records WHERE memory_kind = ?1 AND state = ?2 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?3",
                params![kind, state_code(MemoryState::Tombstone), limit],
            ),
        };
        self.query_limited_records(query.0, query.1)
    }

    /// 按可见范围与 scope 聚合统计各类型记录数量（不区分归档状态）。
    pub fn memory_kind_counts(
        &self,
        visibility: MemoryVisibility<'_>,
        namespace_ref: Option<&str>,
    ) -> AxonResult<Vec<MemoryKindCount>> {
        self.memory_kind_counts_with_status(visibility, namespace_ref, MemoryArchiveStatus::All)
    }

    /// 按可见范围、scope 与归档状态聚合统计各类型记录数量。
    ///
    /// 管理页的页签计数必须与同一筛选下的列表同口径，否则会出现「页签写着 5、
    /// 列表只有 4」的读数打架。
    pub fn memory_kind_counts_with_status(
        &self,
        visibility: MemoryVisibility<'_>,
        namespace_ref: Option<&str>,
        status: MemoryArchiveStatus,
    ) -> AxonResult<Vec<MemoryKindCount>> {
        if namespace_ref.is_some_and(|value| value.trim().is_empty()) {
            return Err(AxonError::memory_persistence(
                "Memory 类型统计 scope 参数无效",
            ));
        }
        let namespace_predicate = namespace_ref.map_or("1 = 1", |_| "namespace_ref = :namespace");
        let sql = format!(
            "SELECT memory_kind, COUNT(*) FROM memory_records WHERE {} AND {namespace_predicate} AND {} GROUP BY memory_kind ORDER BY memory_kind ASC",
            visibility.predicate(),
            status.predicate()
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|_| AxonError::memory_persistence("Memory 类型统计查询失败"))?;
        let owner_bind = visibility.owner().unwrap_or_default();
        let namespace_bind = namespace_ref.unwrap_or_default();
        let mut binds: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
        if visibility.owner().is_some() {
            binds.push((":owner", &owner_bind));
        }
        if namespace_ref.is_some() {
            binds.push((":namespace", &namespace_bind));
        }
        let rows = statement
            .query_map(&binds[..], kind_count_row)
            .map_err(|_| AxonError::memory_persistence("Memory 类型统计读取失败"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory 类型统计解析失败"))
    }

    /// 单次聚合查询列出可见范围内实际存在的 scope 及记录数。
    pub fn memory_scope_facets(
        &self,
        visibility: MemoryVisibility<'_>,
    ) -> AxonResult<Vec<MemoryScopeFacet>> {
        let sql = format!(
            "SELECT COALESCE(namespace_ref, '') AS namespace_ref, CASE WHEN user_ref IS NULL THEN 'workspace' ELSE 'personal' END AS scope_kind, COUNT(*) AS record_count FROM memory_records WHERE {} GROUP BY namespace_ref, scope_kind ORDER BY scope_kind ASC, namespace_ref ASC",
            visibility.predicate()
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|_| AxonError::memory_persistence("Memory scope 聚合查询失败"))?;
        let owner_bind = visibility.owner().unwrap_or_default();
        let mut binds: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
        if visibility.owner().is_some() {
            binds.push((":owner", &owner_bind));
        }
        let rows = statement
            .query_map(&binds[..], |row| {
                Ok(MemoryScopeFacet {
                    namespace_ref: row.get(0)?,
                    scope_kind: if row.get::<_, String>(1)? == "personal" {
                        "personal"
                    } else {
                        "workspace"
                    },
                    record_count: usize_from_i64(row.get(2)?)?,
                })
            })
            .map_err(|_| AxonError::memory_persistence("Memory scope 聚合读取失败"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory scope 聚合解析失败"))
    }

    /// 按 content_ref 精确读取活跃记录。
    pub fn find_active_by_content_ref(
        &self,
        content_ref: &str,
    ) -> AxonResult<Option<MemorySqliteRecord>> {
        let content_ref = content_ref.trim();
        if content_ref.is_empty() {
            return Err(AxonError::memory_persistence("Memory content_ref 不能为空"));
        }
        self.connection
            .query_row(
                "SELECT * FROM memory_records WHERE archived_at_ms IS NULL AND TRIM(content_ref) = ?1 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT 1",
                [content_ref],
                row_to_record,
            )
            .optional()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite content_ref 查询失败"))
    }

    /// 按筛选条件和数量上限读取治理记录。
    pub fn records_limited(
        &self,
        filter: MemorySqliteRecordFilter,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if limit == 0 {
            return Err(AxonError::memory_persistence(
                "Memory SQLite 记录列表上限无效",
            ));
        }
        let limit = i64_from_usize(limit)?;
        match filter {
            MemorySqliteRecordFilter::All => self.query_limited_records(
                "SELECT * FROM memory_records ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?1",
                [limit],
            ),
            MemorySqliteRecordFilter::Active => self.query_limited_records(
                "SELECT * FROM memory_records WHERE archived_at_ms IS NULL ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?1",
                [limit],
            ),
            MemorySqliteRecordFilter::Archived => self.query_limited_records(
                "SELECT * FROM memory_records WHERE archived_at_ms IS NOT NULL ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?1",
                [limit],
            ),
            MemorySqliteRecordFilter::Strong => {
                self.query_state_records_limited(MemoryState::Strong, limit)
            }
            MemorySqliteRecordFilter::Weak => {
                self.query_state_records_limited(MemoryState::Weak, limit)
            }
            MemorySqliteRecordFilter::Quarantined => {
                self.query_state_records_limited(MemoryState::Quarantined, limit)
            }
            MemorySqliteRecordFilter::Tombstone => {
                self.query_state_records_limited(MemoryState::Tombstone, limit)
            }
        }
    }

    /// 按待复核条件和数量上限读取治理记录。
    pub fn pending_review_records_limited(
        &self,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        self.query_limited_records(
            "SELECT * FROM memory_records WHERE state IN (?1, ?2) OR source_origin = ?3 OR source_origin IS NULL OR prompt_text_persisted = 1 OR raw_payload_persisted = 1 OR remote_write_allowed = 1 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?4",
            params![
                state_code(MemoryState::Weak),
                state_code(MemoryState::Quarantined),
                origin_code(MemorySourceOrigin::RemoteUpload),
                i64_from_usize(limit)?,
            ],
        )
    }

    /// 按可召回条件和数量上限读取治理记录。
    pub fn recallable_records_limited(
        &self,
        now_ms: u64,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        self.query_limited_records(
            "SELECT * FROM memory_records WHERE state IN (?1, ?2) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?3) AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?4",
            params![
                state_code(MemoryState::Weak),
                state_code(MemoryState::Strong),
                i64::try_from(now_ms).map_err(|_| {
                    AxonError::memory_persistence("Memory SQLite 数值溢出")
                })?,
                i64_from_usize(limit)?,
            ],
        )
    }

    /// 统计指定 scope 内当前可召回治理记录数。
    pub fn recallable_record_count_by_scope(
        &self,
        user_ref: &str,
        namespace_ref: &str,
        now_ms: u64,
    ) -> AxonResult<usize> {
        if user_ref.trim().is_empty() || namespace_ref.trim().is_empty() {
            return Err(AxonError::memory_persistence("Memory scope 查询参数无效"));
        }
        self.connection
            .query_row(
                "SELECT COUNT(*) FROM memory_records WHERE user_ref = ?1 AND namespace_ref = ?2 AND state IN (?3, ?4) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?5) AND content_ref IS NOT NULL AND TRIM(content_ref) <> ''",
                params![
                    user_ref,
                    namespace_ref,
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    i64::try_from(now_ms).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?,
                ],
                |row| usize_from_i64(row.get(0)?),
            )
            .map_err(|_| AxonError::memory_persistence("Memory SQLite scope 记录统计失败"))
    }

    /// 统计 Workspace namespace 内当前可召回治理记录数。
    pub fn recallable_record_count_by_namespace(
        &self,
        namespace_ref: &str,
        now_ms: u64,
    ) -> AxonResult<usize> {
        if namespace_ref.trim().is_empty() {
            return Err(AxonError::memory_persistence(
                "Memory namespace 查询参数无效",
            ));
        }
        self.connection
            .query_row(
                "SELECT COUNT(*) FROM memory_records WHERE user_ref IS NULL AND namespace_ref = ?1 AND state IN (?2, ?3) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?4) AND content_ref IS NOT NULL AND TRIM(content_ref) <> ''",
                params![
                    namespace_ref,
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    i64::try_from(now_ms).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?,
                ],
                |row| usize_from_i64(row.get(0)?),
            )
            .map_err(|_| AxonError::memory_persistence("Memory SQLite namespace 记录统计失败"))
    }

    /// 统计当前可召回治理记录数。
    pub fn recallable_record_count(&self, now_ms: u64) -> AxonResult<usize> {
        self.connection
            .query_row(
                "SELECT COUNT(*) AS recallable_records FROM memory_records WHERE state IN (?1, ?2) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?3) AND content_ref IS NOT NULL AND TRIM(content_ref) <> ''",
                params![
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    i64::try_from(now_ms).map_err(|_| {
                        AxonError::memory_persistence("Memory SQLite 数值溢出")
                    })?,
                ],
                |row| usize_from_i64(row.get("recallable_records")?),
            )
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 可召回汇总查询失败"))
    }

    /// 统计当前可召回且带 vector_id 的治理记录数。
    pub fn recallable_vector_record_count(&self, now_ms: u64) -> AxonResult<usize> {
        self.connection
            .query_row(
                "SELECT COUNT(*) AS recallable_vector_records FROM memory_records WHERE state IN (?1, ?2) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?3) AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' AND vector_id IS NOT NULL",
                params![
                    state_code(MemoryState::Weak),
                    state_code(MemoryState::Strong),
                    i64::try_from(now_ms).map_err(|_| {
                        AxonError::memory_persistence("Memory SQLite 数值溢出")
                    })?,
                ],
                |row| usize_from_i64(row.get("recallable_vector_records")?),
            )
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 可召回向量汇总查询失败"))
    }

    /// 汇总 SQLite store 状态。
    pub fn summary(&self) -> AxonResult<MemorySqliteSummary> {
        self.memory_summary_scoped(MemoryVisibility::All, None)
    }

    /// 汇总可见范围与 scope 内的 SQLite store 状态；单次聚合，不加载全表。
    pub fn memory_summary_scoped(
        &self,
        visibility: MemoryVisibility<'_>,
        namespace_ref: Option<&str>,
    ) -> AxonResult<MemorySqliteSummary> {
        if namespace_ref.is_some_and(|value| value.trim().is_empty()) {
            return Err(AxonError::memory_persistence("Memory 汇总 scope 参数无效"));
        }
        let namespace_predicate = namespace_ref.map_or("1 = 1", |_| "namespace_ref = :namespace");
        let sql = format!(
            "SELECT COUNT(*) AS total_records, COALESCE(SUM(CASE WHEN archived_at_ms IS NULL THEN 1 ELSE 0 END), 0) AS active_records, COALESCE(SUM(CASE WHEN archived_at_ms IS NOT NULL THEN 1 ELSE 0 END), 0) AS archived_records, COALESCE(SUM(CASE WHEN vector_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS vector_records, COALESCE(MAX(prompt_text_persisted), 0) AS prompt_text_persisted, COALESCE(MAX(raw_payload_persisted), 0) AS raw_payload_persisted, COALESCE(MAX(remote_write_allowed), 0) AS remote_write_allowed FROM memory_records WHERE {} AND {namespace_predicate}",
            visibility.predicate()
        );
        let owner_bind = visibility.owner().unwrap_or_default();
        let namespace_bind = namespace_ref.unwrap_or_default();
        let mut binds: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
        if visibility.owner().is_some() {
            binds.push((":owner", &owner_bind));
        }
        if namespace_ref.is_some() {
            binds.push((":namespace", &namespace_bind));
        }
        self.connection
            .query_row(&sql, &binds[..], |row| {
                Ok(MemorySqliteSummary {
                    total_records: usize_from_i64(row.get("total_records")?)?,
                    active_records: usize_from_i64(row.get("active_records")?)?,
                    archived_records: usize_from_i64(row.get("archived_records")?)?,
                    vector_records: usize_from_i64(row.get("vector_records")?)?,
                    prompt_text_persisted: bool_from_i64(row.get("prompt_text_persisted")?)?,
                    raw_payload_persisted: bool_from_i64(row.get("raw_payload_persisted")?)?,
                    remote_write_allowed: bool_from_i64(row.get("remote_write_allowed")?)?,
                })
            })
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 汇总查询失败"))
    }

    /// 汇总单个 namespace 内的 SQLite store 状态。
    pub fn summary_by_namespace(&self, namespace_ref: &str) -> AxonResult<MemorySqliteSummary> {
        if namespace_ref.trim().is_empty() {
            return Err(AxonError::memory_persistence("Memory 汇总 scope 参数无效"));
        }
        self.connection
            .query_row(
                "SELECT COUNT(*) AS total_records, COALESCE(SUM(CASE WHEN archived_at_ms IS NULL THEN 1 ELSE 0 END), 0) AS active_records, COALESCE(SUM(CASE WHEN archived_at_ms IS NOT NULL THEN 1 ELSE 0 END), 0) AS archived_records, COALESCE(SUM(CASE WHEN vector_id IS NOT NULL THEN 1 ELSE 0 END), 0) AS vector_records, COALESCE(MAX(prompt_text_persisted), 0) AS prompt_text_persisted, COALESCE(MAX(raw_payload_persisted), 0) AS raw_payload_persisted, COALESCE(MAX(remote_write_allowed), 0) AS remote_write_allowed FROM memory_records WHERE namespace_ref = ?1",
                [namespace_ref],
                |row| {
                    Ok(MemorySqliteSummary {
                        total_records: usize_from_i64(row.get("total_records")?)?,
                        active_records: usize_from_i64(row.get("active_records")?)?,
                        archived_records: usize_from_i64(row.get("archived_records")?)?,
                        vector_records: usize_from_i64(row.get("vector_records")?)?,
                        prompt_text_persisted: bool_from_i64(row.get("prompt_text_persisted")?)?,
                        raw_payload_persisted: bool_from_i64(row.get("raw_payload_persisted")?)?,
                        remote_write_allowed: bool_from_i64(row.get("remote_write_allowed")?)?,
                    })
                },
            )
            .map_err(|_| AxonError::memory_persistence("Memory SQLite scope 汇总查询失败"))
    }

    fn query_state_records_limited(
        &self,
        state: MemoryState,
        limit: i64,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        self.query_limited_records(
            "SELECT * FROM memory_records WHERE state = ?1 ORDER BY updated_at_ms DESC, memory_id ASC LIMIT ?2",
            params![state_code(state), limit],
        )
    }

    pub(crate) fn query_limited_records<P>(
        &self,
        sql: &str,
        params: P,
    ) -> AxonResult<Vec<MemorySqliteRecord>>
    where
        P: Params,
    {
        let mut stmt = self
            .connection
            .prepare(sql)
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 有界列表查询失败"))?;
        stmt.query_map(params, row_to_record)
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 有界列表读取失败"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 有界列表解析失败"))
    }
}

fn kind_count_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryKindCount> {
    let code = row.get::<_, String>(0)?;
    let memory_kind = MemoryKind::from_code(&code).ok_or(rusqlite::Error::InvalidColumnType(
        0,
        "memory_kind".to_owned(),
        rusqlite::types::Type::Text,
    ))?;
    let count = usize_from_i64(row.get(1)?).map_err(|_| {
        rusqlite::Error::InvalidColumnType(1, "count".to_owned(), rusqlite::types::Type::Integer)
    })?;
    Ok(MemoryKindCount { memory_kind, count })
}
