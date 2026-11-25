//! 事件面写入失败的观测、水位补偿与有界重试。
//!
//! 写入顺序固定为「控制面事务先推进水位、事件面后落盘」，因此任何一次事件面失败都会留下
//! 「水位有、帧没有」的半成品状态。本模块把这种状态收敛回一致：先留观测事实，再把本次
//! 写入推进的水位按 CAS 退回落盘尾，最后按文件尾重新分配序号并重试，保证并发失败方的
//! 事件最终仍然落盘，而不是把不变量的修复转嫁给每个调用方。

use rusqlite::params;

use super::super::SqliteRuntimeStore;
use super::super::helpers::{i64_value, storage};
use super::{durable_tail, log_error};
use crate::ports::RuntimeStoreError;
use axon_domain::DaemonSessionId;

/// 单次追加的最大尝试次数：并发冲突允许重试，但不允许无限重跑。
pub(super) const MAX_APPEND_ATTEMPTS: u32 = 3;

/// 追加一批已经分配好序号的事件；序号与文件尾冲突时按文件尾重排并重试。
///
/// `written_sequence` 是调用方这一次写进控制面水位的值，用于 CAS 回退：只有水位仍停在
/// 它上面时回退才是正确的，否则说明更晚的写者已经接管，回退会抹掉它领走的号。
pub(super) fn append_preallocated(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    created_at_ms: u64,
    records: &mut [axon_eventlog::RuntimeEventRecord],
    written_sequence: u64,
) -> Result<(), RuntimeStoreError> {
    if records.is_empty() {
        return Ok(());
    }
    let mut written = written_sequence;
    let mut attempt = 0_u32;
    loop {
        attempt = attempt.saturating_add(1);
        let expected_next = records[0].sequence;
        match axon_eventlog::append_once(
            &store.event_log(session_id),
            expected_next,
            created_at_ms,
            records,
        ) {
            Ok(_) => return Ok(()),
            Err(error) => {
                let retryable = observe_failure(session_id, expected_next, &error);
                compensate_watermark(store, session_id, written);
                if !retryable || attempt >= MAX_APPEND_ATTEMPTS {
                    return Err(log_error(error));
                }
            }
        }
        written = realign_sequences(store, session_id, records)?;
    }
}

/// 观测失败并判定是否可重试。
///
/// 「序号与文件尾不一致」是并发写者竞争后的可恢复失败，必须带齐 `session_id`、
/// `expected_next`、`file_tail` 三个事实再退水位重排；其余失败一律不可重试，按原样上报，
/// 用原始错误文本区分 IO、损坏与抢锁超时。
///
/// 这里用穷举 `match` 且不写通配 `_`：`EventLogError` 以后新增变体时本函数会编译失败，
/// 强迫后来者显式判定新变体的观测口径与重试语义。
fn observe_failure(
    session_id: &DaemonSessionId,
    expected_next: u64,
    error: &axon_eventlog::EventLogError,
) -> bool {
    match error {
        axon_eventlog::EventLogError::SequenceTailMismatch { expected, actual } => {
            report_structured(&format!(
                "事件日志序号与文件尾不一致，已回滚水位待重试: session_id={} expected_next={} file_tail={}",
                session_id.as_str(),
                expected,
                actual
            ));
            true
        }
        /*
         * 句柄代际失效：日志刚被整体重写，常驻句柄已经指向被替换的文件。
         * 这类失败必须重试：重新打开的句柄会读新文件，重试即成功；直接上报就等于
         * 在重写窗口里丢掉这一批事件。
         */
        axon_eventlog::EventLogError::StaleHandle => {
            report_structured(&format!(
                "事件日志句柄已随重写作废，已重新打开待重试: session_id={} expected_next={}",
                session_id.as_str(),
                expected_next
            ));
            true
        }
        axon_eventlog::EventLogError::Io(_)
        | axon_eventlog::EventLogError::Codec(_)
        | axon_eventlog::EventLogError::SequenceGap { .. }
        | axon_eventlog::EventLogError::UnsupportedVersion(_)
        | axon_eventlog::EventLogError::Corrupt(_)
        | axon_eventlog::EventLogError::Locked
        | axon_eventlog::EventLogError::EmptyBatch => {
            report_structured(&format!(
                "事件日志写入失败: session_id={} expected_next={} error={error}",
                session_id.as_str(),
                expected_next
            ));
            false
        }
    }
}

