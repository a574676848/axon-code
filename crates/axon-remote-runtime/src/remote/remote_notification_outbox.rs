//! 远控通知 adapter outbox。
//!
//! 该模块把 fanout 计划转换为本机可审计的 adapter dispatch 记录；不发送网络、不保存 raw payload。

use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult, RuntimeFailure, SessionId};
use axon_protocol::wire::WireEventKind;
use serde::{Deserialize, Serialize};

use crate::{
    REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS, REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS,
    REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS, REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS,
    RemoteNotificationFanoutPlan, RemoteNotificationKind, RemoteStateStoreIndex,
    RemoteTransportKind,
};

#[path = "remote_notification_outbox_coalescing.rs"]
mod coalescing;
#[path = "remote_notification_timing.rs"]
mod timing;
#[path = "remote_notification_outbox_validation.rs"]
mod validation;
pub use timing::{RemoteNotificationThrottlePolicy, RemoteNotificationTimingPolicy};
use validation::{is_pending, validate_input, validate_record};

/// 远控通知 outbox 文件名。
pub const REMOTE_NOTIFICATION_OUTBOX_FILE: &str = "remote_notification_outbox.sqlite3";
/// 远控通知 outbox schema 版本。
pub const REMOTE_NOTIFICATION_OUTBOX_SCHEMA_VERSION: u32 = 3;
/// 远控通知 outbox dispatch id 最大字符数。
pub const REMOTE_NOTIFICATION_OUTBOX_DISPATCH_ID_MAX_CHARS: usize = 96;
/// 远控通知 outbox source trace 最大字符数。
pub const REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS: usize = 160;
/// 远控通知 outbox 原因最大字符数。
pub const REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS: usize = 160;
/// 远控通知 outbox adapter 错误摘要最大字符数。
///
/// 上游供应商的错误体本身就可能很长（例如飞书 OpenAPI 的 `field_violations` +
/// `log_id` + troubleshooter 链接，实测约 495 字符）。上界过紧会把「网络确实失败了」
/// 判成「outcome 无效」，使这条失败连记都记不下来、重试通道整体失效：因此这里留出
/// 足够容纳完整诊断信息的余量。
pub const REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS: usize = 1024;
/// 远控通知 outbox adapter message id 最大字符数。
pub const REMOTE_NOTIFICATION_OUTBOX_ADAPTER_MESSAGE_ID_MAX_CHARS: usize = 128;
/// 远控通知 outbox 最大尝试次数。
pub const REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS: u32 = 8;
/// 远控通知 outbox 最多保留的当前 dispatch 数。
pub const REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS: usize = 512;
const MINUTES_PER_DAY: u16 = 24 * 60;
/// 远控通知 outbox 记录计划原因。
pub const REMOTE_NOTIFICATION_OUTBOX_REASON_PLANNED: &str = "remote_notification_outbox:planned";
/// 远控通知 outbox 网络关闭原因。
pub const REMOTE_NOTIFICATION_OUTBOX_REASON_NETWORK_CLOSED: &str =
    "remote_notification_outbox:network_closed";
/// 远控通知 outbox raw payload 关闭原因。
pub const REMOTE_NOTIFICATION_OUTBOX_REASON_RAW_PAYLOAD_CLOSED: &str =
    "remote_notification_outbox:raw_payload_closed";
/// 远控通知 outbox 最新状态折叠原因。
pub const REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED: &str =
    "remote_notification_outbox:status_folded";
/// attempting 恢复 marker。
pub const REMOTE_NOTIFICATION_OUTBOX_REASON_ATTEMPT_RECOVERED: &str =
    "remote_notification_outbox:attempt_recovered";
/// attempting 状态写入原因。
pub const REMOTE_NOTIFICATION_OUTBOX_REASON_ATTEMPTING: &str =
    "remote_notification_outbox:attempting";
const OUTBOX_STATE_NAMESPACE: &str = "remote_notification_outbox";
const OUTBOX_STATUS_PENDING: &str = "pending";
const OUTBOX_STATUS_ATTEMPTING: &str = "attempting";
const OUTBOX_STATUS_DELIVERED: &str = "delivered";
const OUTBOX_STATUS_FAILED: &str = "failed";
const OUTBOX_STATUS_ABANDONED: &str = "abandoned";

/// 远控通知 outbox dispatch 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteNotificationDispatchStatus {
    /// 等待本地 adapter 消费。
    Pending,
    /// 已写入发送检查点，网络请求可能已经发生。
    Attempting,
    /// adapter 已确认投递。
    Delivered,
    /// 明确失败，可按 next_retry_after_ms 重试。
    Failed,
    /// 已终止，不再投递。
    Abandoned,
}

