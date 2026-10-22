//! 远控通知订阅本机持久化。
//!
//! 该模块只保存订阅恢复所需的有界元数据，不保存通知正文、raw body 或终端原始输出。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use axon_domain::{AxonError, AxonResult, SessionId};
use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

use crate::{
    REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS, REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS,
    REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS, REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS,
    RemoteNotificationKind, RemoteNotificationSubscription, RemoteTransportKind,
};

/// 远控订阅 store 文件名。
pub const REMOTE_SUBSCRIPTION_STORE_FILE: &str = "remote_subscriptions.jsonl";
/// 远控订阅 store schema 版本。
pub const REMOTE_SUBSCRIPTION_STORE_SCHEMA_VERSION: u32 = 1;
/// 远控订阅恢复 source trace 最大字符数。
pub const REMOTE_SUBSCRIPTION_SOURCE_TRACE_MAX_CHARS: usize = 160;
/// 远控订阅恢复原因最大字符数。
pub const REMOTE_SUBSCRIPTION_REASON_MAX_CHARS: usize = 160;
/// 远控订阅持久化原因。
pub const REMOTE_SUBSCRIPTION_REASON_PERSISTED: &str = "remote_subscription:persisted";
/// 远控订阅重连恢复原因。
pub const REMOTE_SUBSCRIPTION_REASON_RECOVERED: &str = "remote_subscription:recovered";
/// 远控订阅撤销原因。
pub const REMOTE_SUBSCRIPTION_REASON_REVOKED: &str = "remote_subscription:revoked";
/// 远控订阅安全边界原因。
pub const REMOTE_SUBSCRIPTION_REASON_NETWORK_CLOSED: &str = "remote_subscription:network_closed";
/// 远控订阅使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// 远控订阅持久化输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSubscriptionStoreInput {
    /// 订阅。
    pub subscription: RemoteNotificationSubscription,
    /// 连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 可选过期时间。
    pub expires_at_ms: Option<u64>,
    /// 是否撤销。
    pub revoked: bool,
    /// source trace。
    pub source_trace: String,
    /// 原因。
    pub reason: String,
}

/// 可持久化远控订阅记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSubscriptionStoreRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// 目标会话 ID。
    pub session_id: u64,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 订阅类别。
    pub kinds: Vec<RemoteNotificationKind>,
    /// 单批最大通知数量。
    pub max_batch: usize,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 可选过期时间。
    pub expires_at_ms: Option<u64>,
    /// 是否撤销。
    pub revoked: bool,
    /// source trace。
    pub source_trace: String,
    /// 原因。
    pub reason: String,
    /// 是否持久化 raw body。
    pub raw_body_persisted: bool,
    /// 是否持久化 raw payload。
    pub raw_payload_persisted: bool,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
}

/// 远控订阅恢复条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSubscriptionRecoveryEntry {
    /// 可恢复订阅。
    pub subscription: RemoteNotificationSubscription,
    /// 原连接 ID。
    pub source_connection_id: String,
    /// 当前重连 connection ID。
    pub recovery_connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 原传输类型。
    pub transport: RemoteTransportKind,
    /// 是否只允许本地恢复。
    pub local_only: bool,
}

/// 远控订阅恢复报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSubscriptionRecoveryReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 当前连接 ID。
    pub recovery_connection_id: String,
    /// 可恢复订阅。
    pub active_subscriptions: Vec<RemoteSubscriptionRecoveryEntry>,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控订阅 store runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteSubscriptionStoreRuntime;

struct RemoteSubscriptionValidationView<'a> {
    session_id: u64,
    subscriber_id: &'a str,
    connection_id: &'a str,
    pairing_id: &'a str,
    kinds: &'a [RemoteNotificationKind],
    max_batch: usize,
    recorded_at_ms: u64,
    updated_at_ms: u64,
    expires_at_ms: Option<u64>,
    source_trace: &'a str,
    reason: &'a str,
}

impl RemoteSubscriptionStoreRecord {
    /// 从输入生成持久化记录。
    pub fn from_input(input: RemoteSubscriptionStoreInput) -> AxonResult<Self> {
        validate_input(&input)?;
        Ok(Self {
            schema_version: REMOTE_SUBSCRIPTION_STORE_SCHEMA_VERSION,
            session_id: input.subscription.session_id.0,
            subscriber_id: input.subscription.subscriber_id,
            connection_id: input.connection_id,
            pairing_id: input.pairing_id,
            transport: input.transport,
            kinds: input.subscription.kinds,
            max_batch: input.subscription.max_batch,
            recorded_at_ms: input.recorded_at_ms,
            updated_at_ms: input.updated_at_ms,
            expires_at_ms: input.expires_at_ms,
            revoked: input.revoked,
            source_trace: input.source_trace,
            reason: input.reason,
            raw_body_persisted: false,
            raw_payload_persisted: false,
            network_send_allowed: false,
        })
    }

    fn to_subscription(&self) -> RemoteNotificationSubscription {
        RemoteNotificationSubscription {
            session_id: SessionId(self.session_id),
            subscriber_id: self.subscriber_id.clone(),
            kinds: self.kinds.clone(),
            max_batch: self.max_batch,
        }
    }
}