/// 失败事实同时写日志文件与 stderr：日志文件是线上定位依据，stderr 是不依赖日志
/// 初始化的兜底通道（CLI 直跑时也能立刻看到）。
fn report_structured(message: &str) {
    axon_logger::error(message);
    eprintln!("{message}");
}

/// 把控制面水位按 CAS 退回落盘尾；补偿本身失败只上报，最终由恢复路径的对账兜底。
///
/// 不能无条件回退：并发场景里另一个写者可能已经领走更大的号并成功落盘，把水位硬拉回
/// 文件尾会抹掉它，随后每一次写入都永远对不上尾。
pub(super) fn compensate_watermark(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    written_sequence: u64,
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
    if let Err(error) = rollback_watermark(store, session_id, durable, written_sequence) {
        eprintln!(
            "事件面写入失败后水位补偿失败: session={} durable={durable} error={error}",
            session_id.as_str()
        );
    }
}

/// 条件回退：只有水位仍等于 `written_sequence` 时才写回 `durable`。
fn rollback_watermark(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    durable: u64,
    written_sequence: u64,
) -> Result<(), RuntimeStoreError> {
    let connection = store.connection()?;
    connection
        .execute(
            "UPDATE sessions SET last_event_sequence=?1 WHERE session_id=?2 AND last_event_sequence=?3",
            params![
                i64_value(durable)?,
                session_id.as_str(),
                i64_value(written_sequence)?
            ],
        )
        .map(|_| ())
        .map_err(storage)
}

/// 按文件尾重新分配序号并推进控制面水位；返回本次写入推进到的水位值。
///
/// 抢号用「水位不超前于落盘尾」（`last_event_sequence <= durable`）做 CAS，三种水位关系
/// 各有语义：
/// - **落后**于落盘尾（`<`）：缓存已被别的写者补偿过，但文件尾才是位置权威。此时必须允许
///   把水位前推到 `durable + count`，否则并发失败方的事件只能丢。典型时序：A 领 57、B 领 58，
///   B 先抢到锁而失败并把水位退到 56，A 随后以尾 56 落盘 57，此时水位 56 落后于尾 57，
///   B 的 58 只有前推水位才写得进去。
/// - **等于**落盘尾：正常情形，直接认领。
/// - **超前**于落盘尾（`>`）：更晚的写者已经领走文件尾之后的一段、尚未落盘。这一段号已被
///   占用，抢过来会与它将来的落盘冲突，必须拒绝；本次追加失败但水位未被破坏。
///
/// 放行「落后」不会抢到别人的号：任何一次领号都会把水位推进到该批的末号，因此
/// 「水位不超前于落盘尾」本身就证明文件尾之后没有活跃领号。
pub(super) fn realign_sequences(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    records: &mut [axon_eventlog::RuntimeEventRecord],
) -> Result<u64, RuntimeStoreError> {
    let durable = durable_tail(store, session_id)?.map_or(0, |(sequence, _)| sequence);
    let count = u64::try_from(records.len())
        .map_err(|_| RuntimeStoreError::Storage("事件批次过大".to_owned()))?;
    let start = durable.saturating_add(1);
    let written = start.saturating_add(count.saturating_sub(1));
    let connection = store.connection()?;
    let claimed = connection
        .execute(
            "UPDATE sessions SET last_event_sequence=?1 WHERE session_id=?2 AND last_event_sequence<=?3",
            params![
                i64_value(written)?,
                session_id.as_str(),
                i64_value(durable)?
            ],
        )
        .map_err(storage)?;
    if claimed == 0 {
        return Err(RuntimeStoreError::Storage(format!(
            "事件日志重排失败，水位已被更晚的写者推进: session={} durable={durable}",
            session_id.as_str()
        )));
    }
    axon_logger::warning(&format!(
        "事件日志序号与文件尾不符，按落盘尾重排: session_id={} start={start} end={written}",
        session_id.as_str()
    ));
    for (offset, record) in records.iter_mut().enumerate() {
        record.sequence = start.saturating_add(u64::try_from(offset).unwrap_or(0));
    }
    Ok(written)
}
