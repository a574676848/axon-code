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

/// 事件面写入失败的观测、水位补偿与有界重试。
#[path = "event_log_payload.rs"]
mod payload;

#[path = "event_log_write_failure.rs"]
mod write_failure;

/// 投影水位：同一批事件只投影一次。
#[path = "projection_watermark.rs"]
mod projection_watermark;

/// 派生投影的进程内单线程队列。
#[path = "projection_worker.rs"]
mod projection_worker;

/// 派生投影的重建入口：按会话从事件日志整体重放。
#[path = "projection_rebuild.rs"]
mod projection_rebuild;

pub(super) fn log_error(error: axon_eventlog::EventLogError) -> RuntimeStoreError {
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

    /*
     * 分配序号与落盘必须在同一把会话锁里完成。
     *
     * 两步之间原本没有互斥：两个写者按相反顺序落盘时，后落盘的一方必然撞上「序号与
     * 文件尾不一致」，只能回退水位并按文件尾重排；重排会改掉已经发布的序号，订阅端
     * 把这一批当成重复帧丢掉，界面上就是「服务端已收口、界面还在跑」。
     */
    let write_lock = store.event_log_write_lock(session_id);
    let write_guard = write_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    /*
     * 阶段 1：控制面事务。fence 校验、租约校验、分配序号、推进水位。
     *
     * 写锁争用是瞬时的（持有者几毫秒到几百毫秒后放锁），交给 `with_busy_retry` 重试；
     * 事务在错误路径整体回滚，重来一次不会把水位推进两次。
     */
    let allocate_started = std::time::Instant::now();
    let (start, end) = super::with_busy_retry(|| {
        allocate_event_sequence(
            store,
            session_id,
            host,
            events.len(),
            now_ms,
            interaction_lease,
        )
    })?;
    /*
     * 序号事务这一段不会进事件日志写者的分段埋点：它发生在控制面库，跨 crate 边界。
     * 超过 100ms 单独留痕，便于把「慢在号段分配」与「慢在写帧/fsync」区分开。
     */
    let allocate_ms = allocate_started.elapsed().as_millis();
    if allocate_ms >= 100 {
        axon_logger::warning(&format!(
            "事件序号分配偏慢：session={} 帧数={} 事务耗时={allocate_ms}ms",
            session_id.as_str(),
            events.len()
        ));
    }

    // 阶段 2：事件面追加。不持有任何数据库锁。
    let interaction_column = attribution
        .or(interaction_lease.map(|(interaction_id, _)| interaction_id))
        .map(InteractionId::as_str);
    let origin_channel = interaction_channel(store, interaction_column)?;
    let mut records = build_records(session_id, host, events, start, now_ms);
    let mut runtime_records = records
        .iter()
        .map(|record| to_runtime_record(record, interaction_column, origin_channel.as_deref()))
        .collect::<Vec<_>>();
    write_failure::append_preallocated(store, session_id, now_ms, &mut runtime_records, end)?;
    // 失败重试可能按文件尾重排了序号：返回给调用方的记录必须与落盘一致。
    for (record, runtime) in records.iter_mut().zip(&runtime_records) {
        record.sequence = runtime.sequence;
    }
    // 重排会改掉分配时的序号：投影水位跟落盘的真实批首序号走，才与日志里的帧对得上。
    let projected_from = records.first().map_or(start, |record| record.sequence);
    /*
     * 落盘序号小于分配序号，说明事件日志尾被清空或按更短的尾重排过：水位仍停在旧的高
     * 序号上，闸门会把这一批以及之后每一批都当成已投影而整批跳过。标记随批投递，由
     * worker 在执行前重置水位。
     */
    let log_rewound = projected_from < start;
    /*
     * 派生表投影在锁内投递、锁外执行。同一会话的批在写锁内按序号顺序入队，单 worker 按
     * 投递顺序消费，水位因此单调推进；投影若不在锁内投递，后一批可能先进队列，先跑完并
     * 把水位推高，前一批随后整批被跳过。
     */
    apply_projections(
        store,
        session_id,
        projected_from,
        events,
        origin_channel.as_deref(),
        now_ms,
        log_rewound,
    );
    drop(write_guard);
    Ok(records)
}

