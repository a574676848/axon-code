//! Memory Phase1/Phase2 pipeline 状态表。

use std::time::Duration;

use axon_domain::{AxonError, AxonResult};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::MemoryConfig;

/// Phase1 job kind。
pub const MEMORY_PIPELINE_KIND_PHASE1: &str = "phase1";
/// Phase2 job kind。
pub const MEMORY_PIPELINE_KIND_PHASE2: &str = "phase2";
/// 显式写入 job kind。
pub const MEMORY_PIPELINE_KIND_EXPLICIT_WRITE: &str = "explicit_write";
const MEMORY_PIPELINE_BUSY_TIMEOUT_MS: u64 = 5_000;
/// job 租约：`running` 超过这个时间没有更新，就判定执行进程已死。
///
/// 一期/二期 job 是一次大模型整理，正常远短于一小时；取一小时而不是几分钟，
/// 是为了不给「慢但仍在跑」的任务误判死刑。
const MEMORY_PIPELINE_JOB_LEASE_MS: u64 = 60 * 60 * 1_000;
/// 过期租约回收后的可见原因。
const MEMORY_PIPELINE_LEASE_EXPIRED_ERROR: &str =
    "pipeline job 租约过期：执行进程可能在写入中途退出，已标记为需要修复";
const MEMORY_PIPELINE_STATUS_PENDING: &str = "pending";
const MEMORY_PIPELINE_STATUS_RUNNING: &str = "running";
const MEMORY_PIPELINE_STATUS_SUCCEEDED: &str = "succeeded";
const MEMORY_PIPELINE_STATUS_NEEDS_REPAIR: &str = "needs_repair";
const MEMORY_PIPELINE_STATUS_FAILED: &str = "failed";

/// Memory pipeline job kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPipelineJobKind {
    /// Phase1 per-thread extraction。
    Phase1,
    /// Phase2 global consolidation。
    Phase2,
    /// 显式用户写入。
    ExplicitWrite,
}

/// Memory pipeline job 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPipelineJobStatus {
    /// 等待执行。
    Pending,
    /// 运行中。
    Running,
    /// 成功。
    Succeeded,
    /// 权威数据已写入，但派生投影需要异步修复。
    NeedsRepair,
    /// 权威写入失败。
    Failed,
}

impl MemoryPipelineJobStatus {
    /// 返回稳定持久化与对外状态码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => MEMORY_PIPELINE_STATUS_PENDING,
            Self::Running => MEMORY_PIPELINE_STATUS_RUNNING,
            Self::Succeeded => MEMORY_PIPELINE_STATUS_SUCCEEDED,
            Self::NeedsRepair => MEMORY_PIPELINE_STATUS_NEEDS_REPAIR,
            Self::Failed => MEMORY_PIPELINE_STATUS_FAILED,
        }
    }
}

/// Memory pipeline job。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPipelineJob {
    /// job kind。
    pub kind: MemoryPipelineJobKind,
    /// job key。
    pub job_key: String,
    /// 状态。
    pub status: MemoryPipelineJobStatus,
    /// 更新时间。
    pub updated_at_ms: u64,
}

/// Memory pipeline job 统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryPipelineJobStats {
    /// job 总数。
    pub total: usize,
    /// 等待 job 数。
    pub pending: usize,
    /// 运行中 job 数。
    pub running: usize,
    /// 成功 job 数。
    pub succeeded: usize,
    /// 需要修复的 job 数。
    pub needs_repair: usize,
    /// 失败 job 数。
    pub failed: usize,
}

/// Memory pipeline state store。
pub struct MemoryPipelineStateStore {
    connection: Connection,
}

