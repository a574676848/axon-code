//! Feishu per-chat 串行调度。
//!
//! 单次 worker tick 内，同一 chat(connection_id) 只允许一个 dispatch 进入执行。

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};
use axon_remote_runtime::{RemoteStateStoreIndex, RemoteStateStoreRuntime};
use rusqlite::params;

use crate::RemoteNotificationAdapterDispatchItem;

/// Feishu chat 锁状态持久化文件。
pub const REMOTE_FEISHU_SCHEDULER_STATE_FILE: &str = "remote_feishu_scheduler_state.sqlite3";
const REMOTE_FEISHU_SCHEDULER_SCHEMA_VERSION: u32 = 2;
const SCHEDULER_STATE_NAMESPACE: &str = "feishu_scheduler";
const SCHEDULER_STATUS_ACTIVE: &str = "active";
const SCHEDULER_STATUS_INACTIVE: &str = "inactive";
/// 覆盖 token、reply、fallback create 的有界重试网络窗口。
pub(crate) const REMOTE_FEISHU_CHAT_LEASE_DURATION_MS: u64 =
    Duration::from_mins(6).as_millis() as u64;

/// 串行调度结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuSerialSchedule {
    /// 本轮允许执行的 dispatch 项。
    pub active_items: Vec<RemoteNotificationAdapterDispatchItem>,
    /// 同 chat 被延后的 dispatch 项。
    pub deferred_items: Vec<RemoteNotificationAdapterDispatchItem>,
}

/// 单条 chat 锁状态记录。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFeishuSchedulerStateRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// chat_id SHA-256。
    pub chat_id_sha256: String,
    /// 对应 dispatch_id。
    pub dispatch_id: String,
    /// 是否仍为活跃锁。
    pub active: bool,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// lease owner process id。
    pub owner_pid: u32,
    /// owner 进程世代起始时间。
    pub owner_started_at_ms: u64,
    /// lease 到期时间。
    pub lease_expires_at_ms: u64,
}

/// Feishu 串行调度 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuSchedulerRuntime;

/// Feishu chat 锁状态存储 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuSchedulerStoreRuntime;

impl RemoteFeishuSchedulerRuntime {
    /// 基于 connection_id(chat) 规划单轮执行集合。
    pub fn plan(
        items: &[RemoteNotificationAdapterDispatchItem],
    ) -> AxonResult<RemoteFeishuSerialSchedule> {
        Self::plan_with_active_chats(items, &HashSet::new())
    }

    /// 基于已有活跃 chat 锁规划单轮执行集合。
    pub fn plan_with_active_chats(
        items: &[RemoteNotificationAdapterDispatchItem],
        active_chat_hashes: &HashSet<String>,
    ) -> AxonResult<RemoteFeishuSerialSchedule> {
        if items
            .iter()
            .any(|item| item.connection_id.trim().is_empty())
        {
            return Err(AxonError::runtime_failure(
                "Feishu 串行调度要求 connection_id 不能为空",
            ));
        }
        let mut seen = HashSet::new();
        let mut active_items = Vec::new();
        let mut deferred_items = Vec::new();
        for item in items {
            let chat_hash = sha256_hex(&item.connection_id);
            if active_chat_hashes.contains(&chat_hash) {
                deferred_items.push(item.clone());
                continue;
            }
            if seen.insert(item.connection_id.clone()) {
                active_items.push(item.clone());
            } else {
                deferred_items.push(item.clone());
            }
        }
        Ok(RemoteFeishuSerialSchedule {
            active_items,
            deferred_items,
        })
    }
}