/// 远控通知 outbox 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationOutboxInput {
    /// dispatch id。
    pub dispatch_id: String,
    /// fanout 计划。
    pub plan: RemoteNotificationFanoutPlan,
    /// dispatch 状态。
    pub status: RemoteNotificationDispatchStatus,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// adapter 尝试次数。
    pub attempt_count: u32,
    /// 下次重试时间。
    pub next_retry_after_ms: Option<u64>,
    /// adapter 错误摘要。
    pub last_error: Option<String>,
    /// adapter 返回的有界消息 ID。
    pub adapter_message_id: Option<String>,
    /// source trace。
    pub source_trace: String,
    /// 原因。
    pub reason: String,
}

/// 可持久化远控通知 dispatch 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteNotificationOutboxRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// dispatch id。
    pub dispatch_id: String,
    /// 目标会话。
    pub session_id: u64,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// dispatch 状态。
    pub status: RemoteNotificationDispatchStatus,
    /// 投递通知数量。
    pub delivery_count: usize,
    /// 因 batch 或订阅过滤丢弃的数量。
    pub dropped_count: usize,
    /// 投递通知类别。
    pub delivery_kinds: Vec<RemoteNotificationKind>,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// adapter 尝试次数。
    pub attempt_count: u32,
    /// 下次重试时间。
    pub next_retry_after_ms: Option<u64>,
    /// adapter 错误摘要。
    pub last_error: Option<String>,
    /// 运行失败的权威结构化事实。
    pub failure: Option<RuntimeFailure>,
    /// adapter 返回的有界消息 ID。
    #[serde(default)]
    pub adapter_message_id: Option<String>,
    /// 最近一次发送开始时间。
    #[serde(default)]
    pub attempt_started_at_ms: Option<u64>,
    /// 最近一次发送 owner，包含进程世代信息。
    #[serde(default)]
    pub attempt_owner: Option<String>,
    /// 崩溃恢复 marker。
    #[serde(default)]
    pub recovery_marker: Option<String>,
    /// source trace。
    pub source_trace: String,
    /// 原因。
    pub reason: String,
    /// 来源 Session Event 的单调序号。
    ///
    /// 同一毫秒内的多条记录据此恢复真实先后；`dispatch_id` 是哈希值，其大小序与事件
    /// 先后无关，不能作为次序键。缺失表示早于本字段的历史记录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// 是否只允许本地 adapter 消费。
    pub local_only: bool,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
    /// 是否持久化 raw body。
    pub raw_body_persisted: bool,
    /// 是否持久化 raw payload。
    pub raw_payload_persisted: bool,
}

/// 远控通知 outbox 查询结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationOutboxPending {
    /// 目标会话。
    pub session_id: SessionId,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 待本地 adapter 消费记录。
    pub records: Vec<RemoteNotificationOutboxRecord>,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// outbox 写入后的容量回收结果。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RemoteNotificationOutboxAppendReport {
    /// 为给新 dispatch 腾出容量而回收的 dispatch id。
    pub evicted_dispatch_ids: Vec<String>,
    /// 本次写入被合并到的既有 dispatch id。
    pub coalesced_dispatch_id: Option<String>,
}

/// 远控通知 outbox runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationOutboxRuntime;
impl RemoteNotificationOutboxRecord {
    /// 从 fanout plan 生成可持久化 dispatch 记录。
    pub fn from_input(input: RemoteNotificationOutboxInput) -> AxonResult<Self> {
        validate_input(&input)?;
        let failure = input.plan.delivered.iter().find_map(|delivery| {
            if let WireEventKind::RuntimeError(failure) = &delivery.wire_event.kind {
                Some(failure.clone())
            } else {
                None
            }
        });
        Ok(Self {
            schema_version: REMOTE_NOTIFICATION_OUTBOX_SCHEMA_VERSION,
            dispatch_id: input.dispatch_id,
            session_id: input.plan.session_id.0,
            subscriber_id: input.plan.subscriber_id,
            connection_id: input.plan.connection_id,
            pairing_id: input.plan.pairing_id,
            transport: input.plan.transport,
            status: input.status,
            delivery_count: input.plan.delivered.len(),
            dropped_count: input.plan.dropped_count,
            delivery_kinds: input
                .plan
                .delivered
                .iter()
                .map(|delivery| delivery.kind)
                .collect(),
            recorded_at_ms: input.recorded_at_ms,
            updated_at_ms: input.updated_at_ms,
            attempt_count: input.attempt_count,
            next_retry_after_ms: input.next_retry_after_ms,
            last_error: input.last_error,
            failure,
            adapter_message_id: None,
            attempt_started_at_ms: (input.status == RemoteNotificationDispatchStatus::Attempting)
                .then_some(input.updated_at_ms),
            attempt_owner: (input.status == RemoteNotificationDispatchStatus::Attempting)
                .then(|| input.source_trace.clone()),
            recovery_marker: None,
            source_trace: input.source_trace,
            reason: input.reason,
            // 序号由掌握 Session Event sequence 的调用方在构造后就地设置。
            sequence: None,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            raw_body_persisted: false,
            raw_payload_persisted: false,
        })
    }
}

