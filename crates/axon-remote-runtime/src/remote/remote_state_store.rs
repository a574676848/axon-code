//! 远控可变状态的 SQLite 存储。

use axon_domain::{AxonError, AxonResult};
use rusqlite::{
    Connection, OptionalExtension as _, TransactionBehavior, params, params_from_iter, types::Value,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
    time::Duration,
};

use crate::remote_state_connection::{self, PooledConnection};
const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
const SQLITE_INITIALIZATION_RETRY_INTERVAL_MS: u64 = 25;
const SQLITE_SCHEMA_VERSION_QUERY: &str = "PRAGMA schema_version";
static INITIALIZED_DATABASES: OnceLock<Mutex<HashMap<PathBuf, i64>>> = OnceLock::new();
/// 按状态聚合的当前记录统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStateStatusSummary {
    /// 状态名。
    pub status: String,
    /// 该状态下的记录数量。
    pub count: u64,
    /// 该状态下最老一条记录的更新时间。
    pub oldest_updated_at_ms: u64,
}
/// 当前状态记录的可查询元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStateStoreIndex {
    namespace: &'static str,
    partition: String,
    status: &'static str,
    updated_at_ms: u64,
}
impl RemoteStateStoreIndex {
    /// 创建索引元数据。
    #[must_use]
    pub fn new(
        namespace: &'static str,
        partition: impl Into<String>,
        status: &'static str,
        updated_at_ms: u64,
    ) -> Self {
        Self {
            namespace,
            partition: partition.into(),
            status,
            updated_at_ms,
        }
    }
}
/// 远控 SQLite 状态存储运行时。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteStateStoreRuntime;

impl RemoteStateStoreRuntime {
    /// 关闭指定库路径的池化连接。
    ///
    /// 连接池按路径复用连接，池中连接会持续持有库文件句柄。删除、替换或重建库文件之前必须
    /// 先调用本函数，否则文件在 Windows 上无法删除。
    pub fn close_path(path: impl AsRef<Path>) {
        remote_state_connection::close_path(path.as_ref());
    }

