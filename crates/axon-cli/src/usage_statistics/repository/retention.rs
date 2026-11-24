//! 数据生命周期与防膨胀清理（SSOT §7.1 / §2.2 - M5）。
//!
//! 1. 五档 TTL 淘汰契约：
//!    - usage_rollup_minute: 48h
//!    - usage_rollup_hour: 90d
//!    - usage_rollup_day: 365d
//!    - usage_facts: 90d
//!    - 孤儿 active 事实: > 7d 停滞标记 abandoned 后回收
//! 2. Dry-Run 100% 镜像覆盖过滤谓词；
//! 3. 严格短事务分批 (≤ 256 条/批)，级联全桶重建；
//! 4. auto_vacuum 一次性平滑迁移（事务外，退避重试，绝不 panic）；
//! 5. freelist_count > 2000 页时触发 incremental_vacuum(500)；
//! 6. 索引规格核验（严禁重复建删）。

use std::collections::BTreeSet;
use std::thread;
use std::time::Duration;

use rusqlite::{Connection, params};

use super::{UsageRepository, db_error, from_i64, rebuild_day_bucket, rebuild_hour_bucket, to_i64};

/// 分批清理单批上限。
pub const USAGE_CLEANUP_BATCH_LIMIT: usize = 256;

/// 五档 TTL 常量定义（毫秒）。
pub const TTL_MINUTE_ROLLUP_MS: u64 = 48 * 60 * 60 * 1_000; // 48 小时
pub const TTL_HOUR_ROLLUP_MS: u64 = 90 * 24 * 60 * 60 * 1_000; // 90 天
pub const TTL_DAY_ROLLUP_MS: u64 = 365 * 24 * 60 * 60 * 1_000; // 365 天
pub const TTL_FACTS_MS: u64 = 90 * 24 * 60 * 60 * 1_000; // 90 天
pub const TTL_ORPHAN_ACTIVE_MS: u64 = 7 * 24 * 60 * 60 * 1_000; // 7 天

/// 空闲页回收阈值与批次。
pub const FREELIST_VACUUM_PAGE_THRESHOLD: i64 = 2_000;
pub const INCREMENTAL_VACUUM_PAGES: i64 = 500;

/// 清理执行统计报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageCleanupReport {
    /// 是否为 dry-run 模拟运行。
    pub dry_run: bool,
    /// 停滞孤儿 active 事实处理条数。
    pub stale_active_marked: u64,
    /// 超过 90 天事实删除条数。
    pub deleted_facts: u64,
    /// 超过 48 小时分钟桶删除条数。
    pub deleted_minute_rollups: u64,
    /// 超过 90 天小时桶删除条数。
    pub deleted_hour_rollups: u64,
    /// 超过 365 天日桶删除条数。
    pub deleted_day_rollups: u64,
    /// 空闲页数。
    pub freelist_count: i64,
    /// 是否执行了 incremental_vacuum。
    pub vacuum_executed: bool,
}

// ============================================================================
// 共享谓词截止时间函数（Dry-Run 与真实删除 100% 镜像）
// ============================================================================

#[must_use]
pub fn orphan_active_cutoff_ms(now_ms: u64) -> u64 {
    now_ms.saturating_sub(TTL_ORPHAN_ACTIVE_MS)
}

#[must_use]
pub fn facts_retention_cutoff_ms(now_ms: u64) -> u64 {
    now_ms.saturating_sub(TTL_FACTS_MS)
}

#[must_use]
pub fn minute_rollup_cutoff_ms(now_ms: u64) -> u64 {
    now_ms.saturating_sub(TTL_MINUTE_ROLLUP_MS)
}

#[must_use]
pub fn hour_rollup_cutoff_ms(now_ms: u64) -> u64 {
    now_ms.saturating_sub(TTL_HOUR_ROLLUP_MS)
}

#[must_use]
pub fn day_rollup_cutoff_ms(now_ms: u64) -> u64 {
    now_ms.saturating_sub(TTL_DAY_ROLLUP_MS)
}

