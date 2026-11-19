//! `inbox` 的行映射、状态码与错误适配。
//!
//! 列顺序是本模块与各条 SELECT 的共同契约：新增列必须同时出现在每条 SELECT 的
//! 同一位置并在这里按同一位置读取，否则所有读取路径都会整体列错位。

use super::decode::{decode_error, decode_json};
use super::helpers::u64_value;
use crate::ports::InteractionStoreError;
use axon_domain::{
    DaemonSessionId, InteractionId, InteractionRecord, InteractionSource, InteractionStatus,
};
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

/// 队列口径的正常收口：账本结算把工作项写成 `completed`。
const STATUS_COMPLETED: &str = "completed";
/// 队列口径的被取消：取消路径把工作项写成 `cancelled`。
const STATUS_CANCELLED: &str = "cancelled";
/// 队列口径的失败：执行失败路径把工作项写成 `failed`。
const STATUS_FAILED: &str = "failed";

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

fn parse_status(value: &str) -> rusqlite::Result<InteractionStatus> {
    match value {
        STATUS_RECEIVED => Ok(InteractionStatus::Received),
        STATUS_PROCESSING => Ok(InteractionStatus::Processing),
        // 派生列同时承载两套词汇：结算写入的 Interaction 口径，与历史表由
        // `COALESCE(terminal_kind,status)` 带出来的队列口径。后者漏认一个词，
        // 含该轮的整条车道历史就整体读不出来。
        STATUS_UNDERSTOOD | STATUS_COMPLETED => Ok(InteractionStatus::Understood),
        STATUS_REJECTED => Ok(InteractionStatus::Rejected),
        // 失败与取消在 Interaction 口径下都表示本轮没有正常收口。
        STATUS_INTERRUPTED | STATUS_FAILED | STATUS_CANCELLED => Ok(InteractionStatus::Interrupted),
        _ => Err(decode_error("inbox.status", value)),
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
