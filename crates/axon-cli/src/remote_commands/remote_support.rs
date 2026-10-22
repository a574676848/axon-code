use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::stable_ids::stable_numeric_id;
use axon_config::provider_credential;
use axon_crypto::sha256_hex;
pub(super) use axon_display::display_path;
use axon_remote_feishu::remote_feishu_runtime_root;
use axon_remote_runtime::{
    REMOTE_SUBSCRIPTION_REASON_REVOKED, RemoteNotificationDispatchStatus, RemoteNotificationKind,
    RemoteNotificationOutboxRecord, RemoteNotificationOutboxRuntime, RemoteSubscriptionStoreRecord,
    RemoteSubscriptionStoreRuntime, RemoteTransportKind,
};
use axon_time::unix_now_ms;
use serde::Serialize;

pub(super) const REMOTE_CLI_DEFAULT_LIMIT: usize = 20;
pub(super) const REMOTE_CLI_MAX_LIMIT: usize = 100;
pub(super) const REMOTE_CLI_REVOKE_SOURCE_TRACE: &str = "remote_cli:revoke";
pub(super) const REMOTE_CLI_REVOKE_REASON_HASH_PREFIX: &str = "remote_subscription:revoked:sha256:";
const REMOTE_RUNTIME_DIR: &str = "remote";
const RUNTIME_DATA_DIR: &str = "data";

#[derive(Clone)]
pub(crate) struct RemoteCliContext {
    pub(super) config_path: PathBuf,
    pub(super) axon_root: PathBuf,
    pub(super) root: PathBuf,
    pub(super) subscription_store_path: PathBuf,
    pub(super) outbox_store_path: PathBuf,
    pub(super) feishu_websocket: axon_config::RemoteFeishuWebSocketConfig,
}

impl RemoteCliContext {
    pub(super) fn new(
        config_path: PathBuf,
        axon_root: PathBuf,
        feishu_websocket: axon_config::RemoteFeishuWebSocketConfig,
    ) -> Self {
        let root = remote_runtime_root(&axon_root);
        Self::from_roots(config_path, axon_root, root, feishu_websocket)
    }

    pub(crate) fn feishu_runtime(&self) -> Self {
        Self::from_roots(
            self.config_path.clone(),
            self.axon_root.clone(),
            remote_feishu_runtime_root(&self.axon_root),
            self.feishu_websocket.clone(),
        )
    }

    pub(crate) fn gateway_session_id(&self) -> u64 {
        feishu_gateway_session_id(&self.feishu_websocket)
    }

    pub(super) fn feishu_app_secret(&self) -> Option<String> {
        resolve_remote_credential(&self.feishu_websocket.app_secret_env)
    }

    fn from_roots(
        config_path: PathBuf,
        axon_root: PathBuf,
        root: PathBuf,
        feishu_websocket: axon_config::RemoteFeishuWebSocketConfig,
    ) -> Self {
        let subscription_store_path = RemoteSubscriptionStoreRuntime::resolve_store_path(&root);
        let outbox_store_path = RemoteNotificationOutboxRuntime::resolve_store_path(&root);
        Self {
            config_path,
            axon_root,
            root,
            subscription_store_path,
            outbox_store_path,
            feishu_websocket,
        }
    }
}

pub(super) fn resolve_remote_credential(name: &str) -> Option<String> {
    provider_credential(name).filter(|value| !value.trim().is_empty())
}

pub(super) fn feishu_gateway_session_id(config: &axon_config::RemoteFeishuWebSocketConfig) -> u64 {
    stable_numeric_id(&format!("gateway:feishu:{}", config.app_id))
}