impl RemoteFeishuSchedulerStoreRuntime {
    /// 解析 chat 锁状态存储路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_FEISHU_SCHEDULER_STATE_FILE)
    }

    /// 追加 chat 锁状态记录。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &RemoteFeishuSchedulerStateRecord,
    ) -> AxonResult<()> {
        validate_record(record)?;
        if !record.active {
            return RemoteStateStoreRuntime::update::<RemoteFeishuSchedulerStateRecord>(
                path,
                &record.chat_id_sha256,
                scheduler_index,
                |current| match current {
                    Some(current)
                        if current.active
                            && (current.dispatch_id != record.dispatch_id
                                || current.owner_pid != record.owner_pid
                                || current.owner_started_at_ms != record.owner_started_at_ms) =>
                    {
                        Ok(current)
                    }
                    _ => Ok(record.clone()),
                },
            )
            .map(|_| ());
        }
        RemoteStateStoreRuntime::upsert(
            path,
            &record.chat_id_sha256,
            scheduler_index(record),
            record,
        )
    }

    /// 读取全部 chat 锁状态记录。
    pub fn read_records(
        path: impl AsRef<Path>,
    ) -> AxonResult<Vec<RemoteFeishuSchedulerStateRecord>> {
        let records = RemoteStateStoreRuntime::read_all(path)?;
        records.iter().try_for_each(validate_record)?;
        Ok(records)
    }

    /// 在同一 durable 锁内读取 lease、规划并认领可执行项。
    pub fn claim_items(
        path: impl AsRef<Path>,
        items: &[RemoteNotificationAdapterDispatchItem],
        now_ms: u64,
        recorded_at_ms: u64,
    ) -> AxonResult<RemoteFeishuSerialSchedule> {
        if now_ms == 0 || recorded_at_ms < now_ms {
            return Err(AxonError::runtime_failure("Feishu chat 锁认领时间无效"));
        }
        RemoteStateStoreRuntime::with_immediate(path, |transaction| {
            let mut statement = transaction
                .prepare(
                    "SELECT state.payload FROM remote_state_index AS state_index \
                     INNER JOIN remote_state AS state ON state.record_key=state_index.record_key \
                     WHERE state_index.namespace=?1 AND state_index.status=?2",
                )
                .map_err(|_| AxonError::runtime_failure("Feishu chat 锁查询失败"))?;
            let rows = statement
                .query_map(
                    params![SCHEDULER_STATE_NAMESPACE, SCHEDULER_STATUS_ACTIVE],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| AxonError::runtime_failure("Feishu chat 锁查询失败"))?;
            let records = rows
                .map(|row| {
                    let payload =
                        row.map_err(|_| AxonError::runtime_failure("Feishu chat 锁读取失败"))?;
                    let record = serde_json::from_str::<RemoteFeishuSchedulerStateRecord>(&payload)
                        .map_err(|_| AxonError::runtime_failure("Feishu chat 锁记录无效"))?;
                    validate_record(&record)?;
                    Ok(record)
                })
                .collect::<AxonResult<Vec<_>>>()?;
            let active_chat_hashes = Self::active_chat_hashes_at(&records, now_ms);
            let schedule =
                RemoteFeishuSchedulerRuntime::plan_with_active_chats(items, &active_chat_hashes)?;
            for record in build_acquire_records(&schedule.active_items, recorded_at_ms)? {
                let payload = serde_json::to_string(&record)
                    .map_err(|_| AxonError::runtime_failure("Feishu chat 锁序列化失败"))?;
                transaction.execute(
                    "INSERT INTO remote_state(record_key, revision, payload) VALUES (?1, ?2, ?3)
                     ON CONFLICT(record_key) DO UPDATE SET revision=excluded.revision, payload=excluded.payload
                     WHERE excluded.revision >= remote_state.revision",
                    params![record.chat_id_sha256, i64::try_from(record.recorded_at_ms).map_err(|_| AxonError::runtime_failure("Feishu chat 锁版本超限"))?, payload],
                ).map_err(|_| AxonError::runtime_failure("Feishu chat 锁写入失败"))?;
                transaction.execute(
                    "INSERT INTO remote_state_index(record_key, namespace, partition, status, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(record_key) DO UPDATE SET namespace=excluded.namespace, partition=excluded.partition, status=excluded.status, updated_at_ms=excluded.updated_at_ms",
                    params![record.chat_id_sha256, SCHEDULER_STATE_NAMESPACE, record.chat_id_sha256, SCHEDULER_STATUS_ACTIVE, i64::try_from(record.recorded_at_ms).map_err(|_| AxonError::runtime_failure("Feishu chat 锁版本超限"))?],
                ).map_err(|_| AxonError::runtime_failure("Feishu chat 锁索引写入失败"))?;
            }
            Ok(schedule)
        })
    }

    /// 返回当前活跃 chat 锁集合。
    #[must_use]
    pub fn active_chat_hashes(records: &[RemoteFeishuSchedulerStateRecord]) -> HashSet<String> {
        Self::active_chat_hashes_at(records, 0)
    }

    /// 返回未过期活跃 chat 锁集合，并丢弃已过期 lease。
    #[must_use]
    pub fn active_chat_hashes_at(
        records: &[RemoteFeishuSchedulerStateRecord],
        now_ms: u64,
    ) -> HashSet<String> {
        active_leases_at(records, now_ms).into_keys().collect()
    }

    /// 为已完成 dispatch 生成 release 记录。
    pub fn release_record(
        item: &RemoteNotificationAdapterDispatchItem,
        recorded_at_ms: u64,
    ) -> AxonResult<RemoteFeishuSchedulerStateRecord> {
        if item.connection_id.trim().is_empty() || item.dispatch_id.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu chat 锁 release 缺少必要字段",
            ));
        }
        Ok(RemoteFeishuSchedulerStateRecord {
            schema_version: REMOTE_FEISHU_SCHEDULER_SCHEMA_VERSION,
            chat_id_sha256: sha256_hex(&item.connection_id),
            dispatch_id: item.dispatch_id.clone(),
            active: false,
            recorded_at_ms,
            owner_pid: std::process::id(),
            owner_started_at_ms: process_started_at_ms(),
            lease_expires_at_ms: recorded_at_ms,
        })
    }

    /// 清理保留期之前已释放的 chat lease。
    pub fn remove_inactive_records_before(
        path: impl AsRef<Path>,
        cutoff_ms: u64,
    ) -> AxonResult<usize> {
        RemoteStateStoreRuntime::delete_indexed_before(
            path,
            SCHEDULER_STATE_NAMESPACE,
            &[SCHEDULER_STATUS_INACTIVE],
            cutoff_ms,
        )
    }
}