impl RemoteSubscriptionStoreRuntime {
    /// 解析默认远控订阅 store 路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_SUBSCRIPTION_STORE_FILE)
    }

    /// 追加远控订阅记录。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &RemoteSubscriptionStoreRecord,
    ) -> AxonResult<()> {
        validate_record(record)?;
        let path = path.as_ref();
        let line = serde_json::to_string(record)
            .map_err(|_| AxonError::runtime_failure("远控订阅记录序列化失败"))?;
        SHARD_STORE
            .append_line(path, line.as_bytes())
            .map_err(|_| AxonError::runtime_failure("无法写入远控订阅记录"))
    }

    /// 读取全部远控订阅记录。
    pub fn read_records(path: impl AsRef<Path>) -> AxonResult<Vec<RemoteSubscriptionStoreRecord>> {
        let path = path.as_ref();
        SHARD_STORE
            .read_lines(path)
            .map_err(|_| AxonError::runtime_failure("无法读取远控订阅记录"))?
            .iter()
            .map(|line| {
                let record = serde_json::from_slice::<RemoteSubscriptionStoreRecord>(line)
                    .map_err(|_| AxonError::runtime_failure("远控订阅 JSONL 无效"))?;
                validate_record(&record)?;
                Ok(record)
            })
            .collect()
    }

    /// 折叠指定 session 的最新活跃订阅。
    pub fn active_for_session(
        path: impl AsRef<Path>,
        session_id: SessionId,
        now_ms: u64,
    ) -> AxonResult<Vec<RemoteSubscriptionStoreRecord>> {
        let mut latest = HashMap::<String, RemoteSubscriptionStoreRecord>::new();
        for record in Self::read_records(path)? {
            if record.session_id != session_id.0 {
                continue;
            }
            latest
                .entry(record.subscriber_id.clone())
                .and_modify(|current| {
                    if is_newer(&record, current) {
                        *current = record.clone();
                    }
                })
                .or_insert(record);
        }
        let mut records = latest
            .into_values()
            .filter(|record| !record.revoked)
            .filter(|record| record.expires_at_ms.is_none_or(|expires| expires > now_ms))
            .collect::<Vec<_>>();
        records.sort_by_key(|record| (record.updated_at_ms, record.subscriber_id.clone()));
        Ok(records)
    }

    /// 为重连连接生成本地订阅恢复报告。
    pub fn recovery_report(
        path: impl AsRef<Path>,
        session_id: SessionId,
        recovery_connection_id: impl Into<String>,
        now_ms: u64,
    ) -> AxonResult<RemoteSubscriptionRecoveryReport> {
        let recovery_connection_id = recovery_connection_id.into();
        validate_bounded(
            &recovery_connection_id,
            REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS,
            "远控订阅恢复 connection id 无效",
        )?;
        let active_subscriptions = Self::active_for_session(path, session_id, now_ms)?
            .into_iter()
            .map(|record| RemoteSubscriptionRecoveryEntry {
                subscription: record.to_subscription(),
                source_connection_id: record.connection_id,
                recovery_connection_id: recovery_connection_id.clone(),
                pairing_id: record.pairing_id,
                transport: record.transport,
                local_only: true,
            })
            .collect();
        Ok(RemoteSubscriptionRecoveryReport {
            session_id,
            recovery_connection_id,
            active_subscriptions,
            network_send_allowed: false,
            reasons: vec![
                REMOTE_SUBSCRIPTION_REASON_RECOVERED.to_owned(),
                REMOTE_SUBSCRIPTION_REASON_NETWORK_CLOSED.to_owned(),
            ],
        })
    }
}

fn validate_input(input: &RemoteSubscriptionStoreInput) -> AxonResult<()> {
    validate_common(&RemoteSubscriptionValidationView {
        session_id: input.subscription.session_id.0,
        subscriber_id: &input.subscription.subscriber_id,
        connection_id: &input.connection_id,
        pairing_id: &input.pairing_id,
        kinds: &input.subscription.kinds,
        max_batch: input.subscription.max_batch,
        recorded_at_ms: input.recorded_at_ms,
        updated_at_ms: input.updated_at_ms,
        expires_at_ms: input.expires_at_ms,
        source_trace: &input.source_trace,
        reason: &input.reason,
    })
}

fn validate_record(record: &RemoteSubscriptionStoreRecord) -> AxonResult<()> {
    if record.schema_version != REMOTE_SUBSCRIPTION_STORE_SCHEMA_VERSION {
        return Err(AxonError::runtime_failure("远控订阅 schema 版本无效"));
    }
    if record.raw_body_persisted || record.raw_payload_persisted || record.network_send_allowed {
        return Err(AxonError::runtime_failure(
            "远控订阅记录不能暴露 raw payload、网络发送",
        ));
    }
    validate_common(&RemoteSubscriptionValidationView {
        session_id: record.session_id,
        subscriber_id: &record.subscriber_id,
        connection_id: &record.connection_id,
        pairing_id: &record.pairing_id,
        kinds: &record.kinds,
        max_batch: record.max_batch,
        recorded_at_ms: record.recorded_at_ms,
        updated_at_ms: record.updated_at_ms,
        expires_at_ms: record.expires_at_ms,
        source_trace: &record.source_trace,
        reason: &record.reason,
    })
}