    /// 在 IMMEDIATE 事务内执行多个相关状态读写。
    pub fn with_immediate<T>(
        path: impl AsRef<Path>,
        operation: impl FnOnce(&rusqlite::Transaction<'_>) -> AxonResult<T>,
    ) -> AxonResult<T> {
        let mut connection = open(path.as_ref())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| AxonError::runtime_failure("远控状态存储事务启动失败"))?;
        let result = operation(&transaction)?;
        transaction
            .commit()
            .map_err(|_| AxonError::runtime_failure("远控状态存储提交失败"))?;
        Ok(result)
    }
    /// 按业务主键写入当前状态，revision 较旧的写入不会覆盖当前值。
    pub fn upsert<T: Serialize>(
        path: impl AsRef<Path>,
        key: &str,
        index: RemoteStateStoreIndex,
        value: &T,
    ) -> AxonResult<()> {
        validate_state_write(key, &index)?;
        let payload = serde_json::to_string(value)
            .map_err(|_| AxonError::runtime_failure("远控状态存储序列化失败"))?;
        let mut connection = open(path.as_ref())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| AxonError::runtime_failure("远控状态存储事务启动失败"))?;
        write_state(&transaction, key, index.updated_at_ms, &index, &payload)?;
        transaction
            .commit()
            .map_err(|_| AxonError::runtime_failure("远控状态存储提交失败"))
    }
    /// 在容量满时按 LRU 回收记录，并写入新的当前状态。
    pub fn upsert_with_lru_eviction<T: Serialize>(
        path: impl AsRef<Path>,
        key: &str,
        index: RemoteStateStoreIndex,
        value: &T,
        max_records: usize,
        terminal_statuses: &[&str],
    ) -> AxonResult<Vec<String>> {
        validate_state_write(key, &index)?;
        if max_records == 0
            || terminal_statuses.is_empty()
            || terminal_statuses
                .iter()
                .any(|status| status.trim().is_empty())
        {
            return Err(AxonError::runtime_failure("远控状态存储容量无效"));
        }
        let payload = serde_json::to_string(value)
            .map_err(|_| AxonError::runtime_failure("远控状态存储序列化失败"))?;
        let mut connection = open(path.as_ref())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| AxonError::runtime_failure("远控状态存储事务启动失败"))?;
        let exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM remote_state WHERE record_key=?1)",
                [key],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| AxonError::runtime_failure("远控状态存储读取失败"))?
            != 0;
        let evicted_keys = if exists {
            Vec::new()
        } else {
            evict_lru_records(&transaction, &index, max_records, terminal_statuses)?
        };
        write_state(&transaction, key, index.updated_at_ms, &index, &payload)?;
        transaction
            .commit()
            .map_err(|_| AxonError::runtime_failure("远控状态存储提交失败"))?;
        Ok(evicted_keys)
    }
    /// 读取全部当前状态，不扫描历史版本。
    pub fn read_all<T: DeserializeOwned>(path: impl AsRef<Path>) -> AxonResult<Vec<T>> {
        let connection = open(path.as_ref())?;
        let mut statement = connection
            .prepare("SELECT payload FROM remote_state ORDER BY revision, record_key")
            .map_err(|_| AxonError::runtime_failure("远控状态存储查询失败"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| AxonError::runtime_failure("远控状态存储查询失败"))?;
        rows.map(|row| {
            let payload = row.map_err(|_| AxonError::runtime_failure("远控状态存储读取失败"))?;
            serde_json::from_str(&payload)
                .map_err(|_| AxonError::runtime_failure("远控状态存储记录无效"))
        })
        .collect()
    }
    /// 读取指定业务主键的当前状态。
    pub fn read<T: DeserializeOwned>(path: impl AsRef<Path>, key: &str) -> AxonResult<Option<T>> {
        if key.trim().is_empty() {
            return Err(AxonError::runtime_failure("远控状态存储主键无效"));
        }
        let connection = open(path.as_ref())?;
        connection
            .query_row(
                "SELECT payload FROM remote_state WHERE record_key=?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| AxonError::runtime_failure("远控状态存储读取失败"))?
            .map(|payload| {
                serde_json::from_str(&payload)
                    .map_err(|_| AxonError::runtime_failure("远控状态存储记录无效"))
            })
            .transpose()
    }
    /// 按状态聚合指定 namespace 的当前记录，供健康与容量观测使用。
    ///
    /// 聚合只走 (namespace, status) 索引，不读取 payload 明细。
    pub fn status_summary(
        path: impl AsRef<Path>,
        namespace: &str,
    ) -> AxonResult<Vec<RemoteStateStatusSummary>> {
        if namespace.trim().is_empty() {
            return Err(AxonError::runtime_failure("远控状态索引查询参数无效"));
        }
        let connection = open(path.as_ref())?;
        let mut statement = connection
            .prepare(
                "SELECT status,COUNT(*),MIN(updated_at_ms) FROM remote_state_index \
                 WHERE namespace=?1 GROUP BY status ORDER BY status",
            )
            .map_err(|_| AxonError::runtime_failure("远控状态统计查询失败"))?;
        let rows = statement
            .query_map([namespace], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|_| AxonError::runtime_failure("远控状态统计查询失败"))?;
        let mut summaries = Vec::new();
        for row in rows {
            let (status, count, oldest_updated_at_ms) =
                row.map_err(|_| AxonError::runtime_failure("远控状态统计读取失败"))?;
            summaries.push(RemoteStateStatusSummary {
                status,
                count: u64::try_from(count)
                    .map_err(|_| AxonError::runtime_failure("远控状态统计数值无效"))?,
                oldest_updated_at_ms: u64::try_from(oldest_updated_at_ms)
                    .map_err(|_| AxonError::runtime_failure("远控状态统计数值无效"))?,
            });
        }
        Ok(summaries)
    }
    /// 按状态索引读取当前记录，避免扫描无关业务状态。
    pub fn read_indexed<T: DeserializeOwned>(
        path: impl AsRef<Path>,
        namespace: &str,
        partition: Option<&str>,
        statuses: &[&str],
    ) -> AxonResult<Vec<T>> {
        if namespace.trim().is_empty() || statuses.iter().any(|status| status.trim().is_empty()) {
            return Err(AxonError::runtime_failure("远控状态索引查询参数无效"));
        }
        let mut statement = String::from(
            "SELECT state.payload FROM remote_state_index AS state_index \
             INNER JOIN remote_state AS state ON state.record_key=state_index.record_key \
             WHERE state_index.namespace=?",
        );
        let mut values = vec![Value::Text(namespace.to_owned())];
        if let Some(partition) = partition {
            statement.push_str(" AND state_index.partition=?");
            values.push(Value::Text(partition.to_owned()));
        }
        if !statuses.is_empty() {
            statement.push_str(" AND state_index.status IN (");
            statement.push_str(&vec!["?"; statuses.len()].join(","));
            statement.push(')');
            values.extend(
                statuses
                    .iter()
                    .map(|status| Value::Text((*status).to_owned())),
            );
        }
        statement.push_str(" ORDER BY state_index.updated_at_ms, state_index.record_key");
        let connection = open(path.as_ref())?;
        let mut statement = connection
            .prepare(&statement)
            .map_err(|_| AxonError::runtime_failure("远控状态索引查询失败"))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| row.get::<_, String>(0))
            .map_err(|_| AxonError::runtime_failure("远控状态索引查询失败"))?;
        rows.map(|row| {
            let payload = row.map_err(|_| AxonError::runtime_failure("远控状态存储读取失败"))?;
            serde_json::from_str(&payload)
                .map_err(|_| AxonError::runtime_failure("远控状态存储记录无效"))
        })
        .collect()
    }
    /// 按状态索引有界读取当前记录，避免后台 worker 读取完整状态分区。
    pub fn read_indexed_limited<T: DeserializeOwned>(
        path: impl AsRef<Path>,
        namespace: &str,
        partition: Option<&str>,
        statuses: &[&str],
        limit: usize,
    ) -> AxonResult<Vec<T>> {
        Self::read_indexed_paged(path, namespace, partition, statuses, 0, limit)
    }
    /// 按状态索引支持 offset 分页读取当前记录，防止内存过滤时发生饥饿。
    pub fn read_indexed_paged<T: DeserializeOwned>(
        path: impl AsRef<Path>,
        namespace: &str,
        partition: Option<&str>,
        statuses: &[&str],
        offset: usize,
        limit: usize,
    ) -> AxonResult<Vec<T>> {
        if limit == 0 {
            return Err(AxonError::runtime_failure("远控状态索引查询上限无效"));
        }
        if namespace.trim().is_empty() || statuses.iter().any(|status| status.trim().is_empty()) {
            return Err(AxonError::runtime_failure("远控状态索引查询参数无效"));
        }
        let mut statement = String::from(
            "SELECT state.payload FROM remote_state_index AS state_index \
             INNER JOIN remote_state AS state ON state.record_key=state_index.record_key \
             WHERE state_index.namespace=?",
        );
        let mut values = vec![Value::Text(namespace.to_owned())];
        if let Some(partition) = partition {
            statement.push_str(" AND state_index.partition=?");
            values.push(Value::Text(partition.to_owned()));
        }
        if !statuses.is_empty() {
            statement.push_str(" AND state_index.status IN (");
            statement.push_str(&vec!["?"; statuses.len()].join(","));
            statement.push(')');
            values.extend(
                statuses
                    .iter()
                    .map(|status| Value::Text((*status).to_owned())),
            );
        }
        statement.push_str(
            " ORDER BY state_index.updated_at_ms, state_index.record_key LIMIT ? OFFSET ?",
        );
        values.push(Value::Integer(i64::try_from(limit).map_err(|_| {
            AxonError::runtime_failure("远控状态索引查询上限超限")
        })?));
        values.push(Value::Integer(i64::try_from(offset).map_err(|_| {
            AxonError::runtime_failure("远控状态索引查询 offset 超限")
        })?));
        let connection = open(path.as_ref())?;
        let mut statement = connection
            .prepare(&statement)
            .map_err(|_| AxonError::runtime_failure("远控状态索引查询失败"))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| row.get::<_, String>(0))
            .map_err(|_| AxonError::runtime_failure("远控状态索引查询失败"))?;
        rows.map(|row| {
            let payload = row.map_err(|_| AxonError::runtime_failure("远控状态存储读取失败"))?;
            serde_json::from_str(&payload)
                .map_err(|_| AxonError::runtime_failure("远控状态存储记录无效"))
        })
        .collect()
    }
    /// 删除保留期之前的指定终态当前记录。
    pub fn delete_indexed_before(
        path: impl AsRef<Path>,
        namespace: &str,
        statuses: &[&str],
        cutoff_ms: u64,
    ) -> AxonResult<usize> {
        if namespace.trim().is_empty()
            || statuses.is_empty()
            || statuses.iter().any(|status| status.trim().is_empty())
            || cutoff_ms == 0
        {
            return Err(AxonError::runtime_failure("远控状态清理参数无效"));
        }
        let mut statement = String::from(
            "DELETE FROM remote_state WHERE record_key IN (\
             SELECT record_key FROM remote_state_index WHERE namespace=? \
             AND status IN (",
        );
        statement.push_str(&vec!["?"; statuses.len()].join(","));
        statement.push_str(") AND updated_at_ms<?)");
        let mut values = vec![Value::Text(namespace.to_owned())];
        values.extend(
            statuses
                .iter()
                .map(|status| Value::Text((*status).to_owned())),
        );
        values
            .push(Value::Integer(i64::try_from(cutoff_ms).map_err(|_| {
                AxonError::runtime_failure("远控状态清理时间超限")
            })?));
        let mut connection = open(path.as_ref())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| AxonError::runtime_failure("远控状态存储事务启动失败"))?;
        let removed = transaction
            .execute(&statement, params_from_iter(values))
            .map_err(|_| AxonError::runtime_failure("远控状态清理失败"))?;
        remove_orphaned_indexes(&transaction)?;
        transaction
            .commit()
            .map_err(|_| AxonError::runtime_failure("远控状态存储提交失败"))?;
        Ok(removed)
    }
    /// 在 IMMEDIATE 事务内读取指定状态并写回新版本。
    pub fn update<T>(
        path: impl AsRef<Path>,
        key: &str,
        index: impl FnOnce(&T) -> RemoteStateStoreIndex,
        operation: impl FnOnce(Option<T>) -> AxonResult<T>,
    ) -> AxonResult<T>
    where
        T: Serialize + DeserializeOwned,
    {
        if key.trim().is_empty() {
            return Err(AxonError::runtime_failure("远控状态存储主键无效"));
        }
        let mut connection = open(path.as_ref())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| AxonError::runtime_failure("远控状态存储事务启动失败"))?;
        let current = transaction
            .query_row(
                "SELECT revision, payload FROM remote_state WHERE record_key=?1",
                [key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|_| AxonError::runtime_failure("远控状态存储读取失败"))?
            .map(|(revision, payload)| {
                serde_json::from_str(&payload)
                    .map_err(|_| AxonError::runtime_failure("远控状态存储记录无效"))
                    .map(|value| (revision, value))
            })
            .transpose()?;
        let next_revision = current
            .as_ref()
            .map_or(1, |(revision, _)| revision.saturating_add(1));
        let value = operation(current.map(|(_, value)| value))?;
        let index = index(&value);
        let payload = serde_json::to_string(&value)
            .map_err(|_| AxonError::runtime_failure("远控状态存储序列化失败"))?;
        write_state(
            &transaction,
            key,
            u64::try_from(next_revision).unwrap_or(u64::MAX),
            &index,
            &payload,
        )?;
        transaction
            .commit()
            .map_err(|_| AxonError::runtime_failure("远控状态存储提交失败"))?;
        Ok(value)
    }
    /// 删除指定命名空间和分区的当前状态。
    pub fn delete_indexed_partitions(
        path: impl AsRef<Path>,
        namespace: &str,
        partitions: &[String],
    ) -> AxonResult<usize> {
        if namespace.trim().is_empty()
            || partitions
                .iter()
                .any(|partition| partition.trim().is_empty())
        {
            return Err(AxonError::runtime_failure("远控状态分区清理参数无效"));
        }
        if partitions.is_empty() {
            return Ok(0);
        }
        let placeholders = vec!["?"; partitions.len()].join(",");
        let statement = format!(
            "DELETE FROM remote_state WHERE record_key IN (\
             SELECT record_key FROM remote_state_index WHERE namespace=? \
             AND partition IN ({placeholders}))"
        );
        let mut values = vec![Value::Text(namespace.to_owned())];
        values.extend(partitions.iter().cloned().map(Value::Text));
        let mut connection = open(path.as_ref())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| AxonError::runtime_failure("远控状态存储事务启动失败"))?;
        let removed = transaction
            .execute(&statement, params_from_iter(values))
            .map_err(|_| AxonError::runtime_failure("远控状态清理失败"))?;
        remove_orphaned_indexes(&transaction)?;
        transaction
            .commit()
            .map_err(|_| AxonError::runtime_failure("远控状态存储提交失败"))?;
        Ok(removed)
    }
}