impl UsageRepository {
    pub(crate) fn active_fact_keys(&self, limit: usize) -> Result<Vec<(String, String)>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT session_id, assistant_part_id
                 FROM usage_facts
                 WHERE status = 'active'
                 ORDER BY recorded_at_ms ASC, session_id ASC, assistant_part_id ASC
                 LIMIT ?1",
            )
            .map_err(db_error("准备 usage active 事实复核查询"))?;
        let rows = statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(db_error("查询 usage active 事实复核键"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 usage active 事实复核键"))
    }

    /// 执行五档 TTL 清理（支持 dry-run 镜像预测）。
    pub fn cleanup_five_tier_retention(
        &mut self,
        now_ms: u64,
        dry_run: bool,
    ) -> Result<UsageCleanupReport, String> {
        let orphan_cutoff = to_i64(orphan_active_cutoff_ms(now_ms))?;
        let facts_cutoff = to_i64(facts_retention_cutoff_ms(now_ms))?;
        let minute_cutoff = to_i64(minute_rollup_cutoff_ms(now_ms))?;
        let hour_cutoff = to_i64(hour_rollup_cutoff_ms(now_ms))?;
        let day_cutoff = to_i64(day_rollup_cutoff_ms(now_ms))?;

        if dry_run {
            let stale_active_marked = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_facts WHERE status = 'active' AND recorded_at_ms < ?1",
                    [orphan_cutoff],
                    |row| from_i64(row.get(0)?),
                )
                .map_err(db_error("统计孤儿 active 事实"))?;

            let deleted_facts = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_facts WHERE status <> 'active' AND recorded_at_ms < ?1",
                    [facts_cutoff],
                    |row| from_i64(row.get(0)?),
                )
                .map_err(db_error("统计到期 usage facts"))?;

            let deleted_minute_rollups = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_rollup_minute WHERE bucket_start_ms < ?1",
                    [minute_cutoff],
                    |row| from_i64(row.get(0)?),
                )
                .map_err(db_error("统计到期分钟聚合"))?;

            let deleted_hour_rollups = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_rollup_hour WHERE bucket_start_ms < ?1",
                    [hour_cutoff],
                    |row| from_i64(row.get(0)?),
                )
                .map_err(db_error("统计到期小时聚合"))?;

            let deleted_day_rollups = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_rollup_day WHERE bucket_start_ms < ?1",
                    [day_cutoff],
                    |row| from_i64(row.get(0)?),
                )
                .map_err(db_error("统计到期日聚合"))?;

            let freelist_count: i64 = self
                .connection
                .query_row("PRAGMA freelist_count", [], |row| row.get(0))
                .unwrap_or(0);

            return Ok(UsageCleanupReport {
                dry_run: true,
                stale_active_marked,
                deleted_facts,
                deleted_minute_rollups,
                deleted_hour_rollups,
                deleted_day_rollups,
                freelist_count,
                vacuum_executed: false,
            });
        }

        // 1. 真实处理孤儿 active 事实：标记为 abandoned
        let mut stale_active_marked = 0_u64;
        loop {
            let tx = self
                .connection
                .transaction()
                .map_err(db_error("启动孤儿 active 处理事务"))?;
            let affected = tx
                .execute(
                    "UPDATE usage_facts
                     SET status = 'abandoned'
                     WHERE rowid IN (
                         SELECT rowid FROM usage_facts
                         WHERE status = 'active' AND recorded_at_ms < ?1
                         LIMIT ?2
                     )",
                    params![
                        orphan_cutoff,
                        i64::try_from(USAGE_CLEANUP_BATCH_LIMIT).unwrap_or(i64::MAX)
                    ],
                )
                .map_err(db_error("标记孤儿 active 事实为 abandoned"))?;
            tx.commit().map_err(db_error("提交孤儿 active 处理事务"))?;
            stale_active_marked = stale_active_marked.saturating_add(affected as u64);
            if affected < USAGE_CLEANUP_BATCH_LIMIT {
                break;
            }
        }

        // 记录事实级联删除前到期的小时/日聚合总数，确保与 dry-run 统计口径 100% 镜像一致
        let initial_expired_hour: u64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM usage_rollup_hour WHERE bucket_start_ms < ?1",
                [hour_cutoff],
                |row| from_i64(row.get(0)?),
            )
            .map_err(db_error("统计清理前到期小时聚合"))?;

        let initial_expired_day: u64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM usage_rollup_day WHERE bucket_start_ms < ?1",
                [day_cutoff],
                |row| from_i64(row.get(0)?),
            )
            .map_err(db_error("统计清理前到期日聚合"))?;

        // 2. 真实处理到期 facts：分批删除并级联重建受影响的 hour/day 聚合桶
        let mut deleted_facts = 0_u64;
        loop {
            let tx = self
                .connection
                .transaction()
                .map_err(db_error("启动到期 facts 删除事务"))?;

            let selected: Vec<(i64, u64, u64)> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT rowid, hour_bucket_ms, day_bucket_ms
                         FROM usage_facts
                         WHERE status <> 'active' AND recorded_at_ms < ?1
                         ORDER BY recorded_at_ms ASC, rowid ASC
                         LIMIT ?2",
                    )
                    .map_err(db_error("准备到期 facts 批次查询"))?;
                let rows = stmt
                    .query_map(
                        params![
                            facts_cutoff,
                            i64::try_from(USAGE_CLEANUP_BATCH_LIMIT).unwrap_or(i64::MAX)
                        ],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                from_i64(row.get(1)?)?,
                                from_i64(row.get(2)?)?,
                            ))
                        },
                    )
                    .map_err(db_error("查询到期 facts 批次"))?;
                let mut batch = Vec::new();
                for r in rows {
                    batch.push(r.map_err(db_error("解析到期 facts 批次行"))?);
                }
                batch
            };

            if selected.is_empty() {
                break;
            }

            let batch_count = selected.len();
            let mut touched_hours = BTreeSet::new();
            let mut touched_days = BTreeSet::new();

            for (rowid, hour, day) in selected {
                tx.execute("DELETE FROM usage_facts WHERE rowid = ?1", [rowid])
                    .map_err(db_error("删除到期 usage fact"))?;
                touched_hours.insert(hour);
                touched_days.insert(day);
            }

            for hour in touched_hours {
                rebuild_hour_bucket(&tx, hour)?;
            }
            for day in touched_days {
                rebuild_day_bucket(&tx, day)?;
            }

            tx.commit()
                .map_err(db_error("提交到期 facts 批次删除事务"))?;
            deleted_facts = deleted_facts.saturating_add(batch_count as u64);

            if batch_count < USAGE_CLEANUP_BATCH_LIMIT {
                break;
            }
        }

        // 3. 真实物理删除到期聚合表记录 (minute, hour, day)
        let deleted_minute_rollups = self
            .connection
            .execute(
                "DELETE FROM usage_rollup_minute WHERE bucket_start_ms < ?1",
                [minute_cutoff],
            )
            .map_err(db_error("物理删除到期分钟聚合"))? as u64;

        self.connection
            .execute(
                "DELETE FROM usage_rollup_hour WHERE bucket_start_ms < ?1",
                [hour_cutoff],
            )
            .map_err(db_error("物理删除到期小时聚合"))?;

        self.connection
            .execute(
                "DELETE FROM usage_rollup_day WHERE bucket_start_ms < ?1",
                [day_cutoff],
            )
            .map_err(db_error("物理删除到期日聚合"))?;

        let final_expired_hour: u64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM usage_rollup_hour WHERE bucket_start_ms < ?1",
                [hour_cutoff],
                |row| from_i64(row.get(0)?),
            )
            .map_err(db_error("统计清理后剩余到期小时聚合"))?;

        let final_expired_day: u64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM usage_rollup_day WHERE bucket_start_ms < ?1",
                [day_cutoff],
                |row| from_i64(row.get(0)?),
            )
            .map_err(db_error("统计清理后剩余到期日聚合"))?;

        let deleted_hour_rollups = initial_expired_hour.saturating_sub(final_expired_hour);
        let deleted_day_rollups = initial_expired_day.saturating_sub(final_expired_day);

        // 4. 同步更新 usage_projection_state 中的 projected_fact_count
        let remaining_facts: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| row.get(0))
            .map_err(db_error("统计清理后剩余 facts"))?;

        self.connection
            .execute(
                "UPDATE usage_projection_state
                 SET projected_fact_count = ?1
                 WHERE projector = ?2",
                params![remaining_facts, super::super::USAGE_PROJECTOR_NAME],
            )
            .map_err(db_error("更新清理后 projector 状态"))?;

        // 5. 碎片整理：freelist_count > 2000 页时执行 incremental_vacuum(500)
        let freelist_count: i64 = self
            .connection
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .unwrap_or(0);

        let mut vacuum_executed = false;
        if freelist_count > FREELIST_VACUUM_PAGE_THRESHOLD {
            if let Err(e) = self.connection.execute_batch(&format!(
                "PRAGMA incremental_vacuum({INCREMENTAL_VACUUM_PAGES});"
            )) {
                eprintln!("[usage-cleanup] incremental_vacuum 失败: {e}");
            } else {
                vacuum_executed = true;
            }
        }

        Ok(UsageCleanupReport {
            dry_run: false,
            stale_active_marked,
            deleted_facts,
            deleted_minute_rollups,
            deleted_hour_rollups,
            deleted_day_rollups,
            freelist_count,
            vacuum_executed,
        })
    }

    /// 兼容既有接口的保留清理包装。
    pub fn cleanup_retention(&mut self, _cutoff_ms: u64, dry_run: bool) -> Result<u64, String> {
        let now_ms = axon_time::unix_now_ms();
        let report = self.cleanup_five_tier_retention(now_ms, dry_run)?;
        Ok(report
            .deleted_facts
            .saturating_add(report.stale_active_marked))
    }

    /// 一次性 auto_vacuum INCREMENTAL 迁移（事务外执行，支持 3 次退避重试，绝不 panic）。
    pub fn ensure_auto_vacuum_incremental(connection: &Connection) -> Result<bool, String> {
        let mode: i64 = connection
            .query_row("PRAGMA auto_vacuum;", [], |row| row.get(0))
            .map_err(db_error("查询 PRAGMA auto_vacuum"))?;

        if mode == 2 {
            // 2 = INCREMENTAL
            return Ok(false);
        }

        for attempt in 1..=3 {
            match connection.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;") {
                Ok(()) => return Ok(true),
                Err(err) => {
                    eprintln!("[usage-cleanup] auto_vacuum VACUUM 第 {attempt} 次失败: {err}");
                    if attempt < 3 {
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        }

        eprintln!("[usage-cleanup] auto_vacuum VACUUM 3 次受阻，安全顺延至下次低峰维护窗口");
        Ok(false)
    }

    /// 核验 usage.sqlite3 既有索引规格完整性（SSOT §6.2 / §2.2）。
    #[cfg(test)]
    pub fn verify_indexes(connection: &Connection) -> Result<Vec<String>, String> {
        let mut found_indexes = Vec::new();

        let mut stmt = connection
            .prepare("PRAGMA index_list('usage_facts')")
            .map_err(db_error("读取 usage_facts 索引列表"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(db_error("查询 usage_facts 索引列表"))?;
        for r in rows {
            found_indexes.push(r.map_err(db_error("解析 usage_facts 索引名"))?);
        }

        let mut stmt_min = connection
            .prepare("PRAGMA index_list('usage_rollup_minute')")
            .map_err(db_error("读取 usage_rollup_minute 索引列表"))?;
        let rows_min = stmt_min
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(db_error("查询 usage_rollup_minute 索引列表"))?;
        for r in rows_min {
            found_indexes.push(r.map_err(db_error("解析 usage_rollup_minute 索引名"))?);
        }

        let mut stmt_pr = connection
            .prepare("PRAGMA index_list('pricing_rules')")
            .map_err(db_error("读取 pricing_rules 索引列表"))?;
        let rows_pr = stmt_pr
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(db_error("查询 pricing_rules 索引列表"))?;
        for r in rows_pr {
            found_indexes.push(r.map_err(db_error("解析 pricing_rules 索引名"))?);
        }

        Ok(found_indexes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cutoff_constants_monotonic() {
        let now_ms = 1_000_000_000_000_u64;
        assert!(orphan_active_cutoff_ms(now_ms) > facts_retention_cutoff_ms(now_ms));
        assert!(minute_rollup_cutoff_ms(now_ms) > hour_rollup_cutoff_ms(now_ms));
        assert!(hour_rollup_cutoff_ms(now_ms) > day_rollup_cutoff_ms(now_ms));
    }
}
