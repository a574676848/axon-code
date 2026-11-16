//! `channel_bindings` 的 SQLite 实现。
//!
//! 表在控制面库：绑定是长期事实，不随 outbox 清理消失。写入用 `INSERT ... ON CONFLICT`
//! 覆盖，读取按主键直取，两者都命中索引。

use super::SqliteRuntimeStore;
use super::helpers::i64_value;
use crate::ports::{ChannelBinding, ChannelBindingStore, ChannelBindingStoreError, RuntimeStore};
use axon_domain::DaemonSessionId;
use rusqlite::{OptionalExtension, params};

impl ChannelBindingStore for SqliteRuntimeStore {
    fn bind_channel(
        &self,
        channel: &str,
        destination: &str,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<(), ChannelBindingStoreError> {
        ensure_field(channel, "channel")?;
        ensure_field(destination, "destination")?;
        self.initialize().map_err(runtime_error)?;
        let now = i64_value(now_ms).map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        connection
            .execute(
                "INSERT INTO channel_bindings(channel, destination, session_id, created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(channel, destination) DO UPDATE SET
                     session_id = excluded.session_id,
                     updated_at_ms = excluded.updated_at_ms",
                params![channel, destination, session_id.as_str(), now],
            )
            .map(|_| ())
            .map_err(storage_error)
    }

    fn resolve_channel_binding(
        &self,
        channel: &str,
        destination: &str,
    ) -> Result<Option<DaemonSessionId>, ChannelBindingStoreError> {
        if channel.trim().is_empty() || destination.trim().is_empty() {
            return Ok(None);
        }
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let resolved: Option<String> = connection
            .query_row(
                "SELECT session_id FROM channel_bindings WHERE channel=?1 AND destination=?2",
                params![channel, destination],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        resolved
            .map(|session_id| {
                DaemonSessionId::new(session_id)
                    .map_err(|_| ChannelBindingStoreError::Storage("绑定的会话 ID 非法".to_owned()))
            })
            .transpose()
    }

    fn unbind_channel(
        &self,
        channel: &str,
        destination: &str,
    ) -> Result<bool, ChannelBindingStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        connection
            .execute(
                "DELETE FROM channel_bindings WHERE channel=?1 AND destination=?2",
                params![channel, destination],
            )
            .map(|deleted| deleted > 0)
            .map_err(storage_error)
    }

    fn list_channel_bindings(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Vec<ChannelBinding>, ChannelBindingStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let mut statement = connection
            .prepare(
                "SELECT channel, destination, session_id, updated_at_ms
                 FROM channel_bindings
                 WHERE session_id=?1
                 ORDER BY updated_at_ms DESC, channel ASC, destination ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        rows.into_iter()
            .map(|(channel, destination, session_id, updated_at_ms)| {
                Ok(ChannelBinding {
                    channel,
                    destination,
                    session_id: DaemonSessionId::new(session_id).map_err(|_| {
                        ChannelBindingStoreError::Storage("绑定的会话 ID 非法".to_owned())
                    })?,
                    updated_at_ms: u64::try_from(updated_at_ms).unwrap_or_default(),
                })
            })
            .collect()
    }
}

fn ensure_field(value: &str, field: &str) -> Result<(), ChannelBindingStoreError> {
    if value.trim().is_empty() {
        return Err(ChannelBindingStoreError::InvalidInput(format!(
            "{field} 不能为空"
        )));
    }
    Ok(())
}

fn storage_error(error: rusqlite::Error) -> ChannelBindingStoreError {
    ChannelBindingStoreError::Storage(error.to_string())
}

fn runtime_error(error: crate::ports::RuntimeStoreError) -> ChannelBindingStoreError {
    ChannelBindingStoreError::Storage(error.to_string())
}