impl RemoteNotificationOutboxRuntime {
    /// 在静默时段将通知 durable 延后；记录仍保留在 outbox，且不改变 LRU 背压策略。
    pub fn append_quiet_hours_record(
        path: impl AsRef<Path>,
        record: &RemoteNotificationOutboxRecord,
        quiet_start_minute: u16,
        quiet_end_minute: u16,
        now_minute: u16,
        retry_after_ms: u64,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        if quiet_start_minute >= MINUTES_PER_DAY
            || quiet_end_minute >= MINUTES_PER_DAY
            || now_minute >= MINUTES_PER_DAY
        {
            return Err(AxonError::runtime_failure(
                "quiet hours minute out of range",
            ));
        }
        let in_quiet = if quiet_start_minute <= quiet_end_minute {
            now_minute >= quiet_start_minute && now_minute < quiet_end_minute
        } else {
            now_minute >= quiet_start_minute || now_minute < quiet_end_minute
        };
        if !in_quiet {
            return Self::append_record(path, record);
        }
        let mut deferred = record.clone();
        deferred.status = RemoteNotificationDispatchStatus::Failed;
        deferred.next_retry_after_ms = Some(retry_after_ms);
        deferred.last_error = Some("notification quiet hours".to_owned());
        deferred.reason = format!("{}:quiet_hours", record.reason)
            .chars()
            .take(REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS)
            .collect();
        Self::append_record(path, &deferred)
    }

