//! 会话事件的文件写入与读取。
//!
//! 序号由控制面分配（`sessions.last_event_sequence`），文件只承载内容。
//! 写入顺序固定为「先控制面事务、后文件追加」：崩溃时最多出现"水位大于文件尾"，
//! 恢复逻辑据此回退水位并补一条 `session_recovered`。

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
    append_events_with(
        store,
        session_id,
        host,
        events,
        now_ms,
        interaction_lease,
        None,
    )
}

/// 只写归属、不校验租约的追加：轮次已收口时用。
///
/// 租约是「这一轮还在跑」的凭据，收口之后就必然失效；而逐轮指标、终态摘要这类事实
/// 恰恰在收口之后才写。本入口因此只做归属，不碰状态机：既不打开「无租约也能改状态」
/// 的口子，也不让先收口的轮次永远拿不到自己的读数。
pub(super) fn append_attributed_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    interaction_id: &InteractionId,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    append_events_with(
        store,
        session_id,
        host,
        events,
        now_ms,
        None,
        Some(interaction_id),
    )
}

fn append_events_with(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: &[(String, String)],
    now_ms: u64,
    interaction_lease: Option<(&InteractionId, &str)>,
    attribution: Option<&InteractionId>,
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
    let interaction_column = attribution
        .or(interaction_lease.map(|(interaction_id, _)| interaction_id))
        .map(InteractionId::as_str);
    let origin_channel = interaction_channel(store, interaction_column)?;
    let records = build_records(session_id, host, events, start, now_ms);
    let runtime_records = records
        .iter()
        .map(|record| to_runtime_record(record, interaction_column, origin_channel.as_deref()))
        .collect::<Vec<_>>();
    if let Err(error) = axon_eventlog::append_once(
        &store.event_log(session_id),
        start,
        now_ms,
        &runtime_records,
    ) {
        // 第二段失败：控制面已提交，把水位退回落盘尾再上报。
        compensate_watermark_after_log_failure(store, session_id);
        return Err(log_error(error));
    }
    apply_projections(store, session_id, events, origin_channel.as_deref(), now_ms);
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
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    // 分页只解压覆盖请求范围的尾部帧：历史越长越不能每次全量解压。
    Ok(axon_eventlog::read_after(&path, after_sequence, limit)
        .map_err(log_error)?
        .iter()
        .map(|record| from_runtime_record(session_id, record))
        .collect())
}

/// 读取某条 Interaction 在 `after_sequence` 之后的至多 `limit` 条事件。
pub(super) fn replay_interaction_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &InteractionId,
    after_sequence: u64,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let path = store.event_log(session_id).path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    // 按键取帧：索引的交互指纹把解压范围收窄到这条交互自己的帧，不再整会话解压。
    Ok(
        axon_eventlog::read_interaction_after(
            &path,
            interaction_id.as_str(),
            after_sequence,
            limit,
        )
        .map_err(log_error)?
        .iter()
        .map(|record| from_runtime_record(session_id, record))
        .collect(),
    )
}

/// 读取一组 Interaction 各自最近一条 `turn_metrics` 事件。
///
/// 只解压「可能属于这些交互」的帧：指纹粗筛加逐条确认，一页历史因此是每条交互一次
/// 尾部定位，而不是把整条会话解压一遍。事件不存在（CLI 直跑、老会话）的轮次不出现在
/// 结果里，调用方按读数未知降级。
pub(super) fn latest_turn_metrics(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_ids: &[InteractionId],
) -> Result<Vec<(String, axon_eventlog::TurnMetrics)>, RuntimeStoreError> {
    if interaction_ids.is_empty() {
        return Ok(Vec::new());
    }
    let path = store.event_log(session_id).path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let wanted: Vec<&str> = interaction_ids.iter().map(InteractionId::as_str).collect();
    let kind = axon_eventlog::RuntimeEventKind::TurnMetrics.as_str();
    let records =
        axon_eventlog::read_latest_per_interaction(&path, &wanted, kind).map_err(log_error)?;
    Ok(records
        .into_iter()
        .filter_map(|entry| {
            let metrics =
                serde_json::from_value::<axon_eventlog::TurnMetrics>(entry.record.payload).ok()?;
            Some((entry.interaction_id, metrics))
        })
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
    let keep = usize::try_from(limit).unwrap_or(usize::MAX);
    // 尾窗同样从尾部倒推：凑够该交互的条数就停，且按交互指纹跳过别条交互的帧。
    Ok(
        axon_eventlog::read_interaction_tail(&path, interaction_id.as_str(), keep)
            .map_err(log_error)?
            .iter()
            .map(|record| from_runtime_record(session_id, record))
            .collect(),
    )
}

/// 事件面真正落盘的末条序号与 generation；日志不存在或没有完整帧时返回 `None`。
///
/// 撕裂尾不算已落盘：它会被写侧截断，水位不该跟着它走。
///
/// 只解压末帧：恢复路径要给每个待恢复会话核对水位，逐会话全量解压会把一次恢复
/// 放大成「会话数 × 日志大小」的读放大。
pub(super) fn durable_tail(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) -> Result<Option<(u64, Option<String>)>, RuntimeStoreError> {
    let path = store.event_log(session_id).path();
    if !path.is_file() {
        return Ok(None);
    }
    let tail = axon_eventlog::read_tail_record(&path).map_err(log_error)?;
    Ok(tail.map(|record| (record.sequence, record.generation.clone())))
}

/// 事件面写入失败后的水位补偿：把控制面水位退回落盘尾。
///
/// 两段式的第二段失败时控制面已经提交，幂等重放会看到「水位有、帧没有」。
/// 把水位退回落盘尾可以让下一次写入从真实尾部继续，避免序号空洞；
/// 补偿本身失败只上报，最终由恢复路径的对账兜底。
fn compensate_watermark_after_log_failure(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) {
    let durable = match durable_tail(store, session_id) {
        Ok(tail) => tail.map_or(0, |(sequence, _)| sequence),
        Err(error) => {
            eprintln!(
                "事件面写入失败后读不到落盘尾，水位补偿放弃: session={} error={error}",
                session_id.as_str()
            );
            return;
        }
    };
    let result = store.connection().and_then(|connection| {
        connection
            .execute(
                "UPDATE sessions SET last_event_sequence=?1 WHERE session_id=?2",
                params![
                    i64::try_from(durable)
                        .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?,
                    session_id.as_str()
                ],
            )
            .map(|_| ())
            .map_err(storage)
    });
    if let Err(error) = result {
        eprintln!(
            "事件面写入失败后水位补偿失败: session={} durable={durable} error={error}",
            session_id.as_str()
        );
    }
}

/// 会话事件日志的物理路径（供诊断与清理使用）。
#[must_use]
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
    origin_channel: Option<&str>,
) -> axon_eventlog::RuntimeEventRecord {
    let payload = serde_json::from_str::<serde_json::Value>(&record.payload_json)
        .unwrap_or_else(|_| serde_json::Value::String(record.payload_json.clone()));
    axon_eventlog::RuntimeEventRecord {
        sequence: record.sequence,
        emitted_at_ms: record.emitted_at_ms,
        generation: Some(record.generation.clone()),
        interaction_id: interaction_id.map(str::to_owned),
        origin_channel: origin_channel.map(str::to_owned),
        // 这三个写入口都来自外部渠道触发的一轮执行（交互、动作、失败、隔离），
        // 因此来源类别固定是外部；定时与目标推进的上游另有生产者，不经过这里。
        origin_kind: axon_eventlog::TurnOrigin::External,
        event_type: axon_eventlog::RuntimeEventKind::from_str_lossy(&record.event_type),
        payload,
    }
}