fn scheduler_index(record: &RemoteFeishuSchedulerStateRecord) -> RemoteStateStoreIndex {
    RemoteStateStoreIndex::new(
        SCHEDULER_STATE_NAMESPACE,
        record.chat_id_sha256.clone(),
        if record.active {
            SCHEDULER_STATUS_ACTIVE
        } else {
            SCHEDULER_STATUS_INACTIVE
        },
        record.recorded_at_ms,
    )
}

fn build_acquire_records(
    items: &[RemoteNotificationAdapterDispatchItem],
    recorded_at_ms: u64,
) -> AxonResult<Vec<RemoteFeishuSchedulerStateRecord>> {
    items
        .iter()
        .map(|item| {
            if item.connection_id.trim().is_empty() || item.dispatch_id.trim().is_empty() {
                return Err(AxonError::runtime_failure(
                    "Feishu chat 锁 acquire 缺少必要字段",
                ));
            }
            Ok(RemoteFeishuSchedulerStateRecord {
                schema_version: REMOTE_FEISHU_SCHEDULER_SCHEMA_VERSION,
                chat_id_sha256: sha256_hex(&item.connection_id),
                dispatch_id: item.dispatch_id.clone(),
                active: true,
                recorded_at_ms,
                owner_pid: std::process::id(),
                owner_started_at_ms: process_started_at_ms(),
                lease_expires_at_ms: recorded_at_ms
                    .saturating_add(REMOTE_FEISHU_CHAT_LEASE_DURATION_MS),
            })
        })
        .collect()
}

fn active_leases_at(
    records: &[RemoteFeishuSchedulerStateRecord],
    now_ms: u64,
) -> HashMap<String, RemoteFeishuSchedulerStateRecord> {
    let mut active = HashMap::new();
    for record in records {
        if record.active && record.lease_expires_at_ms > now_ms {
            active.insert(record.chat_id_sha256.clone(), record.clone());
        } else if active.get(&record.chat_id_sha256).is_some_and(|lease| {
            lease.dispatch_id == record.dispatch_id
                && lease.owner_pid == record.owner_pid
                && lease.owner_started_at_ms == record.owner_started_at_ms
        }) {
            active.remove(&record.chat_id_sha256);
        }
    }
    active
}

fn validate_record(record: &RemoteFeishuSchedulerStateRecord) -> AxonResult<()> {
    if record.schema_version != REMOTE_FEISHU_SCHEDULER_SCHEMA_VERSION
        || record.chat_id_sha256.trim().is_empty()
        || record.dispatch_id.trim().is_empty()
        || record.recorded_at_ms == 0
        || record.owner_pid == 0
        || record.owner_started_at_ms == 0
        || (record.active && record.lease_expires_at_ms <= record.recorded_at_ms)
    {
        return Err(AxonError::runtime_failure("Feishu chat 锁记录字段无效"));
    }
    Ok(())
}