impl MemoryPipelineStateStore {
    /// 打开 pipeline state store。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        let connection = Connection::open(&config.db_path)
            .map_err(|_| AxonError::memory_persistence("无法打开 Memory pipeline 数据库"))?;
        connection
            .busy_timeout(Duration::from_millis(MEMORY_PIPELINE_BUSY_TIMEOUT_MS))
            .map_err(|_| AxonError::memory_persistence("Memory pipeline busy timeout 配置失败"))?;
        Ok(Self { connection })
    }

    /// 初始化 pipeline schema，并回收上一进程遗留的过期租约。
    ///
    /// `MemoryOrchestrator::init` 会走到这里，所以它同时是 pipeline 的启动点：
    /// 崩溃或强杀留下的 `running` 行必须在这里改成 `needs_repair`，否则那条 job 会一直
    /// 假装在跑，修复线与诊断都看不到它。
    pub fn initialize_schema(&self) -> AxonResult<()> {
        self.connection
            .execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS memory_stage1_outputs (
                    thread_id TEXT PRIMARY KEY,
                    source_updated_at_ms INTEGER NOT NULL,
                    raw_memory TEXT NOT NULL,
                    turn_summary TEXT NOT NULL,
                    turn_slug TEXT,
                    generated_at_ms INTEGER NOT NULL,
                    usage_count INTEGER NOT NULL DEFAULT 0,
                    last_usage_ms INTEGER,
                    selected_for_phase2 INTEGER NOT NULL DEFAULT 0,
                    selected_for_phase2_source_updated_at_ms INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_memory_stage1_outputs_source_updated_at
                    ON memory_stage1_outputs(source_updated_at_ms DESC, thread_id DESC);
                CREATE TABLE IF NOT EXISTS memory_pipeline_jobs (
                    kind TEXT NOT NULL,
                    job_key TEXT NOT NULL,
                    status TEXT NOT NULL,
                    worker_id TEXT,
                    ownership_token TEXT,
                    started_at_ms INTEGER,
                    finished_at_ms INTEGER,
                    lease_until_ms INTEGER,
                    last_error TEXT,
                    input_watermark_ms INTEGER,
                    last_success_watermark_ms INTEGER,
                    updated_at_ms INTEGER NOT NULL,
                    PRIMARY KEY(kind, job_key)
                );
                CREATE INDEX IF NOT EXISTS idx_memory_pipeline_jobs_kind_status_lease
                    ON memory_pipeline_jobs(kind, status, lease_until_ms);
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory pipeline schema 初始化失败"))?;
        self.migrate_legacy_stage1_columns()?;
        self.reclaim_stale_running_jobs(unix_now_ms())?;
        Ok(())
    }

    /// 回收过期租约：`running` 且租约已过的 job 改判 `needs_repair`，返回回收条数。
    ///
    /// 只改状态、不删行：`needs_repair` 会进入 `stats_for_kind`、`memory status` 与
    /// HTTP 的 `needsRepair`，于是「这台机器有一条没跑完的整理」成为可见事实，而不是
    /// 一条永远停在 running 的幽灵记录。
    ///
    /// 老版本行没有 `lease_until_ms`，按 `updated_at_ms + 租约` 兜底推算，不能因为缺列
    /// 就把它们永远排除在回收之外。
    pub fn reclaim_stale_running_jobs(&self, now_ms: u64) -> AxonResult<usize> {
        let now = i64_from_u64(now_ms)?;
        let lease = i64_from_u64(MEMORY_PIPELINE_JOB_LEASE_MS)?;
        self.connection
            .execute(
                r"
                UPDATE memory_pipeline_jobs
                   SET status = ?1,
                       last_error = ?2,
                       finished_at_ms = ?3,
                       lease_until_ms = NULL,
                       updated_at_ms = ?3
                 WHERE status = ?4
                   AND COALESCE(lease_until_ms, updated_at_ms + ?5) < ?3
                ",
                params![
                    MEMORY_PIPELINE_STATUS_NEEDS_REPAIR,
                    MEMORY_PIPELINE_LEASE_EXPIRED_ERROR,
                    now,
                    MEMORY_PIPELINE_STATUS_RUNNING,
                    lease,
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory pipeline 过期租约回收失败"))
    }

    /// 老库升级：`memory_stage1_outputs` 的历史列名统一由 `turn_*` 承担。
    ///
    /// 只做一次「新增列 + 回填 + 删除旧列」，不保留双写：两个列名是同一份事实的两种
    /// 叫法，并存会让后续写入方无法判断该写哪一列。新建库在建表时已直接使用
    /// `turn_*`，因此这里只对遗留库生效。
    fn migrate_legacy_stage1_columns(&self) -> AxonResult<()> {
        let mut columns = Vec::new();
        {
            let mut statement = self
                .connection
                .prepare("PRAGMA table_info(memory_stage1_outputs)")
                .map_err(|_| AxonError::memory_persistence("Memory pipeline schema 读取失败"))?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|_| AxonError::memory_persistence("Memory pipeline schema 读取失败"))?;
            for row in rows {
                let column = row.map_err(|_| {
                    AxonError::memory_persistence("Memory pipeline schema 读取失败")
                })?;
                columns.push(column);
            }
        }
        let has = |name: &str| columns.iter().any(|column| column == name);
        if !has("rollout_summary") && !has("rollout_slug") {
            return Ok(());
        }
        let mut batch = String::new();
        if !has("turn_summary") {
            batch.push_str(
                "ALTER TABLE memory_stage1_outputs ADD COLUMN turn_summary TEXT NOT NULL DEFAULT '';\n",
            );
        }
        if !has("turn_slug") {
            batch.push_str("ALTER TABLE memory_stage1_outputs ADD COLUMN turn_slug TEXT;\n");
        }
        if has("rollout_summary") {
            batch.push_str("UPDATE memory_stage1_outputs SET turn_summary = rollout_summary;\n");
        }
        if has("rollout_slug") {
            batch.push_str("UPDATE memory_stage1_outputs SET turn_slug = rollout_slug;\n");
        }
        if has("rollout_summary") {
            batch.push_str("ALTER TABLE memory_stage1_outputs DROP COLUMN rollout_summary;\n");
        }
        if has("rollout_slug") {
            batch.push_str("ALTER TABLE memory_stage1_outputs DROP COLUMN rollout_slug;\n");
        }
        self.connection
            .execute_batch(&batch)
            .map_err(|_| AxonError::memory_persistence("Memory pipeline schema 迁移失败"))
    }

    /// 插入或更新 job。
    pub fn upsert_job(
        &self,
        kind: MemoryPipelineJobKind,
        job_key: &str,
        status: MemoryPipelineJobStatus,
        updated_at_ms: u64,
    ) -> AxonResult<()> {
        self.initialize_schema()?;
        if job_key.trim().is_empty() || updated_at_ms == 0 {
            return Err(AxonError::memory_persistence("Memory pipeline job 无效"));
        }
        self.connection
            .execute(
                r"
                INSERT INTO memory_pipeline_jobs (
                    kind, job_key, status, started_at_ms, finished_at_ms, lease_until_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(kind, job_key) DO UPDATE SET
                    status = excluded.status,
                    finished_at_ms = excluded.finished_at_ms,
                    lease_until_ms = excluded.lease_until_ms,
                    updated_at_ms = excluded.updated_at_ms
                ",
                params![
                    kind_code(kind),
                    job_key.trim(),
                    status_code(status),
                    started_at(status, updated_at_ms)?,
                    finished_at(status, updated_at_ms)?,
                    lease_until(status, updated_at_ms)?,
                    i64_from_u64(updated_at_ms)?,
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory pipeline job 写入失败"))?;
        Ok(())
    }

    /// 读取 job。
    pub fn get_job(
        &self,
        kind: MemoryPipelineJobKind,
        job_key: &str,
    ) -> AxonResult<Option<MemoryPipelineJob>> {
        self.initialize_schema()?;
        self.connection
            .query_row(
                "SELECT kind, job_key, status, updated_at_ms FROM memory_pipeline_jobs WHERE kind = ?1 AND job_key = ?2",
                params![kind_code(kind), job_key],
                |row| {
                    Ok(MemoryPipelineJob {
                        kind: kind_from_code(&row.get::<_, String>(0)?)?,
                        job_key: row.get(1)?,
                        status: status_from_code(&row.get::<_, String>(2)?)?,
                        updated_at_ms: u64_from_i64(row.get(3)?)?,
                    })
                },
            )
            .optional()
            .map_err(|_| AxonError::memory_persistence("Memory pipeline job 读取失败"))
    }

    /// 判断指定 kind 是否已有 job。
    pub fn has_kind(&self, kind: MemoryPipelineJobKind) -> AxonResult<bool> {
        self.initialize_schema()?;
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_pipeline_jobs WHERE kind = ?1)",
                [kind_code(kind)],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| AxonError::memory_persistence("Memory pipeline kind 读取失败"))
    }

    /// 统计指定 kind 的 job 状态。
    pub fn stats_for_kind(
        &self,
        kind: MemoryPipelineJobKind,
    ) -> AxonResult<MemoryPipelineJobStats> {
        self.initialize_schema()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT status, COUNT(*) FROM memory_pipeline_jobs WHERE kind = ?1 GROUP BY status",
            )
            .map_err(|_| AxonError::memory_persistence("Memory pipeline stats 查询失败"))?;
        let rows = statement
            .query_map([kind_code(kind)], |row| {
                Ok((
                    status_from_code(&row.get::<_, String>(0)?)?,
                    row.get::<_, i64>(1)?,
                ))
            })
            .map_err(|_| AxonError::memory_persistence("Memory pipeline stats 读取失败"))?;
        let mut stats = MemoryPipelineJobStats::default();
        for row in rows {
            let (job_status, count) =
                row.map_err(|_| AxonError::memory_persistence("Memory pipeline stats 行无效"))?;
            let count = usize::try_from(count)
                .map_err(|_| AxonError::memory_persistence("Memory pipeline stats 溢出"))?;
            stats.total = stats.total.saturating_add(count);
            match job_status {
                MemoryPipelineJobStatus::Pending => stats.pending = count,
                MemoryPipelineJobStatus::Running => stats.running = count,
                MemoryPipelineJobStatus::Succeeded => stats.succeeded = count,
                MemoryPipelineJobStatus::NeedsRepair => stats.needs_repair = count,
                MemoryPipelineJobStatus::Failed => stats.failed = count,
            }
        }
        Ok(stats)
    }
}

fn started_at(status: MemoryPipelineJobStatus, value: u64) -> AxonResult<Option<i64>> {
    matches!(status, MemoryPipelineJobStatus::Running)
        .then_some(value)
        .map(i64_from_u64)
        .transpose()
}

fn finished_at(status: MemoryPipelineJobStatus, value: u64) -> AxonResult<Option<i64>> {
    matches!(
        status,
        MemoryPipelineJobStatus::Succeeded
            | MemoryPipelineJobStatus::NeedsRepair
            | MemoryPipelineJobStatus::Failed
    )
    .then_some(value)
    .map(i64_from_u64)
    .transpose()
}

/// `running` 才持租约；终态一律清空，避免一条已完成的 job 看起来还在跑。
fn lease_until(status: MemoryPipelineJobStatus, value: u64) -> AxonResult<Option<i64>> {
    matches!(status, MemoryPipelineJobStatus::Running)
        .then_some(value.saturating_add(MEMORY_PIPELINE_JOB_LEASE_MS))
        .map(i64_from_u64)
        .transpose()
}

/// 本机 UTC 毫秒；本 crate 不依赖 axon-time，只用 std 取同一口径的时钟。
fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

const fn kind_code(kind: MemoryPipelineJobKind) -> &'static str {
    match kind {
        MemoryPipelineJobKind::Phase1 => MEMORY_PIPELINE_KIND_PHASE1,
        MemoryPipelineJobKind::Phase2 => MEMORY_PIPELINE_KIND_PHASE2,
        MemoryPipelineJobKind::ExplicitWrite => MEMORY_PIPELINE_KIND_EXPLICIT_WRITE,
    }
}