    /// 解析默认 outbox 路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_NOTIFICATION_OUTBOX_FILE)
    }

    /// 追加远控通知 outbox 记录。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &RemoteNotificationOutboxRecord,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        validate_record(record)?;
        let evicted_dispatch_ids = crate::RemoteStateStoreRuntime::upsert_with_lru_eviction(
            path,
            &record.dispatch_id,
            outbox_index(record),
            record,
            REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS,
            &[OUTBOX_STATUS_DELIVERED, OUTBOX_STATUS_ABANDONED],
        )?;
        Ok(RemoteNotificationOutboxAppendReport {
            evicted_dispatch_ids,
            coalesced_dispatch_id: None,
        })
    }

    /// 读取所有 outbox 记录。
    pub fn read_records(path: impl AsRef<Path>) -> AxonResult<Vec<RemoteNotificationOutboxRecord>> {
        crate::RemoteStateStoreRuntime::read_all(path)
    }

    /// 按 dispatch id 折叠为最新 outbox 记录。
    pub fn latest_records(
        path: impl AsRef<Path>,
    ) -> AxonResult<Vec<RemoteNotificationOutboxRecord>> {
        Self::read_records(path)
    }

    /// 有界发现指定 transport 的待投递会话。
    pub fn pending_session_ids_for_transport(
        path: impl AsRef<Path>,
        transport: RemoteTransportKind,
        now_ms: u64,
        limit: usize,
    ) -> AxonResult<Vec<SessionId>> {
        if now_ms == 0 || limit == 0 {
            return Err(AxonError::runtime_failure(
                "远控通知 outbox 会话查询参数无效",
            ));
        }
        let records =
            crate::RemoteStateStoreRuntime::read_indexed_limited::<RemoteNotificationOutboxRecord>(
                path,
                OUTBOX_STATE_NAMESPACE,
                None,
                &[OUTBOX_STATUS_PENDING, OUTBOX_STATUS_FAILED],
                limit,
            )?;
        let mut session_ids = std::collections::BTreeSet::new();
        for record in records {
            if record.transport == transport && is_pending(&record, now_ms) {
                session_ids.insert(record.session_id);
            }
        }
        Ok(session_ids.into_iter().map(SessionId).collect())
    }

    /// 查询指定 session/transport 的待消费记录。
    pub fn pending_for_transport(
        path: impl AsRef<Path>,
        session_id: SessionId,
        transport: RemoteTransportKind,
        now_ms: u64,
    ) -> AxonResult<RemoteNotificationOutboxPending> {
        if session_id.0 == 0 || now_ms == 0 {
            return Err(AxonError::runtime_failure(
                "远控通知 outbox pending 查询参数无效",
            ));
        }
        let mut records =
            crate::RemoteStateStoreRuntime::read_indexed::<RemoteNotificationOutboxRecord>(
                path,
                OUTBOX_STATE_NAMESPACE,
                Some(&outbox_partition(session_id.0)),
                &[OUTBOX_STATUS_PENDING, OUTBOX_STATUS_FAILED],
            )?
            .into_iter()
            .filter(|record: &RemoteNotificationOutboxRecord| record.transport == transport)
            .filter(|record| is_pending(record, now_ms))
            .collect::<Vec<_>>();
        // 次序键必须单调：同毫秒内先按来源事件序号，再退回稳定字符串序（哈希值只用于定序兜底）。
        records.sort_by_key(|record| {
            (
                record.updated_at_ms,
                record.sequence.unwrap_or(u64::MAX),
                record.dispatch_id.clone(),
            )
        });
        Ok(RemoteNotificationOutboxPending {
            session_id,
            transport,
            records,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: vec![
                REMOTE_NOTIFICATION_OUTBOX_REASON_PLANNED.to_owned(),
                REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED.to_owned(),
                REMOTE_NOTIFICATION_OUTBOX_REASON_NETWORK_CLOSED.to_owned(),
                REMOTE_NOTIFICATION_OUTBOX_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
            ],
        })
    }

    /// 将发送前检查点写入 durable ledger。
    pub fn mark_attempting(
        path: impl AsRef<Path>,
        dispatch_id: &str,
        started_at_ms: u64,
        owner: &str,
    ) -> AxonResult<RemoteNotificationOutboxRecord> {
        if owner.trim().is_empty() {
            return Err(AxonError::runtime_failure("远控通知 attempting 检查点无效"));
        }
        crate::RemoteStateStoreRuntime::update::<RemoteNotificationOutboxRecord>(
            path,
            dispatch_id,
            outbox_index,
            |current| {
                let mut record =
                    current.ok_or(AxonError::runtime_failure("远控通知 attempting 记录不存在"))?;
                let retry_is_due = record
                    .next_retry_after_ms
                    .is_some_and(|retry_at| retry_at <= started_at_ms);
                if started_at_ms < record.updated_at_ms
                    || !(matches!(record.status, RemoteNotificationDispatchStatus::Pending)
                        || record.status == RemoteNotificationDispatchStatus::Failed
                            && retry_is_due)
                {
                    return Err(AxonError::runtime_failure("远控通知 attempting 检查点无效"));
                }
                record.status = RemoteNotificationDispatchStatus::Attempting;
                record.updated_at_ms = next_revision_at(record.updated_at_ms, started_at_ms)?;
                record.attempt_started_at_ms = Some(started_at_ms);
                record.attempt_owner = Some(owner.to_owned());
                record.next_retry_after_ms = None;
                record.last_error = None;
                record.recovery_marker = None;
                REMOTE_NOTIFICATION_OUTBOX_REASON_ATTEMPTING.clone_into(&mut record.reason);
                Ok(record)
            },
        )
    }

    /// 恢复超时 attempting。恢复记录明确标记可能重复发送。
    pub fn recover_stale_attempts(
        path: impl AsRef<Path>,
        now_ms: u64,
        stale_after_ms: u64,
    ) -> AxonResult<usize> {
        if now_ms == 0 || stale_after_ms == 0 {
            return Err(AxonError::runtime_failure("远控通知恢复参数无效"));
        }
        let mut recovered = 0;
        for record in crate::RemoteStateStoreRuntime::read_indexed::<RemoteNotificationOutboxRecord>(
            &path,
            OUTBOX_STATE_NAMESPACE,
            None,
            &[OUTBOX_STATUS_ATTEMPTING],
        )? {
            let Some(started_at_ms) = record.attempt_started_at_ms else {
                continue;
            };
            if record.status != RemoteNotificationDispatchStatus::Attempting
                || now_ms.saturating_sub(started_at_ms) < stale_after_ms
            {
                continue;
            }
            crate::RemoteStateStoreRuntime::update::<RemoteNotificationOutboxRecord>(
                &path,
                &record.dispatch_id,
                outbox_index,
                |current| {
                    let mut record =
                        current.ok_or(AxonError::runtime_failure("远控通知恢复记录不存在"))?;
                    if record.status != RemoteNotificationDispatchStatus::Attempting
                        || record.attempt_started_at_ms != Some(started_at_ms)
                        || now_ms.saturating_sub(started_at_ms) < stale_after_ms
                    {
                        return Ok(record);
                    }
                    record.status = RemoteNotificationDispatchStatus::Pending;
                    record.updated_at_ms = next_revision_at(record.updated_at_ms, now_ms)?;
                    record.attempt_started_at_ms = None;
                    record.attempt_owner = None;
                    record.recovery_marker = Some(format!(
                        "{}:{}:{}",
                        REMOTE_NOTIFICATION_OUTBOX_REASON_ATTEMPT_RECOVERED,
                        started_at_ms,
                        record.dispatch_id
                    ));
                    REMOTE_NOTIFICATION_OUTBOX_REASON_ATTEMPT_RECOVERED
                        .clone_into(&mut record.reason);
                    recovered += 1;
                    Ok(record)
                },
            )?;
        }
        Ok(recovered)
    }

    /// 清理保留期之前的终态 dispatch。
    pub fn remove_terminal_records_before(
        path: impl AsRef<Path>,
        cutoff_ms: u64,
    ) -> AxonResult<usize> {
        crate::RemoteStateStoreRuntime::delete_indexed_before(
            path,
            OUTBOX_STATE_NAMESPACE,
            &[OUTBOX_STATUS_DELIVERED, OUTBOX_STATUS_ABANDONED],
            cutoff_ms,
        )
    }

    /// 删除指定 dispatch 的当前状态。
    pub fn remove_for_dispatches(
        path: impl AsRef<Path>,
        dispatch_ids: &[String],
    ) -> AxonResult<usize> {
        crate::RemoteStateStoreRuntime::delete_keys(path, dispatch_ids)
    }
}