fn remote_runtime_root(axon_root: &Path) -> PathBuf {
    axon_root.join(RUNTIME_DATA_DIR).join(REMOTE_RUNTIME_DIR)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteStatusReport {
    pub(super) status: &'static str,
    pub(super) axon_root: String,
    pub(super) remote_runtime_dir: String,
    pub(super) subscription_store_path: String,
    pub(super) outbox_store_path: String,
    pub(super) total_subscription_records: usize,
    pub(super) active_subscription_records: usize,
    pub(super) total_outbox_records: usize,
    pub(super) pending_outbox_records: usize,
    pub(super) network_send_allowed: bool,
    pub(super) raw_payload_forward_allowed: bool,
    pub(super) raw_body_persisted: bool,
    pub(super) raw_payload_persisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteSubscriptionsReport {
    pub(super) status: &'static str,
    pub(super) store_path: String,
    pub(super) limit: usize,
    pub(super) total_records: usize,
    pub(super) active_records: usize,
    pub(super) subscriptions: Vec<RemoteSubscriptionView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteOutboxReport {
    pub(super) status: &'static str,
    pub(super) store_path: String,
    pub(super) limit: usize,
    pub(super) total_records: usize,
    pub(super) pending_records: usize,
    pub(super) dispatches: Vec<RemoteOutboxView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteRevokeReport {
    pub(super) status: &'static str,
    pub(super) store_path: String,
    pub(super) subscription: RemoteSubscriptionView,
    pub(super) revoked: bool,
    pub(super) network_send_allowed: bool,
    pub(super) raw_body_persisted: bool,
    pub(super) raw_payload_persisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteAuditReport {
    pub(super) status: &'static str,
    pub(super) subscription_store_path: String,
    pub(super) outbox_store_path: String,
    pub(super) limit: usize,
    pub(super) total_subscription_records: usize,
    pub(super) latest_subscription_records: usize,
    pub(super) active_subscription_records: usize,
    pub(super) revoked_subscription_records: usize,
    pub(super) total_outbox_records: usize,
    pub(super) pending_outbox_records: usize,
    pub(super) subscriptions: Vec<RemoteSubscriptionView>,
    pub(super) dispatches: Vec<RemoteOutboxView>,
    pub(super) network_send_allowed: bool,
    pub(super) raw_payload_forward_allowed: bool,
    pub(super) raw_body_persisted: bool,
    pub(super) raw_payload_persisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteSubscriptionView {
    pub(super) session_id: u64,
    pub(super) subscriber_id_sha256: String,
    pub(super) connection_id_sha256: String,
    pub(super) pairing_id_sha256: String,
    pub(super) transport: &'static str,
    pub(super) kinds: Vec<&'static str>,
    pub(super) max_batch: usize,
    pub(super) updated_at_ms: u64,
    pub(super) expires_at_ms: Option<u64>,
    pub(super) revoked: bool,
    pub(super) has_source_trace_ref: bool,
    pub(super) has_reason_ref: bool,
    pub(super) network_send_allowed: bool,
    pub(super) raw_body_persisted: bool,
    pub(super) raw_payload_persisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteOutboxView {
    pub(super) dispatch_id: String,
    pub(super) session_id: u64,
    pub(super) subscriber_id_sha256: String,
    pub(super) connection_id_sha256: String,
    pub(super) pairing_id_sha256: String,
    pub(super) transport: &'static str,
    pub(super) status: &'static str,
    pub(super) delivery_count: usize,
    pub(super) dropped_count: usize,
    pub(super) delivery_kinds: Vec<&'static str>,
    pub(super) updated_at_ms: u64,
    pub(super) attempt_count: u32,
    pub(super) next_retry_after_ms: Option<u64>,
    pub(super) has_last_error_ref: bool,
    pub(super) has_adapter_message_ref: bool,
    pub(super) has_source_trace_ref: bool,
    pub(super) has_reason_ref: bool,
    pub(super) local_only: bool,
    pub(super) network_send_allowed: bool,
    pub(super) raw_payload_forward_allowed: bool,
    pub(super) raw_body_persisted: bool,
    pub(super) raw_payload_persisted: bool,
}

pub(super) fn read_subscriptions(
    context: &RemoteCliContext,
) -> Result<Vec<RemoteSubscriptionStoreRecord>, String> {
    RemoteSubscriptionStoreRuntime::read_records(&context.subscription_store_path)
        .map_err(|error| format!("{error:?}"))
}

pub(super) fn read_outbox(
    context: &RemoteCliContext,
) -> Result<Vec<RemoteNotificationOutboxRecord>, String> {
    RemoteNotificationOutboxRuntime::latest_records(&context.outbox_store_path)
        .map_err(|error| format!("{error:?}"))
}

pub(super) fn active_subscriptions(
    records: &[RemoteSubscriptionStoreRecord],
    now_ms: u64,
) -> Vec<RemoteSubscriptionStoreRecord> {
    let mut active = latest_subscriptions(records)
        .into_iter()
        .filter(|record| !record.revoked)
        .filter(|record| record.expires_at_ms.is_none_or(|expires| expires > now_ms))
        .collect::<Vec<_>>();
    active.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
    active
}

pub(super) fn latest_subscriptions(
    records: &[RemoteSubscriptionStoreRecord],
) -> Vec<RemoteSubscriptionStoreRecord> {
    let mut latest = BTreeMap::<(u64, String), RemoteSubscriptionStoreRecord>::new();
    for record in records {
        let key = (record.session_id, record.subscriber_id.clone());
        latest
            .entry(key)
            .and_modify(|current| {
                if (record.updated_at_ms, record.recorded_at_ms)
                    > (current.updated_at_ms, current.recorded_at_ms)
                {
                    *current = record.clone();
                }
            })
            .or_insert_with(|| record.clone());
    }
    let mut records = latest.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
    records
}

pub(super) fn latest_subscription_for(
    records: &[RemoteSubscriptionStoreRecord],
    session_id: u64,
    subscriber_id: &str,
) -> Option<RemoteSubscriptionStoreRecord> {
    latest_subscriptions(records)
        .into_iter()
        .find(|record| record.session_id == session_id && record.subscriber_id == subscriber_id)
}

pub(super) fn subscription_view(record: &RemoteSubscriptionStoreRecord) -> RemoteSubscriptionView {
    RemoteSubscriptionView {
        session_id: record.session_id,
        subscriber_id_sha256: sha256_hex(&record.subscriber_id),
        connection_id_sha256: sha256_hex(&record.connection_id),
        pairing_id_sha256: sha256_hex(&record.pairing_id),
        transport: transport_code(record.transport),
        kinds: record
            .kinds
            .iter()
            .copied()
            .map(notification_kind_code)
            .collect(),
        max_batch: record.max_batch,
        updated_at_ms: record.updated_at_ms,
        expires_at_ms: record.expires_at_ms,
        revoked: record.revoked,
        has_source_trace_ref: !record.source_trace.trim().is_empty(),
        has_reason_ref: !record.reason.trim().is_empty(),
        network_send_allowed: false,
        raw_body_persisted: false,
        raw_payload_persisted: false,
    }
}

pub(super) fn outbox_view(record: &RemoteNotificationOutboxRecord) -> RemoteOutboxView {
    RemoteOutboxView {
        dispatch_id: record.dispatch_id.clone(),
        session_id: record.session_id,
        subscriber_id_sha256: sha256_hex(&record.subscriber_id),
        connection_id_sha256: sha256_hex(&record.connection_id),
        pairing_id_sha256: sha256_hex(&record.pairing_id),
        transport: transport_code(record.transport),
        status: dispatch_status_code(record.status),
        delivery_count: record.delivery_count,
        dropped_count: record.dropped_count,
        delivery_kinds: record
            .delivery_kinds
            .iter()
            .copied()
            .map(notification_kind_code)
            .collect(),
        updated_at_ms: record.updated_at_ms,
        attempt_count: record.attempt_count,
        next_retry_after_ms: record.next_retry_after_ms,
        has_last_error_ref: record
            .last_error
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty()),
        has_adapter_message_ref: record
            .adapter_message_id
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty()),
        has_source_trace_ref: !record.source_trace.trim().is_empty(),
        has_reason_ref: !record.reason.trim().is_empty(),
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        raw_body_persisted: false,
        raw_payload_persisted: false,
    }
}

pub(super) fn outbox_pending(record: &RemoteNotificationOutboxRecord, now_ms: u64) -> bool {
    match record.status {
        RemoteNotificationDispatchStatus::Pending => true,
        RemoteNotificationDispatchStatus::Failed => record
            .next_retry_after_ms
            .is_some_and(|retry_at| retry_at <= now_ms),
        RemoteNotificationDispatchStatus::Attempting
        | RemoteNotificationDispatchStatus::Delivered
        | RemoteNotificationDispatchStatus::Abandoned => false,
    }
}

pub(super) fn bounded_limit(limit: Option<usize>) -> Result<usize, &'static str> {
    let limit = limit.unwrap_or(REMOTE_CLI_DEFAULT_LIMIT);
    if limit == 0 || limit > REMOTE_CLI_MAX_LIMIT {
        return Err("remote limit 必须在 1..=100 范围内");
    }
    Ok(limit)
}

pub(super) fn now_ms() -> u64 {
    unix_now_ms()
}

pub(super) fn revoke_reason_ref(reason: Option<&str>) -> String {
    let Some(reason) = reason.map(str::trim).filter(|value| !value.is_empty()) else {
        return REMOTE_SUBSCRIPTION_REASON_REVOKED.to_owned();
    };
    format!(
        "{REMOTE_CLI_REVOKE_REASON_HASH_PREFIX}{}",
        sha256_hex(reason)
    )
}

pub(super) const fn transport_code(transport: RemoteTransportKind) -> &'static str {
    match transport {
        RemoteTransportKind::Http => "http",
        RemoteTransportKind::WebSocket => "webSocket",
        RemoteTransportKind::Feishu => "feishu",
        RemoteTransportKind::Mobile => "mobile",
        RemoteTransportKind::Web => "web",
    }
}

pub(super) const fn notification_kind_code(kind: RemoteNotificationKind) -> &'static str {
    match kind {
        RemoteNotificationKind::TaskStarted => "taskStarted",
        RemoteNotificationKind::TestCompleted => "testCompleted",
        RemoteNotificationKind::TaskFailed => "taskFailed",
        RemoteNotificationKind::InputRequired => "inputRequired",
        RemoteNotificationKind::ThreadCompleted => "threadCompleted",
        RemoteNotificationKind::SafetyBlocked => "safetyBlocked",
    }
}

pub(super) const fn dispatch_status_code(status: RemoteNotificationDispatchStatus) -> &'static str {
    match status {
        RemoteNotificationDispatchStatus::Pending => "pending",
        RemoteNotificationDispatchStatus::Attempting => "attempting",
        RemoteNotificationDispatchStatus::Delivered => "delivered",
        RemoteNotificationDispatchStatus::Failed => "failed",
        RemoteNotificationDispatchStatus::Abandoned => "abandoned",
    }
}