const fn status_code(status: MemoryPipelineJobStatus) -> &'static str {
    status.as_str()
}

fn kind_from_code(value: &str) -> rusqlite::Result<MemoryPipelineJobKind> {
    match value {
        MEMORY_PIPELINE_KIND_PHASE1 => Ok(MemoryPipelineJobKind::Phase1),
        MEMORY_PIPELINE_KIND_PHASE2 => Ok(MemoryPipelineJobKind::Phase2),
        MEMORY_PIPELINE_KIND_EXPLICIT_WRITE => Ok(MemoryPipelineJobKind::ExplicitWrite),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn status_from_code(value: &str) -> rusqlite::Result<MemoryPipelineJobStatus> {
    match value {
        MEMORY_PIPELINE_STATUS_PENDING => Ok(MemoryPipelineJobStatus::Pending),
        MEMORY_PIPELINE_STATUS_RUNNING => Ok(MemoryPipelineJobStatus::Running),
        MEMORY_PIPELINE_STATUS_SUCCEEDED => Ok(MemoryPipelineJobStatus::Succeeded),
        MEMORY_PIPELINE_STATUS_NEEDS_REPAIR => Ok(MemoryPipelineJobStatus::NeedsRepair),
        MEMORY_PIPELINE_STATUS_FAILED => Ok(MemoryPipelineJobStatus::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn i64_from_u64(value: u64) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory pipeline 数值溢出"))
}

fn u64_from_i64(value: i64) -> rusqlite::Result<u64> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按遗留布局建表：新列尚不存在，历史列名仍在且已有一行数据。
    fn legacy_table(connection: &Connection) {
        connection
            .execute_batch(
                "CREATE TABLE memory_stage1_outputs (
                     thread_id TEXT PRIMARY KEY,
                     source_updated_at_ms INTEGER NOT NULL,
                     raw_memory TEXT NOT NULL,
                     rollout_summary TEXT NOT NULL,
                     rollout_slug TEXT,
                     generated_at_ms INTEGER NOT NULL,
                     usage_count INTEGER NOT NULL DEFAULT 0,
                     last_usage_ms INTEGER,
                     selected_for_phase2 INTEGER NOT NULL DEFAULT 0,
                     selected_for_phase2_source_updated_at_ms INTEGER
                 );
                 INSERT INTO memory_stage1_outputs
                     (thread_id, source_updated_at_ms, raw_memory, rollout_summary, rollout_slug, generated_at_ms)
                 VALUES ('t-1', 10, 'raw', '旧轮次摘要', 'slug-a', 20);",
            )
            .expect("遗留库建表");
    }

    fn stage1_columns(store: &MemoryPipelineStateStore) -> Vec<String> {
        let mut statement = store
            .connection
            .prepare("PRAGMA table_info(memory_stage1_outputs)")
            .expect("读取列信息");
        let rows = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("查询列名");
        rows.map(|row| row.expect("读取列名")).collect()
    }

    #[test]
    fn legacy_schema_is_upgraded_to_turn_columns_with_backfill() {
        let connection = Connection::open_in_memory().expect("内存库");
        legacy_table(&connection);
        let store = MemoryPipelineStateStore { connection };

        store.initialize_schema().expect("遗留库升级");

        let columns = stage1_columns(&store);
        assert!(columns.iter().any(|column| column == "turn_summary"));
        assert!(columns.iter().any(|column| column == "turn_slug"));
        assert!(
            !columns.iter().any(|column| column.starts_with("rollout_")),
            "迁移后不得保留历史列：{columns:?}"
        );
        let (summary, slug) = store
            .connection
            .query_row(
                "SELECT turn_summary, turn_slug FROM memory_stage1_outputs WHERE thread_id = 't-1'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .expect("回填后原有行必须仍在");
        assert_eq!(summary, "旧轮次摘要");
        assert_eq!(slug.as_deref(), Some("slug-a"));
    }

    #[test]
    fn fresh_schema_never_creates_legacy_columns() {
        let connection = Connection::open_in_memory().expect("内存库");
        let store = MemoryPipelineStateStore { connection };

        store.initialize_schema().expect("新建库建表");

        let columns = stage1_columns(&store);
        assert!(columns.iter().any(|column| column == "turn_summary"));
        assert!(columns.iter().any(|column| column == "turn_slug"));
        assert!(
            !columns.iter().any(|column| column.starts_with("rollout_")),
            "新建库不应出现历史列：{columns:?}"
        );
    }

    fn lease_of(store: &MemoryPipelineStateStore, job_key: &str) -> Option<i64> {
        store
            .connection
            .query_row(
                "SELECT lease_until_ms FROM memory_pipeline_jobs WHERE job_key = ?1",
                [job_key],
                |row| row.get(0),
            )
            .expect("job 行必须存在")
    }

    fn status_of(store: &MemoryPipelineStateStore, job_key: &str) -> String {
        store
            .connection
            .query_row(
                "SELECT status FROM memory_pipeline_jobs WHERE job_key = ?1",
                [job_key],
                |row| row.get(0),
            )
            .expect("job 行必须存在")
    }

    /// `running` 必须持租约：没有租约的 running 行等价于「永远在跑」，谁也回收不掉。
    #[test]
    fn running_job_carries_a_lease_and_terminal_states_clear_it() {
        let store = MemoryPipelineStateStore {
            connection: Connection::open_in_memory().expect("内存库"),
        };

        store
            .upsert_job(
                MemoryPipelineJobKind::ExplicitWrite,
                "memory:1",
                MemoryPipelineJobStatus::Running,
                1_000,
            )
            .expect("写入 running");
        assert_eq!(
            lease_of(&store, "memory:1"),
            Some((1_000 + MEMORY_PIPELINE_JOB_LEASE_MS) as i64)
        );

        store
            .upsert_job(
                MemoryPipelineJobKind::ExplicitWrite,
                "memory:1",
                MemoryPipelineJobStatus::Succeeded,
                2_000,
            )
            .expect("写入 succeeded");
        assert_eq!(lease_of(&store, "memory:1"), None, "终态不得继续持租约");
    }

    /// 崩溃遗留的 running 行必须在启动时回收，并进入可见的 needs_repair 统计。
    #[test]
    fn expired_running_job_is_reclaimed_and_counted_by_startup() {
        let store = MemoryPipelineStateStore {
            connection: Connection::open_in_memory().expect("内存库"),
        };
        store.initialize_schema().expect("建表");
        let now = unix_now_ms();
        // 直接落行而不是走 upsert：upsert 自己会先跑一次启动回收，测不到判据本身。
        for (job_key, lease_until_ms, updated_at_ms) in [
            (
                "thread:crashed",
                i64::try_from(now.saturating_sub(1)).unwrap_or(i64::MAX),
                i64::try_from(now.saturating_sub(MEMORY_PIPELINE_JOB_LEASE_MS)).unwrap_or(i64::MAX),
            ),
            (
                "thread:alive",
                i64::try_from(now.saturating_add(MEMORY_PIPELINE_JOB_LEASE_MS)).unwrap_or(i64::MAX),
                i64::try_from(now).unwrap_or(i64::MAX),
            ),
        ] {
            store
                .connection
                .execute(
                    "INSERT INTO memory_pipeline_jobs (
                        kind, job_key, status, started_at_ms, lease_until_ms, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        MEMORY_PIPELINE_KIND_PHASE1,
                        job_key,
                        MEMORY_PIPELINE_STATUS_RUNNING,
                        updated_at_ms,
                        lease_until_ms,
                        updated_at_ms,
                    ],
                )
                .expect("写入 running 行");
        }

        // 诊断入口自身会跑一次启动回收，因此这里同时验证「回收」与「可见」。
        let stats = store
            .stats_for_kind(MemoryPipelineJobKind::Phase1)
            .expect("统计");

        assert_eq!(stats.needs_repair, 1, "过期租约必须进入可见诊断");
        assert_eq!(stats.running, 1, "未过期租约不得被误收");
        assert_eq!(status_of(&store, "thread:crashed"), "needs_repair");
        assert_eq!(status_of(&store, "thread:alive"), "running");
        let error: Option<String> = store
            .connection
            .query_row(
                "SELECT last_error FROM memory_pipeline_jobs WHERE job_key = 'thread:crashed'",
                [],
                |row| row.get(0),
            )
            .expect("读取失败原因");
        assert_eq!(error.as_deref(), Some(MEMORY_PIPELINE_LEASE_EXPIRED_ERROR));
    }

    /// 回收判据只看租约：未过期的一条都不能动，过期的一条都不能漏。
    #[test]
    fn reclaim_only_touches_expired_leases() {
        let store = MemoryPipelineStateStore {
            connection: Connection::open_in_memory().expect("内存库"),
        };
        store.initialize_schema().expect("建表");
        for (job_key, lease_until_ms) in [("thread:crashed", 2_000_i64), ("thread:alive", 9_000)] {
            store
                .connection
                .execute(
                    "INSERT INTO memory_pipeline_jobs (
                        kind, job_key, status, started_at_ms, lease_until_ms, updated_at_ms
                     ) VALUES (?1, ?2, ?3, 1_000, ?4, 1_000)",
                    params![
                        MEMORY_PIPELINE_KIND_PHASE1,
                        job_key,
                        MEMORY_PIPELINE_STATUS_RUNNING,
                        lease_until_ms,
                    ],
                )
                .expect("写入 running 行");
        }

        // 时钟落在两条租约之间：crashed 已过期，alive 还没有。
        let reclaimed = store
            .reclaim_stale_running_jobs(3_000)
            .expect("回收过期租约");

        assert_eq!(reclaimed, 1);
        assert_eq!(status_of(&store, "thread:crashed"), "needs_repair");
        assert_eq!(status_of(&store, "thread:alive"), "running");
    }

    /// 老版本行没有 `lease_until_ms`：按 `updated_at_ms + 租约` 兜底推算，不能被永远跳过。
    #[test]
    fn legacy_running_row_without_lease_column_value_is_still_reclaimed() {
        let store = MemoryPipelineStateStore {
            connection: Connection::open_in_memory().expect("内存库"),
        };
        store.initialize_schema().expect("建表");
        store
            .connection
            .execute(
                "INSERT INTO memory_pipeline_jobs (kind, job_key, status, updated_at_ms)
                 VALUES (?1, 'thread:legacy', ?2, 1_000)",
                params![MEMORY_PIPELINE_KIND_PHASE2, MEMORY_PIPELINE_STATUS_RUNNING],
            )
            .expect("写入历史行");

        let reclaimed = store
            .reclaim_stale_running_jobs(1_000 + MEMORY_PIPELINE_JOB_LEASE_MS + 1)
            .expect("回收过期租约");

        assert_eq!(reclaimed, 1);
        assert_eq!(status_of(&store, "thread:legacy"), "needs_repair");
    }
}
