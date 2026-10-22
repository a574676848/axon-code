//! 远控状态存储的有界批量键操作。

use std::{path::Path, time::Duration};

use axon_domain::{AxonError, AxonResult};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params_from_iter, types::Value};

use super::{RemoteStateStoreRuntime, SQLITE_BUSY_TIMEOUT_MS, open};

const REMOTE_STATE_KEY_BATCH_LIMIT: usize = 256;

impl RemoteStateStoreRuntime {
    /// 删除指定业务主键的当前状态。
    pub fn delete_keys(path: impl AsRef<Path>, keys: &[String]) -> AxonResult<usize> {
        validate_keys(keys, "远控状态删除主键无效")?;
        if keys.is_empty() {
            return Ok(0);
        }
        let mut connection = open(path.as_ref())?;
        let mut removed = 0_usize;
        for batch in keys.chunks(REMOTE_STATE_KEY_BATCH_LIMIT) {
            let placeholders = placeholders(batch.len());
            let index_statement =
                format!("DELETE FROM remote_state_index WHERE record_key IN ({placeholders})");
            let state_statement =
                format!("DELETE FROM remote_state WHERE record_key IN ({placeholders})");
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| AxonError::runtime_failure("远控状态存储事务启动失败"))?;
            transaction
                .execute(&index_statement, params_from_iter(batch.iter()))
                .map_err(|_| AxonError::runtime_failure("远控状态索引删除失败"))?;
            removed = removed.saturating_add(
                transaction
                    .execute(&state_statement, params_from_iter(batch.iter()))
                    .map_err(|_| AxonError::runtime_failure("远控状态删除失败"))?,
            );
            transaction
                .commit()
                .map_err(|_| AxonError::runtime_failure("远控状态存储提交失败"))?;
        }
        Ok(removed)
    }

    /// 只读统计指定业务主键中存在的当前状态，按有界批次查询。
    pub fn count_keys(path: impl AsRef<Path>, keys: &[String]) -> AxonResult<usize> {
        validate_keys(keys, "远控状态统计主键无效")?;
        if keys.is_empty() || !path.as_ref().exists() {
            return Ok(0);
        }
        let connection = open_read_only(path.as_ref())?;
        let mut count = 0_usize;
        for batch in keys.chunks(REMOTE_STATE_KEY_BATCH_LIMIT) {
            let statement = format!(
                "SELECT COUNT(*) FROM remote_state WHERE record_key IN ({})",
                placeholders(batch.len())
            );
            let batch_count: i64 = connection
                .query_row(&statement, params_from_iter(batch.iter()), |row| row.get(0))
                .map_err(|_| AxonError::runtime_failure("远控状态统计失败"))?;
            count = count.saturating_add(usize::try_from(batch_count).unwrap_or(usize::MAX));
        }
        Ok(count)
    }

    /// 只读查询指定命名空间与分区的业务主键，按有界批次命中组合索引。
    pub fn keys_for_indexed_partitions(
        path: impl AsRef<Path>,
        namespace: &str,
        partitions: &[String],
    ) -> AxonResult<Vec<String>> {
        if namespace.trim().is_empty()
            || partitions
                .iter()
                .any(|partition| partition.trim().is_empty())
        {
            return Err(AxonError::runtime_failure("远控状态分区查询参数无效"));
        }
        if partitions.is_empty() || !path.as_ref().exists() {
            return Ok(Vec::new());
        }
        let connection = open_read_only(path.as_ref())?;
        let mut keys = Vec::new();
        for batch in partitions.chunks(REMOTE_STATE_KEY_BATCH_LIMIT) {
            let sql = format!(
                "SELECT record_key FROM remote_state_index
                 WHERE namespace=? AND partition IN ({})",
                placeholders(batch.len())
            );
            let mut values = vec![Value::Text(namespace.to_owned())];
            values.extend(batch.iter().cloned().map(Value::Text));
            let mut statement = connection
                .prepare(&sql)
                .map_err(|_| AxonError::runtime_failure("远控状态分区查询失败"))?;
            let rows = statement
                .query_map(params_from_iter(values), |row| row.get::<_, String>(0))
                .map_err(|_| AxonError::runtime_failure("远控状态分区查询失败"))?;
            keys.extend(
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|_| AxonError::runtime_failure("远控状态分区读取失败"))?,
            );
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }
}

fn validate_keys(keys: &[String], message: &'static str) -> AxonResult<()> {
    if keys.iter().any(|key| key.trim().is_empty()) {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

fn open_read_only(path: &Path) -> AxonResult<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| AxonError::runtime_failure("远控状态存储只读打开失败"))?;
    connection
        .busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|_| AxonError::runtime_failure("远控状态存储 busy timeout 配置失败"))?;
    Ok(connection)
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}
