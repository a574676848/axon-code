//! 会话事件的文件写入与读取。
//!
//! 序号由控制面分配（`sessions.last_event_sequence`），文件只承载内容。
//! 写入顺序固定为「先控制面事务、后文件追加」：崩溃时最多出现"水位大于文件尾"，
//! 恢复逻辑据此回退水位并补一条 `session_recovered`。

use std::path::Path;

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::RuntimeStoreError;
use axon_domain::{DaemonSessionId, InteractionId};

fn log_error(error: axon_eventlog::EventLogError) -> RuntimeStoreError {
    RuntimeStoreError::Storage(error.to_string())
}

/// 追加一批事件：先在控制面 fence 并推进水位，再写事件面文件。
///
/// `interaction_lease` 非空时，额外校验该 Interaction 仍处于 processing 且租约未过期，
/// 并把归属落到记录上，供按 Interaction 定向取帧。
pub(super) fn append_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: &[(String, String)],
    now_ms: u64,
    interaction_lease: Option<(&InteractionId, &str)>,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }

    // 阶段 1：控制面事务。fence 校验、租约校验、分配序号、推进水位。
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::check_fence(&transaction, host, now_ms)?;
    if let Some((interaction_id, lease_token)) = interaction_lease {
        let active = transaction
            .query_row(
                "SELECT 1 FROM work_items WHERE inbox_id=?1 AND status='running' \
                 AND lease_token=?2 AND lease_expires_at_ms>?3",
                params![interaction_id.as_str(), lease_token, i64_value(now_ms)?],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage)?;
        if active.is_none() {
            return Err(RuntimeStoreError::Fenced);
        }
    }
    let previous: i64 = transaction
        .query_row(
            "SELECT last_event_sequence FROM sessions WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| RuntimeStoreError::NotFound(session_id.to_string()))?;
    let start = u64::try_from(previous)
        .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?
        .checked_add(1)
        .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
    let count = u64::try_from(events.len())
        .map_err(|_| RuntimeStoreError::Storage("事件批次过大".to_owned()))?;
    let end = start
        .checked_add(count.saturating_sub(1))
        .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
    transaction
        .execute(
            "UPDATE sessions SET last_event_sequence=?1,last_activity_at_ms=?2,\
             owner_host_id=?3,owner_generation=?4 WHERE session_id=?5",
            params![
                i64_value(end)?,
                i64_value(now_ms)?,
                host.host_id.as_str(),
                host.generation,
                session_id.as_str()
            ],
        )
        .map_err(storage)?;
    transaction.commit().map_err(storage)?;

    // 阶段 2：事件面追加。不持有任何数据库锁。
    let interaction_column = interaction_lease.map(|(interaction_id, _)| interaction_id.as_str());
    let records = build_records(session_id, host, events, start, now_ms);
    let runtime_records = records
        .iter()
        .map(|record| to_runtime_record(record, interaction_column))
        .collect::<Vec<_>>();
    axon_eventlog::append_once(
        &store.event_log(session_id),
        start,
        now_ms,
        &runtime_records,
    )
    .map_err(log_error)?;
    apply_projections(store, session_id, events, now_ms);
    Ok(records)
}

/// 读取序号严格大于 `after_sequence` 的至多 `limit` 条事件。
pub(super) fn replay_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    after_sequence: u64,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let path = store.event_log(session_id).path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let reader = axon_eventlog::EventLogReader::open(&path).map_err(log_error)?;
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    Ok(reader
        .read_from(after_sequence, limit)
        .iter()
        .map(|record| from_runtime_record(session_id, record))
        .collect())
}

/// 读取某条 Interaction 自身的尾窗帧，按序号升序。
pub(super) fn interaction_tail_window(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &InteractionId,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let path = store.event_log(session_id).path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let reader = axon_eventlog::EventLogReader::open(&path).map_err(log_error)?;
    let matching = reader
        .records()
        .iter()
        .filter(|record| record.interaction_id.as_deref() == Some(interaction_id.as_str()))
        .collect::<Vec<_>>();
    let keep = usize::try_from(limit).unwrap_or(usize::MAX);
    let start = matching.len().saturating_sub(keep);
    Ok(matching[start..]
        .iter()
        .map(|record| from_runtime_record(session_id, record))
        .collect())
}

/// 会话事件日志的物理路径（供诊断与清理使用）。
#[must_use]
pub(super) fn event_log_path(store: &SqliteRuntimeStore, session_id: &str) -> std::path::PathBuf {
    axon_eventlog::EventLog::open(store.events_root(), session_id).path()
}

