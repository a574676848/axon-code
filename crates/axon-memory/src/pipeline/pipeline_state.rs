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

    /// 初始化 pipeline schema。
    pub fn initialize_schema(&self) -> AxonResult<()> {
        self.connection
            .execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS memory_stage1_outputs (
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
            .map_err(|_| AxonError::memory_persistence("Memory pipeline schema 初始化失败"))
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
                    kind, job_key, status, started_at_ms, finished_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(kind, job_key) DO UPDATE SET
                    status = excluded.status,
                    finished_at_ms = excluded.finished_at_ms,
                    updated_at_ms = excluded.updated_at_ms
                ",
                params![
                    kind_code(kind),
                    job_key.trim(),
                    status_code(status),
                    started_at(status, updated_at_ms)?,
                    finished_at(status, updated_at_ms)?,
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