fn outbox_index(record: &RemoteNotificationOutboxRecord) -> RemoteStateStoreIndex {
    RemoteStateStoreIndex::new(
        OUTBOX_STATE_NAMESPACE,
        outbox_partition(record.session_id),
        outbox_status(record.status),
        record.updated_at_ms,
    )
}

fn outbox_partition(session_id: u64) -> String {
    format!("session:{session_id}")
}

const fn outbox_status(status: RemoteNotificationDispatchStatus) -> &'static str {
    match status {
        RemoteNotificationDispatchStatus::Pending => OUTBOX_STATUS_PENDING,
        RemoteNotificationDispatchStatus::Attempting => OUTBOX_STATUS_ATTEMPTING,
        RemoteNotificationDispatchStatus::Delivered => OUTBOX_STATUS_DELIVERED,
        RemoteNotificationDispatchStatus::Failed => OUTBOX_STATUS_FAILED,
        RemoteNotificationDispatchStatus::Abandoned => OUTBOX_STATUS_ABANDONED,
    }
}

fn next_revision_at(current_at_ms: u64, requested_at_ms: u64) -> AxonResult<u64> {
    current_at_ms
        .checked_add(1)
        .map(|next_at_ms| next_at_ms.max(requested_at_ms))
        .ok_or(AxonError::runtime_failure("远控通知 outbox 时间溢出"))
}

/// 把超长文本压到 `max_chars` 以内：**保留头部与尾部，省略中间**。
///
/// 供应商错误体的诊断价值分布在两端：头部是错误码与摘要（`code` / `msg`），尾部是
/// `log_id` 与排查链接。只截尾会丢掉定位所需的 `log_id`，因此中间用
/// [`CLAMP_OMISSION`] 单行标记替代。
///
/// 该函数同时用于 `error_summary` 等文本字段的防御性收敛：上限是**外部契约**
/// （供应商字段长度），不能假设上游一定遵守，否则一条过长的错误串会让整条投递
/// 被判为「outcome 无效」而彻底无法记录与重试。
#[must_use]
pub fn clamp_text_keeping_edges(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    if total <= max_chars {
        return value.to_owned();
    }
    let omission_chars = CLAMP_OMISSION.chars().count();
    if max_chars <= omission_chars {
        return CLAMP_OMISSION.chars().take(max_chars).collect();
    }
    // 余量按两端尽量均分；头部略多，因为错误摘要通常比 log_id 更长。
    let available = max_chars - omission_chars;
    let head_chars = available.div_ceil(2);
    let tail_chars = available - head_chars;
    let head = value.chars().take(head_chars);
    let tail = value
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev();
    head.chain(CLAMP_OMISSION.chars()).chain(tail).collect()
}

/// 中间省略标记。
pub const CLAMP_OMISSION: &str = " ...[中略]... ";

#[cfg(test)]
#[path = "remote_notification_outbox_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "remote_notification_outbox_timing_tests.rs"]
mod timing_tests;
