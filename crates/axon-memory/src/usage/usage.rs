//! Memory read usage telemetry。

use std::time::Duration;

use axon_domain::{AxonError, AxonResult, MemoryId};
use rusqlite::{Connection, params};

use crate::MemoryConfig;

/// recall 使用信号。
pub const MEMORY_USAGE_KIND_RECALLED: &str = "recalled";
/// 注入模型上下文使用信号。
pub const MEMORY_USAGE_KIND_INJECTED: &str = "injected";
/// 模型 citation 使用信号。
pub const MEMORY_USAGE_KIND_CITED: &str = "cited";
/// stale 命中信号。
pub const MEMORY_USAGE_KIND_STALE: &str = "stale";
const MEMORY_USAGE_BUSY_TIMEOUT_MS: u64 = 5_000;

/// Memory usage 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryUsageKind {
    /// 被召回。
    Recalled,
    /// 被注入。
    Injected,
    /// 被 citation 使用。
    Cited,
    /// 命中过期或低质量候选。
    Stale,
}

/// Memory usage 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryUsageEvent {
    /// Memory ID。
    pub memory_id: MemoryId,
    /// 使用类型。
    pub kind: MemoryUsageKind,
    /// 发生时间。
    pub occurred_at_ms: u64,
}

/// Memory usage store。
pub struct MemoryUsageStore {
    connection: Connection,
}

impl MemoryUsageStore {
    /// 打开 usage store。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        let connection = Connection::open(&config.db_path)
            .map_err(|_| AxonError::memory_persistence("无法打开 Memory usage 数据库"))?;
        connection
            .busy_timeout(Duration::from_millis(MEMORY_USAGE_BUSY_TIMEOUT_MS))
            .map_err(|_| AxonError::memory_persistence("Memory usage busy timeout 配置失败"))?;
        Ok(Self { connection })
    }

    /// 初始化 usage schema。
    pub fn initialize_schema(&self) -> AxonResult<()> {
        self.connection
            .execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS memory_usage (
                    memory_id INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    occurred_at_ms INTEGER NOT NULL,
                    PRIMARY KEY(memory_id, kind, occurred_at_ms)
                );
                CREATE INDEX IF NOT EXISTS idx_memory_usage_latest
                    ON memory_usage(memory_id, occurred_at_ms DESC);
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory usage schema 初始化失败"))
    }

    /// 记录 usage 事件。
    pub fn record_events(&self, events: &[MemoryUsageEvent]) -> AxonResult<()> {
        self.initialize_schema()?;
        for event in events {
            validate_event(event)?;
            self.connection
                .execute(
                    r"
                    INSERT OR IGNORE INTO memory_usage (memory_id, kind, occurred_at_ms)
                    VALUES (?1, ?2, ?3)
                    ",
                    params![
                        i64_from_u64(event.memory_id.0)?,
                        kind_code(event.kind),
                        i64_from_u64(event.occurred_at_ms)?,
                    ],
                )
                .map_err(|_| AxonError::memory_persistence("Memory usage 记录写入失败"))?;
        }
        Ok(())
    }

    /// 查询 Memory usage 次数。
    pub fn usage_count(&self, memory_id: MemoryId) -> AxonResult<usize> {
        let count = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM memory_usage WHERE memory_id = ?1",
                [i64_from_u64(memory_id.0)?],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| AxonError::memory_persistence("Memory usage 统计失败"))?;
        usize::try_from(count).map_err(|_| AxonError::memory_persistence("Memory usage 统计溢出"))
    }
}

fn validate_event(event: &MemoryUsageEvent) -> AxonResult<()> {
    if event.memory_id.0 == 0 || event.occurred_at_ms == 0 {
        return Err(AxonError::memory_persistence("Memory usage 事件无效"));
    }
    Ok(())
}

const fn kind_code(kind: MemoryUsageKind) -> &'static str {
    match kind {
        MemoryUsageKind::Recalled => MEMORY_USAGE_KIND_RECALLED,
        MemoryUsageKind::Injected => MEMORY_USAGE_KIND_INJECTED,
        MemoryUsageKind::Cited => MEMORY_USAGE_KIND_CITED,
        MemoryUsageKind::Stale => MEMORY_USAGE_KIND_STALE,
    }
}

fn i64_from_u64(value: u64) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory usage 数值溢出"))
}
