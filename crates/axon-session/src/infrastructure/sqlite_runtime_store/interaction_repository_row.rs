//! `jarvis_interactions` 的行映射、状态码与错误适配。
//!
//! 列顺序是本模块与各条 SELECT 的共同契约：新增列必须同时出现在每条 SELECT 的
//! 同一位置并在这里按同一位置读取，否则所有读取路径都会整体列错位。

use super::helpers::u64_value;
use crate::ports::InteractionStoreError;
use axon_domain::{InteractionId, InteractionRecord, InteractionSource, InteractionStatus};
use rusqlite::Row;

/// 已可靠接收，等待主 turn 领取。
pub(super) const STATUS_RECEIVED: &str = "received";
/// 已被主 turn 领取处理。
pub(super) const STATUS_PROCESSING: &str = "processing";
/// 已完成理解并产生语义结果。
pub(super) const STATUS_UNDERSTOOD: &str = "understood";
/// 因安全或校验原因拒绝。
pub(super) const STATUS_REJECTED: &str = "rejected";
/// 进程中断后无法继续。
pub(super) const STATUS_INTERRUPTED: &str = "interrupted";

/// 按 SELECT 的固定列序还原一条 Interaction。
pub(super) fn read_record(row: &Row<'_>) -> rusqlite::Result<InteractionRecord> {
    let interaction_id =
        InteractionId::new(row.get::<_, String>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let source = InteractionSource::new(row.get::<_, String>(1)?, row.get::<_, String>(2)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let scope = serde_json::from_str(&row.get::<_, String>(3)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let audience = serde_json::from_str(&row.get::<_, String>(4)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let reply_route = serde_json::from_str(&row.get::<_, String>(5)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let status = parse_status(&row.get::<_, String>(7)?)?;
    Ok(InteractionRecord {
        interaction_id,
        source,
        scope,
        audience,
        reply_route,
        content: row.get(6)?,
        attachments: row
            .get::<_, Option<String>>(9)?
            .map_or_else(Vec::new, |json| {
                serde_json::from_str(&json).unwrap_or_default()
            }),
        model: row.get::<_, Option<String>>(10)?,
        status,
        received_at_ms: u64_value(row.get(8)?)?,
    })
}

fn parse_status(value: &str) -> rusqlite::Result<InteractionStatus> {
    match value {
        STATUS_RECEIVED => Ok(InteractionStatus::Received),
        STATUS_PROCESSING => Ok(InteractionStatus::Processing),
        STATUS_UNDERSTOOD => Ok(InteractionStatus::Understood),
        STATUS_REJECTED => Ok(InteractionStatus::Rejected),
        STATUS_INTERRUPTED => Ok(InteractionStatus::Interrupted),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

/// 把领域状态映射为库内状态码。
pub(super) fn status_code(status: InteractionStatus) -> &'static str {
    match status {
        InteractionStatus::Received => STATUS_RECEIVED,
        InteractionStatus::Processing => STATUS_PROCESSING,
        InteractionStatus::Understood => STATUS_UNDERSTOOD,
        InteractionStatus::Rejected => STATUS_REJECTED,
        InteractionStatus::Interrupted => STATUS_INTERRUPTED,
    }
}

pub(super) fn runtime_error(error: impl std::fmt::Display) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

pub(super) fn storage_error(error: rusqlite::Error) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

pub(super) fn serialize_error(error: serde_json::Error) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}