fn open(path: &Path) -> AxonResult<PooledConnection> {
    remote_state_connection::acquire(path, &initialize_database)
}

fn initialize_database(path: &Path, connection: &Connection) -> AxonResult<()> {
    let databases = INITIALIZED_DATABASES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut databases = databases
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let schema_version = read_schema_version(connection)?;
    if databases.get(path) == Some(&schema_version) {
        return Ok(());
    }
    for _ in 0..=SQLITE_BUSY_TIMEOUT_MS / SQLITE_INITIALIZATION_RETRY_INTERVAL_MS {
        if connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS remote_state(record_key TEXT PRIMARY KEY, revision INTEGER NOT NULL, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS remote_state_index(record_key TEXT PRIMARY KEY REFERENCES remote_state(record_key) ON DELETE CASCADE, namespace TEXT NOT NULL, partition TEXT NOT NULL, status TEXT NOT NULL, updated_at_ms INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS remote_state_index_lookup ON remote_state_index(namespace, partition, status, updated_at_ms); CREATE INDEX IF NOT EXISTS remote_state_index_status ON remote_state_index(namespace, status, updated_at_ms);").is_ok() {
            databases.insert(path.to_path_buf(), read_schema_version(connection)?);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(
            SQLITE_INITIALIZATION_RETRY_INTERVAL_MS,
        ));
    }
    Err(AxonError::runtime_persistence("无法初始化远控状态数据库"))
}

fn read_schema_version(connection: &Connection) -> AxonResult<i64> {
    connection
        .query_row(SQLITE_SCHEMA_VERSION_QUERY, [], |row| row.get(0))
        .map_err(|_| AxonError::runtime_persistence("无法读取远控状态数据库版本"))
}

fn write_state(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
    revision: u64,
    index: &RemoteStateStoreIndex,
    payload: &str,
) -> AxonResult<()> {
    let revision =
        i64::try_from(revision).map_err(|_| AxonError::runtime_failure("远控状态版本超限"))?;
    let updated_at_ms = i64::try_from(index.updated_at_ms)
        .map_err(|_| AxonError::runtime_failure("远控状态版本超限"))?;
    let changed = transaction.execute(
        "INSERT INTO remote_state(record_key, revision, payload) VALUES (?1, ?2, ?3) \
         ON CONFLICT(record_key) DO UPDATE SET revision=excluded.revision, payload=excluded.payload \
         WHERE excluded.revision >= remote_state.revision",
        params![key, revision, payload],
    ).map_err(|_| AxonError::runtime_failure("远控状态存储写入失败"))?;
    if changed > 0 {
        transaction.execute(
            "INSERT INTO remote_state_index(record_key, namespace, partition, status, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(record_key) DO UPDATE SET namespace=excluded.namespace, partition=excluded.partition, status=excluded.status, updated_at_ms=excluded.updated_at_ms",
            params![key, index.namespace, index.partition, index.status, updated_at_ms],
        ).map_err(|_| AxonError::runtime_failure("远控状态索引写入失败"))?;
    }
    Ok(())
}

fn evict_lru_records(
    transaction: &rusqlite::Transaction<'_>,
    index: &RemoteStateStoreIndex,
    max_records: usize,
    terminal_statuses: &[&str],
) -> AxonResult<Vec<String>> {
    let current_records = transaction
        .query_row(
            "SELECT COUNT(*) FROM remote_state_index WHERE namespace=?1",
            [index.namespace],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| AxonError::runtime_failure("远控状态存储容量查询失败"))?;
    let current_records = usize::try_from(current_records)
        .map_err(|_| AxonError::runtime_failure("远控状态存储容量无效"))?;
    let eviction_count = current_records
        .saturating_add(1)
        .saturating_sub(max_records);
    if eviction_count == 0 {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; terminal_statuses.len()].join(",");
    let statement = format!(
        "SELECT record_key FROM remote_state_index WHERE namespace=? \
         ORDER BY CASE WHEN status IN ({placeholders}) THEN 0 ELSE 1 END, \
         updated_at_ms, record_key LIMIT ?"
    );
    let mut values = vec![Value::Text(index.namespace.to_owned())];
    values.extend(
        terminal_statuses
            .iter()
            .map(|status| Value::Text((*status).to_owned())),
    );
    values
        .push(Value::Integer(i64::try_from(eviction_count).map_err(
            |_| AxonError::runtime_failure("远控状态存储容量超限"),
        )?));
    let mut statement = transaction
        .prepare(&statement)
        .map_err(|_| AxonError::runtime_failure("远控状态存储回收查询失败"))?;
    let keys = statement
        .query_map(params_from_iter(values), |row| row.get::<_, String>(0))
        .map_err(|_| AxonError::runtime_failure("远控状态存储回收查询失败"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AxonError::runtime_failure("远控状态存储回收读取失败"))?;
    let placeholders = vec!["?"; keys.len()].join(",");
    transaction
        .execute(
            &format!("DELETE FROM remote_state WHERE record_key IN ({placeholders})"),
            params_from_iter(keys.iter()),
        )
        .map_err(|_| AxonError::runtime_failure("远控状态存储回收失败"))?;
    remove_orphaned_indexes(transaction)?;
    Ok(keys)
}

fn validate_state_write(key: &str, index: &RemoteStateStoreIndex) -> AxonResult<()> {
    if key.trim().is_empty()
        || index.namespace.trim().is_empty()
        || index.partition.trim().is_empty()
        || index.status.trim().is_empty()
        || index.updated_at_ms == 0
    {
        return Err(AxonError::runtime_failure("远控状态存储主键或版本无效"));
    }
    Ok(())
}

fn remove_orphaned_indexes(transaction: &rusqlite::Transaction<'_>) -> AxonResult<()> {
    transaction
        .execute(
            "DELETE FROM remote_state_index WHERE record_key NOT IN (SELECT record_key FROM remote_state)",
            [],
        )
        .map_err(|_| AxonError::runtime_failure("远控状态索引清理失败"))?;
    Ok(())
}

#[path = "remote_state_store_keys.rs"]
mod key_operations;
#[cfg(test)]
#[path = "remote_state_store_tests.rs"]
mod tests;