/// 阶段 1：控制面事务里分配本批序号并推进水位。
///
/// 单独成函数是为了让「一个完整事务」能整体交给 `with_busy_retry`：重试必须从事务
/// 开头重来，否则水位这种单调计数会被推进两次。成功时返回本批的 `(start, end)`。
fn allocate_event_sequence(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    event_count: usize,
    now_ms: u64,
    interaction_lease: Option<(&InteractionId, &str)>,
) -> Result<(u64, u64), RuntimeStoreError> {
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
    let count = u64::try_from(event_count)
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
    Ok((start, end))
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
    let records = axon_eventlog::read_after(&path, after_sequence, limit).map_err(log_error)?;
    // 整条会话回放同样要拿到正文：被分级的旧交互按归档回源，归档缺失时降级为占位。
    let records = super::event_body_tiering::hydrate_archived_page(store, session_id, records);
    Ok(records
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
    let records = axon_eventlog::read_interaction_after(
        &path,
        interaction_id.as_str(),
        after_sequence,
        limit,
    )
    .map_err(log_error)?;
    // 被分级的正文按需从归档回源；归档已回收时降级为占位记录。
    let records = super::event_body_tiering::hydrate_archived_records(
        store,
        session_id,
        interaction_id.as_str(),
        records,
    );
    Ok(records
        .iter()
        .map(|record| from_runtime_record(session_id, record))
        .collect())
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
    let records = axon_eventlog::read_interaction_tail(&path, interaction_id.as_str(), keep)
        .map_err(log_error)?;
    let records = super::event_body_tiering::hydrate_archived_records(
        store,
        session_id,
        interaction_id.as_str(),
        records,
    );
    Ok(records
        .iter()
        .map(|record| from_runtime_record(session_id, record))
        .collect())
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
    let payload = payload::with_interaction(payload, interaction_id);
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
/// 在调用方已持会话写锁、并已分配序号推进水位之后，只把事件写进事件面文件。
///
/// 调用前置条件有两条，缺一不可：批次属于同一个会话，且调用方已持有该会话的
/// [`event_log_write_lock`](SqliteRuntimeStore::event_log_write_lock)。原因与
/// [`write_records_locked`] 相同：分配序号与落盘必须落在同一把锁里，锁由调用方持有才能
/// 覆盖它自己的控制面事务。`std::sync::Mutex` 不可重入，持锁调用方不得再走任何自带取锁
/// 的入口。
pub(super) fn write_events_at_locked(
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
    let mut records = events
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
    let mut runtime_records = records
        .iter()
        .map(|record| to_runtime_record(record, interaction_id, origin_channel.as_deref()))
        .collect::<Vec<_>>();
    // 本次事务把水位推进到了这一批的末号；CAS 回退要以它为比对值。
    let written = records.last().map_or(start, |record| record.sequence);
    // 序号已由调用方分配，落盘这一半与同会话其他写者的互斥由调用方持有的会话锁负责。
    write_failure::append_preallocated(store, session_id, now_ms, &mut runtime_records, written)?;
    // 失败重试可能按文件尾重排了序号：返回给调用方的记录必须与落盘一致。
    for (record, runtime) in records.iter_mut().zip(&runtime_records) {
        record.sequence = runtime.sequence;
    }
    // 重排会改掉分配时的序号：投影水位跟落盘的真实批首序号走。
    let projected_from = records.first().map_or(start, |record| record.sequence);
    let log_rewound = projected_from < start;
    apply_projections(
        store,
        session_id,
        projected_from,
        events,
        origin_channel.as_deref(),
        now_ms,
        log_rewound,
    );
    Ok(records)
}
/// 逐会话批量把已分配序号的事件写进事件面文件。
///
/// 供跨会话的隔离与恢复路径使用：这些路径一次产出少量事件，序号已在控制面事务里
/// 分配好。同一会话的连续记录合并成一次追加，避免每条一次 fsync。
///
/// 序号与文件尾冲突时这里会补偿水位并按文件尾重排后重试（见写入失败模块）：
/// 调用方手里的 `SessionEventRecord` 是重排前的那一份，日志才是序号权威。
pub(super) fn write_records_locked(
    store: &SqliteRuntimeStore,
    batch: &mut [SessionEventRecord],
    interaction_id: Option<&str>,
) -> Result<(), RuntimeStoreError> {
    let Some(first) = batch.first().map(|record| record.session_id.clone()) else {
        return Ok(());
    };
    let origin_channel = interaction_channel(store, interaction_id)?;
    let session_id = &first;
    let mut runtime_records = batch
        .iter()
        .map(|record| to_runtime_record(record, interaction_id, origin_channel.as_deref()))
        .collect::<Vec<_>>();
    let written = batch.last().map_or(0, |record| record.sequence);
    write_failure::append_preallocated(
        store,
        session_id,
        batch.first().map_or(0, |record| record.emitted_at_ms),
        &mut runtime_records,
        written,
    )?;
    /*
     * 失败重试会按文件尾重排序号：把落盘后的真实序号回写给调用方。
     *
     * 不回写的话，调用方手里仍是重排前的号，实时桥与界面据此下发的游标与日志不符。
     */
    for (record, runtime) in batch.iter_mut().zip(&runtime_records) {
        record.sequence = runtime.sequence;
    }
    Ok(())
}
/// 把本批事件交给投影队列：逐轮事实、会话累计指标与上下文快照。
///
/// 投影是可重建的派生数据，失败不阻断主流程；执行在进程内单线程 worker 里，写路径只投递
/// 拥有所有权的副本，见 `projection_worker`。上下文快照不走第二条写路径：这一处是它唯一
/// 的写入点。
///
/// `start_sequence` 是本批首条事件的序号，同时是本批在投影水位里的身份：同一批被重放
/// （重试、补投）时整批跳过，累加型投影因此不会翻倍。水位闸门在 worker 内判定，投递时
/// 不判定：投递时刻与执行时刻的水位可能不同。水位只在三个投影全部成功后推进；失败批次
/// 不推进，但后续成功的批次会把水位越过它，因此失败批的派生态要靠重建入口补齐。
/// 事件日志序号回退（重排把批首压到水位之下）时，重排处会先清水位再投递。
///
/// 调用方必须持有该会话的事件面写入锁：同一会话的投递顺序就是投影顺序。
/// 需要兜底时用 `rebuild_session_metrics` 整体重放。
pub(super) fn apply_projections(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
    events: &[(String, String)],
    origin_channel: Option<&str>,
    now_ms: u64,
    log_rewound: bool,
) {
    projection_worker::submit(
        store,
        session_id,
        start_sequence,
        events,
        origin_channel,
        now_ms,
        log_rewound,
    );
}

#[cfg(test)]
#[path = "event_log_repository_tests.rs"]
mod tests;