/// 事件日志根目录是否存在可读日志。
#[must_use]
pub(super) fn has_event_log(store: &SqliteRuntimeStore, session_id: &str) -> bool {
    Path::new(&event_log_path(store, session_id)).exists()
}

fn build_records(
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: &[(String, String)],
    start: u64,
    now_ms: u64,
) -> Vec<SessionEventRecord> {
    events
        .iter()
        .enumerate()
        .map(|(offset, (event_type, payload_json))| SessionEventRecord {
            session_id: session_id.clone(),
            sequence: start.saturating_add(u64::try_from(offset).unwrap_or(0)),
            generation: host.generation.clone(),
            event_type: event_type.clone(),
            payload_json: payload_json.clone(),
            emitted_at_ms: now_ms,
        })
        .collect()
}

fn to_runtime_record(
    record: &SessionEventRecord,
    interaction_id: Option<&str>,
) -> axon_eventlog::RuntimeEventRecord {
    let payload = serde_json::from_str::<serde_json::Value>(&record.payload_json)
        .unwrap_or_else(|_| serde_json::Value::String(record.payload_json.clone()));
    axon_eventlog::RuntimeEventRecord {
        sequence: record.sequence,
        emitted_at_ms: record.emitted_at_ms,
        generation: Some(record.generation.clone()),
        interaction_id: interaction_id.map(str::to_owned),
        origin_channel: None,
        origin_kind: axon_eventlog::TurnOrigin::External,
        event_type: axon_eventlog::RuntimeEventKind::from_str_lossy(&record.event_type),
        payload,
    }
}

fn from_runtime_record(
    session_id: &DaemonSessionId,
    record: &axon_eventlog::RuntimeEventRecord,
) -> SessionEventRecord {
    let payload_json = match &record.payload {
        serde_json::Value::String(raw) => raw.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "null".to_owned()),
    };
    SessionEventRecord {
        session_id: session_id.clone(),
        sequence: record.sequence,
        generation: record.generation.clone().unwrap_or_default(),
        event_type: record.event_type.as_str().to_owned(),
        payload_json,
        emitted_at_ms: record.emitted_at_ms,
    }
}
/// 在调用方已经分配序号并推进水位之后，只把事件写进事件面文件。
///
/// 供 `commit_action`、`fail_action` 与隔离路径使用：它们在同一个控制面事务里
/// 已完成 fence、序号分配与水位推进，事务提交后再落帧。
pub(super) fn write_events_at(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    generation: &str,
    start: u64,
    events: &[(String, String)],
    now_ms: u64,
    interaction_id: Option<&str>,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let records = events
        .iter()
        .enumerate()
        .map(|(offset, (event_type, payload_json))| SessionEventRecord {
            session_id: session_id.clone(),
            sequence: start.saturating_add(u64::try_from(offset).unwrap_or(0)),
            generation: generation.to_owned(),
            event_type: event_type.clone(),
            payload_json: payload_json.clone(),
            emitted_at_ms: now_ms,
        })
        .collect::<Vec<_>>();
    let runtime_records = records
        .iter()
        .map(|record| to_runtime_record(record, interaction_id))
        .collect::<Vec<_>>();
    axon_eventlog::append_once(
        &store.event_log(session_id),
        start,
        now_ms,
        &runtime_records,
    )
    .map_err(log_error)?;
    apply_projections(store, session_id, events, now_ms);
    Ok(records)
}
/// 逐条把已分配序号的事件写进事件面文件。
///
/// 供跨会话的隔离与恢复路径使用：这些路径一次只产出少量事件，且序号已在控制面
/// 事务里分配好。按条追加即可，不做批次合并。
pub(super) fn write_records(
    store: &SqliteRuntimeStore,
    records: &[SessionEventRecord],
    interaction_id: Option<&str>,
) -> Result<(), RuntimeStoreError> {
    for record in records {
        let runtime_record = to_runtime_record(record, interaction_id);
        axon_eventlog::append_once(
            &store.event_log(&record.session_id),
            record.sequence,
            record.emitted_at_ms,
            std::slice::from_ref(&runtime_record),
        )
        .map_err(log_error)?;
    }
    Ok(())
}
/// 把本轮指标投射到 `session_metrics`。
///
/// 投影失败不阻断主流程：它是可重建的派生数据，下一次重放会修正。
fn apply_projections(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    now_ms: u64,
) {
    for (event_type, payload_json) in events {
        if event_type != "turn_metrics" {
            continue;
        }
        let Ok(metrics) = serde_json::from_str::<axon_eventlog::TurnMetrics>(payload_json) else {
            continue;
        };
        let _ = super::session_metrics_repository::apply_turn_metrics(
            store, session_id, &metrics, None, now_ms,
        );
    }
}