/// 入站事实的来源渠道：只取一条，缺失时按内部轮次处理。
fn interaction_channel(
    store: &SqliteRuntimeStore,
    interaction_id: Option<&str>,
) -> Result<Option<String>, RuntimeStoreError> {
    let Some(interaction_id) = interaction_id else {
        return Ok(None);
    };
    let connection = store.connection()?;
    connection
        .query_row(
            "SELECT CASE WHEN TRIM(client)='' THEN source_channel ELSE client END FROM inbox WHERE interaction_id=?1",
            params![interaction_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage)
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
    let origin_channel = interaction_channel(store, interaction_id)?;
    let runtime_records = records
        .iter()
        .map(|record| to_runtime_record(record, interaction_id, origin_channel.as_deref()))
        .collect::<Vec<_>>();
    if let Err(error) = axon_eventlog::append_once(
        &store.event_log(session_id),
        start,
        now_ms,
        &runtime_records,
    ) {
        compensate_watermark_after_log_failure(store, session_id);
        return Err(log_error(error));
    }
    apply_projections(store, session_id, events, origin_channel.as_deref(), now_ms);
    Ok(records)
}
/// 逐会话批量把已分配序号的事件写进事件面文件。
///
/// 供跨会话的隔离与恢复路径使用：这些路径一次产出少量事件，序号已在控制面事务里
/// 分配好。同一会话的连续记录合并成一次追加，避免每条一次 fsync。
pub(super) fn write_records(
    store: &SqliteRuntimeStore,
    records: &[SessionEventRecord],
    interaction_id: Option<&str>,
) -> Result<(), RuntimeStoreError> {
    let origin_channel = interaction_channel(store, interaction_id)?;
    let mut index = 0usize;
    while index < records.len() {
        let session_id = &records[index].session_id;
        let mut end = index + 1;
        while end < records.len() && &records[end].session_id == session_id {
            end += 1;
        }
        let batch = &records[index..end];
        let runtime_records = batch
            .iter()
            .map(|record| to_runtime_record(record, interaction_id, origin_channel.as_deref()))
            .collect::<Vec<_>>();
        if let Err(error) = axon_eventlog::append_once(
            &store.event_log(session_id),
            batch[0].sequence,
            batch[0].emitted_at_ms,
            &runtime_records,
        ) {
            compensate_watermark_after_log_failure(store, session_id);
            return Err(log_error(error));
        }
        index = end;
    }
    Ok(())
}
/// 把本轮指标投射到 `session_metrics`。
///
/// 投影是可重建的派生数据，失败不阻断主流程；但失败必须上报，否则「累计耗时恒 null」
/// 这类问题只能等界面报错才发现。需要兜底时用 `rebuild_session_metrics` 整体重放。
fn apply_projections(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    origin_channel: Option<&str>,
    now_ms: u64,
) {
    for (event_type, payload_json) in events {
        if event_type != "turn_metrics" {
            continue;
        }
        let Ok(metrics) = serde_json::from_str::<axon_eventlog::TurnMetrics>(payload_json) else {
            continue;
        };
        if let Err(error) = super::session_metrics_repository::apply_turn_metrics(
            store,
            session_id,
            &metrics,
            origin_channel,
            now_ms,
        ) {
            eprintln!(
                "会话累计指标投影写入失败，可重建: session={} error={error}",
                session_id.as_str()
            );
        }
    }
}
