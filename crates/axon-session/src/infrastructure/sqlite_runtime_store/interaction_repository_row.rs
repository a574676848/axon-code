//! `inbox` 的行映射、状态码与错误适配。
//!
//! 列顺序是本模块与各条 SELECT 的共同契约：新增列必须同时出现在每条 SELECT 的
//! 同一位置并在这里按同一位置读取，否则所有读取路径都会整体列错位。

use super::decode::{decode_error, decode_json};
use super::execution_repository::status_code;
use super::helpers::u64_value;
use crate::domain::interaction_status;
use crate::ports::InteractionStoreError;
use axon_domain::{
    DaemonSessionId, InteractionId, InteractionRecord, InteractionSource, InteractionStatus,
};
use rusqlite::Row;

/// 按 SELECT 的固定列序还原一条 Interaction。
///
/// 每一列都走 `decode_error`：失败时用户看到的是「数据无法识别」，原始字段与取值进日志。
pub(super) fn read_record(row: &Row<'_>) -> rusqlite::Result<InteractionRecord> {
    let raw_interaction_id = row.get::<_, String>(0)?;
    let interaction_id = InteractionId::new(raw_interaction_id.clone())
        .map_err(|_| decode_error("inbox.interaction_id", &raw_interaction_id))?;
    let raw_channel = row.get::<_, String>(1)?;
    let raw_message_id = row.get::<_, String>(2)?;
    let source = InteractionSource::new(raw_channel.clone(), raw_message_id.clone())
        .map_err(|_| decode_error("inbox.source", format!("{raw_channel}/{raw_message_id}")))?;
    // 客户端来源在列尾：没有客户端事实的老行是空串，按「未知」处理。
    let source = match row.get::<_, Option<String>>(12)? {
        Some(client) if !client.trim().is_empty() => source
            .with_client(client.clone())
            .map_err(|_| decode_error("inbox.client", &client))?,
        _ => source,
    };
    let scope = decode_json(row.get::<_, String>(3)?, "inbox.scope_json")?;
    let audience = decode_json(row.get::<_, String>(4)?, "inbox.audience_json")?;
    let reply_route = decode_json(row.get::<_, String>(5)?, "inbox.reply_route_json")?;
    let status = parse_status(&row.get::<_, String>(7)?)?;
    let raw_session_id = row.get::<_, String>(13)?;
    Ok(InteractionRecord {
        interaction_id,
        session_id: DaemonSessionId::new(raw_session_id.clone())
            .map_err(|_| decode_error("inbox.session_id", &raw_session_id))?,
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
        reasoning_effort: row.get::<_, Option<String>>(11)?,
        status,
        received_at_ms: u64_value(row.get(8)?)?,
    })
}

/// 解析派生列的状态取值。
///
/// 派生列同时承载两套词汇：结算写入的 Interaction 口径，与历史表由
/// `COALESCE(terminal_kind,status)` 带出来的队列口径。两套词汇的映射都取自
/// [`interaction_status`]，本函数不写状态字面量；队列口径漏认一个词，含该轮的
/// 整条车道历史就整体读不出来，所以 `uncertain` 这类存量取值必须有归宿。
pub(super) fn parse_status(value: &str) -> rusqlite::Result<InteractionStatus> {
    if let Some(status) = interaction_status::from_token(value) {
        return Ok(status);
    }
    for status in interaction_status::CONCLUSIVE_EXECUTION_STATUSES {
        if status_code(status) == value {
            return interaction_status::derived_from_conclusive(status)
                .ok_or_else(|| decode_error("inbox.status", value));
        }
    }
    Err(decode_error("inbox.status", value))
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
