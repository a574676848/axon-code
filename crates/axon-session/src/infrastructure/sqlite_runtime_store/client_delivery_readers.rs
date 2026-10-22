//! Client delivery 查询结果映射。

use super::super::helpers::u64_value;
use crate::ports::{
    ClientDeliveryStoreError, ClientInboxEntry, ClientPresenceRecord, CompletionDeliveryRecord,
    CompletionNotificationRecord, NotificationDeliveryState,
};
use rusqlite::Row;

pub(super) fn read_inbox_entry(row: &Row<'_>) -> rusqlite::Result<ClientInboxEntry> {
    Ok(ClientInboxEntry {
        notification_id: row.get(0)?,
        goal_id: row.get(1)?,
        execution_id: row.get(2)?,
        channel: row.get(3)?,
        destination: row.get(4)?,
        content: row.get(5)?,
        created_at_ms: u64_value(row.get(6)?)?,
        expires_at_ms: u64_value(row.get(7)?)?,
    })
}

pub(super) fn read_presence(row: &Row<'_>) -> rusqlite::Result<ClientPresenceRecord> {
    Ok(ClientPresenceRecord {
        client_id: row.get(0)?,
        workspace_id: row.get(1)?,
        channel: row.get(2)?,
        destination: row.get(3)?,
        registered_at_ms: u64_value(row.get(4)?)?,
        expires_at_ms: u64_value(row.get(5)?)?,
    })
}

pub(super) fn read_notification(row: &Row<'_>) -> rusqlite::Result<CompletionNotificationRecord> {
    let state: String = row.get(7)?;
    Ok(CompletionNotificationRecord {
        notification_id: row.get(0)?,
        workspace_id: row.get(1)?,
        goal_id: row.get(2)?,
        execution_id: row.get(3)?,
        content: row.get(4)?,
        summary: row.get(5)?,
        result_ids: serde_json::from_str(&row.get::<_, String>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: NotificationDeliveryState::parse(&state).ok_or(rusqlite::Error::InvalidQuery)?,
        created_at_ms: u64_value(row.get(8)?)?,
        updated_at_ms: u64_value(row.get(9)?)?,
        expires_at_ms: u64_value(row.get(10)?)?,
    })
}

pub(super) fn read_delivery(row: &Row<'_>) -> rusqlite::Result<CompletionDeliveryRecord> {
    let state: String = row.get(3)?;
    Ok(CompletionDeliveryRecord {
        notification_id: row.get(0)?,
        channel: row.get(1)?,
        destination: row.get(2)?,
        state: NotificationDeliveryState::parse(&state).ok_or(rusqlite::Error::InvalidQuery)?,
        created_at_ms: u64_value(row.get(4)?)?,
        updated_at_ms: u64_value(row.get(5)?)?,
    })
}

pub(super) fn storage_error(error: impl std::fmt::Display) -> ClientDeliveryStoreError {
    ClientDeliveryStoreError::Storage(error.to_string())
}