fn validate_common(view: &RemoteSubscriptionValidationView<'_>) -> AxonResult<()> {
    if view.session_id == 0 || view.recorded_at_ms == 0 || view.updated_at_ms < view.recorded_at_ms
    {
        return Err(AxonError::runtime_failure("远控订阅时间或 session 无效"));
    }
    if view
        .expires_at_ms
        .is_some_and(|expires| expires <= view.updated_at_ms)
    {
        return Err(AxonError::runtime_failure("远控订阅过期时间无效"));
    }
    validate_bounded(
        view.subscriber_id,
        REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS,
        "远控订阅 subscriber id 无效",
    )?;
    validate_bounded(
        view.connection_id,
        REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS,
        "远控订阅 connection id 无效",
    )?;
    validate_bounded(
        view.pairing_id,
        REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS,
        "远控订阅 pairing id 无效",
    )?;
    validate_bounded(
        view.source_trace,
        REMOTE_SUBSCRIPTION_SOURCE_TRACE_MAX_CHARS,
        "远控订阅 source trace 无效",
    )?;
    validate_bounded(
        view.reason,
        REMOTE_SUBSCRIPTION_REASON_MAX_CHARS,
        "远控订阅原因无效",
    )?;
    if view.kinds.is_empty()
        || view.max_batch == 0
        || view.max_batch > REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS
    {
        return Err(AxonError::runtime_failure("远控订阅通知范围无效"));
    }
    Ok(())
}

fn validate_bounded(value: &str, max_chars: usize, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

fn is_newer(
    candidate: &RemoteSubscriptionStoreRecord,
    current: &RemoteSubscriptionStoreRecord,
) -> bool {
    (candidate.updated_at_ms, candidate.recorded_at_ms)
        > (current.updated_at_ms, current.recorded_at_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_subscription_store_recovers_latest_active_subscription() {
        let path = test_store_path("recovers_latest_active_subscription");
        let first = record("client-a", "conn-a", 100, false);
        let second = record("client-a", "conn-b", 200, false);

        RemoteSubscriptionStoreRuntime::append_record(&path, &first).expect("append first");
        RemoteSubscriptionStoreRuntime::append_record(&path, &second).expect("append second");

        let report =
            RemoteSubscriptionStoreRuntime::recovery_report(&path, SessionId(7), "conn-retry", 250)
                .expect("recovery report");

        assert_eq!(report.active_subscriptions.len(), 1);
        assert_eq!(
            report.active_subscriptions[0].source_connection_id,
            "conn-b"
        );
        assert_eq!(
            report.active_subscriptions[0].recovery_connection_id,
            "conn-retry"
        );
        assert!(report.active_subscriptions[0].local_only);
        assert!(!report.network_send_allowed);
        let _ = SHARD_STORE.remove_all(path);
    }

    #[test]
    fn remote_subscription_store_rejects_raw_payload_and_revoked_latest() {
        let path = test_store_path("rejects_raw_payload_and_revoked_latest");
        let active = record("client-a", "conn-a", 100, false);
        let revoked = record("client-a", "conn-a", 200, true);
        let mut unsafe_record = record("client-b", "conn-b", 100, false);
        unsafe_record.raw_payload_persisted = true;

        assert!(RemoteSubscriptionStoreRuntime::append_record(&path, &unsafe_record).is_err());
        RemoteSubscriptionStoreRuntime::append_record(&path, &active).expect("append active");
        RemoteSubscriptionStoreRuntime::append_record(&path, &revoked).expect("append revoked");

        let active_records =
            RemoteSubscriptionStoreRuntime::active_for_session(&path, SessionId(7), 250)
                .expect("active records");
        assert!(active_records.is_empty());
        let _ = SHARD_STORE.remove_all(path);
    }

    fn record(
        subscriber_id: &str,
        connection_id: &str,
        updated_at_ms: u64,
        revoked: bool,
    ) -> RemoteSubscriptionStoreRecord {
        RemoteSubscriptionStoreRecord::from_input(RemoteSubscriptionStoreInput {
            subscription: RemoteNotificationSubscription {
                session_id: SessionId(7),
                subscriber_id: subscriber_id.to_owned(),
                kinds: vec![RemoteNotificationKind::TaskStarted],
                max_batch: 4,
            },
            connection_id: connection_id.to_owned(),
            pairing_id: "pairing-local".to_owned(),
            transport: RemoteTransportKind::WebSocket,
            recorded_at_ms: 100,
            updated_at_ms,
            expires_at_ms: Some(1_000),
            revoked,
            source_trace: "remote_transport:test".to_owned(),
            reason: REMOTE_SUBSCRIPTION_REASON_PERSISTED.to_owned(),
        })
        .expect("record")
    }

    fn test_store_path(name: &str) -> PathBuf {
        axon_test_support::test_temp_root().join(format!(
            "axon_remote_subscription_store_{name}_{}.jsonl",
            std::process::id()
        ))
    }
}
