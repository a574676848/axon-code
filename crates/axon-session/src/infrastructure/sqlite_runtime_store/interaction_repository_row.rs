//! `inbox` 的行映射、状态码与错误适配。
//!
//! 列顺序是本模块与各条 SELECT 的共同契约：新增列必须同时出现在每条 SELECT 的
//! 同一位置并在这里按同一位置读取，否则所有读取路径都会整体列错位。

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
pub(super) fn read_record(row: &Row<'_>) -> rusqlite::Result<InteractionRecord> {
    let interaction_id =
        InteractionId::new(row.get::<_, String>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let source = InteractionSource::new(row.get::<_, String>(1)?, row.get::<_, String>(2)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    // 客户端来源在列尾：没有客户端事实的老行是空串，按「未知」处理。
    let source = match row.get::<_, Option<String>>(12)? {
        Some(client) if !client.trim().is_empty() => source
            .with_client(client)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        _ => source,
    };
    let scope = serde_json::from_str(&row.get::<_, String>(3)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let audience = serde_json::from_str(&row.get::<_, String>(4)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let reply_route = serde_json::from_str(&row.get::<_, String>(5)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let status = parse_status(&row.get::<_, String>(7)?)?;
    Ok(InteractionRecord {
        interaction_id,
        session_id: DaemonSessionId::new(row.get::<_, String>(13)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
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
        _ => Err(unknown_status(value)),
    }
}

/// 派生列出现词汇表之外的取值。
///
/// 行解码失败过去一律回 `rusqlite::Error::InvalidQuery`，它的 Display 是
/// 「Query is not read-only」，与真正的原因（状态列里有个陌生词）毫无关系，只能靠猜。
/// 把实际取值带出来，错误信息才自证。
fn unknown_status(value: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(UnknownInteractionStatus(value.to_owned())))
}

#[derive(Debug)]
struct UnknownInteractionStatus(String);

impl std::fmt::Display for UnknownInteractionStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "无法识别的 Interaction 状态：{}", self.0)
    }
}

impl std::error::Error for UnknownInteractionStatus {}

pub(super) fn runtime_error(error: impl std::fmt::Display) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

pub(super) fn storage_error(error: rusqlite::Error) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

pub(super) fn serialize_error(error: serde_json::Error) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}