fn process_started_at_ms() -> u64 {
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};
    static STARTED_AT_MS: OnceLock<u64> = OnceLock::new();
    *STARTED_AT_MS.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |duration| {
                duration.as_millis().try_into().unwrap_or(u64::MAX)
            })
    })
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use super::*;
    use crate::RemoteNotificationKind;

    fn item(dispatch_id: &str, chat: &str) -> RemoteNotificationAdapterDispatchItem {
        RemoteNotificationAdapterDispatchItem {
            dispatch_id: dispatch_id.to_owned(),
            subscriber_id: "sub".to_owned(),
            connection_id: chat.to_owned(),
            pairing_id: "pairing".to_owned(),
            delivery_count: 1,
            delivery_kinds: vec![RemoteNotificationKind::TaskStarted],
            attempt_count: 0,
        }
    }

    #[test]
    fn scheduler_defers_second_dispatch_in_same_chat() {
        let plan = RemoteFeishuSchedulerRuntime::plan(&[
            item("a", "chat-1"),
            item("b", "chat-1"),
            item("c", "chat-2"),
        ])
        .expect("plan");

        assert_eq!(plan.active_items.len(), 2);
        assert_eq!(plan.deferred_items.len(), 1);
        assert_eq!(plan.deferred_items[0].dispatch_id, "b");
    }

    #[test]
    fn scheduler_defers_chat_with_active_lease() {
        let active = HashSet::from([sha256_hex("chat-1")]);
        let plan = RemoteFeishuSchedulerRuntime::plan_with_active_chats(
            &[item("a", "chat-1"), item("b", "chat-2")],
            &active,
        )
        .expect("plan");

        assert_eq!(plan.active_items.len(), 1);
        assert_eq!(plan.active_items[0].dispatch_id, "b");
        assert_eq!(plan.deferred_items.len(), 1);
        assert_eq!(plan.deferred_items[0].dispatch_id, "a");
    }

    #[test]
    fn concurrent_claims_allow_only_one_worker_per_chat() {
        let directory = axon_test_support::test_temp_dir();
        let path = RemoteFeishuSchedulerStoreRuntime::resolve_store_path(directory.path());
        // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
        let _close_pool = axon_test_support::DeferredCleanup::new({
            let path = path.clone();
            move || axon_remote_runtime::RemoteStateStoreRuntime::close_path(&path)
        });
        let barrier = Arc::new(Barrier::new(2));
        let first_path = path.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            first_barrier.wait();
            RemoteFeishuSchedulerStoreRuntime::claim_items(
                first_path,
                &[item("first", "chat-1")],
                100,
                200,
            )
            .expect("first claim")
        });
        barrier.wait();
        let second = RemoteFeishuSchedulerStoreRuntime::claim_items(
            &path,
            &[item("second", "chat-1")],
            100,
            200,
        )
        .expect("second claim");
        let first = first.join().expect("first join");

        assert_eq!(first.active_items.len() + second.active_items.len(), 1);
        assert_eq!(first.deferred_items.len() + second.deferred_items.len(), 1);
    }

    #[test]
    fn late_release_cannot_remove_a_newer_lease_for_the_same_chat() {
        let directory = axon_test_support::test_temp_dir();
        let path = RemoteFeishuSchedulerStoreRuntime::resolve_store_path(directory.path());
        // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
        let _close_pool = axon_test_support::DeferredCleanup::new({
            let path = path.clone();
            move || axon_remote_runtime::RemoteStateStoreRuntime::close_path(&path)
        });
        let first = item("first", "chat-1");
        let second = item("second", "chat-1");
        RemoteFeishuSchedulerStoreRuntime::claim_items(
            &path,
            std::slice::from_ref(&first),
            100,
            200,
        )
        .expect("first claim");
        let after_expiry = 200 + REMOTE_FEISHU_CHAT_LEASE_DURATION_MS + 1;
        RemoteFeishuSchedulerStoreRuntime::claim_items(
            &path,
            &[second],
            after_expiry,
            after_expiry,
        )
        .expect("second claim");
        let late_release = RemoteFeishuSchedulerStoreRuntime::release_record(
            &first,
            after_expiry.saturating_add(1),
        )
        .expect("late release");
        RemoteFeishuSchedulerStoreRuntime::append_record(&path, &late_release)
            .expect("append late release");

        let records = RemoteFeishuSchedulerStoreRuntime::read_records(path).expect("read records");
        assert!(
            RemoteFeishuSchedulerStoreRuntime::active_chat_hashes_at(
                &records,
                after_expiry.saturating_add(1),
            )
            .contains(&sha256_hex("chat-1"))
        );
    }
}
