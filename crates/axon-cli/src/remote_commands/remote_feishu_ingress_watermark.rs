use std::path::{Path, PathBuf};

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};
use axon_remote_runtime::{RemoteStateStoreIndex, RemoteStateStoreRuntime};
use serde::{Deserialize, Serialize};

const STORE_FILE_NAME: &str = "remote_feishu_ingress_watermarks.sqlite3";
const STORE_NAMESPACE: &str = "remote_feishu_ingress_watermark";
const STORE_PARTITION: &str = "feishu";
const STORE_STATUS_ACTIVE: &str = "active";
const RECORD_KEY_PREFIX: &str = "chat:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuIngressWatermark {
    pub(super) request_id: String,
    pub(super) source_created_at_ms: u64,
    recorded_at_ms: u64,
}

impl RemoteFeishuIngressWatermark {
    pub(super) fn rejects(&self, source_created_at_ms: u64) -> bool {
        source_created_at_ms < self.source_created_at_ms
    }
}

pub(super) fn should_reject(
    root: &Path,
    chat_id: &str,
    _request_id: &str,
    source_created_at_ms: Option<u64>,
) -> Result<bool, String> {
    let Some(source_created_at_ms) = source_created_at_ms.filter(|value| *value > 0) else {
        return Ok(true);
    };
    Ok(current(root, chat_id)?.is_some_and(|watermark| watermark.rejects(source_created_at_ms)))
}

pub(super) fn advance(
    root: &Path,
    chat_id: &str,
    request_id: &str,
    source_created_at_ms: u64,
    recorded_at_ms: u64,
) -> Result<RemoteFeishuIngressWatermark, String> {
    validate_record_input(chat_id, request_id, source_created_at_ms, recorded_at_ms)
        .map_err(|error| format!("{error:?}"))?;
    let candidate = RemoteFeishuIngressWatermark {
        request_id: request_id.to_owned(),
        source_created_at_ms,
        recorded_at_ms,
    };
    RemoteStateStoreRuntime::update(
        store_path(root),
        &record_key(chat_id),
        |watermark: &RemoteFeishuIngressWatermark| {
            RemoteStateStoreIndex::new(
                STORE_NAMESPACE,
                STORE_PARTITION,
                STORE_STATUS_ACTIVE,
                watermark.recorded_at_ms,
            )
        },
        |current| match current {
            Some(current) if !should_replace(&current, &candidate) => Ok(current),
            _ => Ok(candidate),
        },
    )
    .map_err(|error| format!("写入飞书入站水位失败：{error:?}"))
}

fn should_replace(
    current: &RemoteFeishuIngressWatermark,
    candidate: &RemoteFeishuIngressWatermark,
) -> bool {
    candidate.source_created_at_ms > current.source_created_at_ms
}

pub(super) fn current(
    root: &Path,
    chat_id: &str,
) -> Result<Option<RemoteFeishuIngressWatermark>, String> {
    validate_chat_id(chat_id).map_err(|error| format!("{error:?}"))?;
    RemoteStateStoreRuntime::read(store_path(root), &record_key(chat_id))
        .map_err(|error| format!("读取飞书入站水位失败：{error:?}"))
}

fn store_path(root: &Path) -> PathBuf {
    root.join(STORE_FILE_NAME)
}

fn record_key(chat_id: &str) -> String {
    format!("{RECORD_KEY_PREFIX}{}", sha256_hex(chat_id))
}

fn validate_chat_id(chat_id: &str) -> AxonResult<()> {
    if chat_id.trim().is_empty() {
        return Err(AxonError::runtime_failure("飞书入站水位缺少 chat_id"));
    }
    Ok(())
}

fn validate_record_input(
    chat_id: &str,
    request_id: &str,
    source_created_at_ms: u64,
    recorded_at_ms: u64,
) -> AxonResult<()> {
    validate_chat_id(chat_id)?;
    if request_id.trim().is_empty() || source_created_at_ms == 0 || recorded_at_ms == 0 {
        return Err(AxonError::runtime_failure("飞书入站水位记录无效"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_frontier_rejects_only_earlier_messages() {
        let watermark = watermark("req-current", 200);

        assert!(watermark.rejects(199));
        assert!(!watermark.rejects(200));
        assert!(!watermark.rejects(201));
    }

    #[test]
    fn older_candidate_cannot_rewind_persisted_frontier() {
        let root = axon_test_support::test_temp_dir();
        // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
        let _close_pool = ingress_watermark_pool_guard(root.path());
        advance(root.path(), "oc_chat", "req-new", 200, 300).expect("新水位应写入");
        let stored =
            advance(root.path(), "oc_chat", "req-old", 100, 400).expect("旧候选水位应保持当前值");

        assert_eq!(stored.request_id, "req-new");
        assert_eq!(stored.source_created_at_ms, 200);
    }

    #[test]
    fn frontiers_are_isolated_by_chat() {
        let root = axon_test_support::test_temp_dir();
        // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
        let _close_pool = ingress_watermark_pool_guard(root.path());
        advance(root.path(), "oc_first", "req-first", 200, 300).expect("首个 chat 水位应写入");

        assert!(
            should_reject(root.path(), "oc_first", "req-old", Some(199))
                .expect("首个 chat 应可判断")
        );
        assert!(
            !should_reject(root.path(), "oc_second", "req-old", Some(199))
                .expect("第二个 chat 应可判断")
        );
    }

    #[test]
    fn missing_or_zero_source_timestamp_is_rejected() {
        let root = axon_test_support::test_temp_dir();
        // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
        let _close_pool = ingress_watermark_pool_guard(root.path());
        assert!(should_reject(root.path(), "oc_chat", "req", None).expect("缺失时间应拒绝"));
        assert!(should_reject(root.path(), "oc_chat", "req", Some(0)).expect("零时间应拒绝"));
    }

    /// 入站水位库走进程内连接池：临时目录析构前必须先关闭连接池，否则库文件会一直锁住目录。
    fn ingress_watermark_pool_guard(root: &Path) -> axon_test_support::DeferredCleanup {
        let watermark = root.join(STORE_FILE_NAME);
        axon_test_support::DeferredCleanup::new(move || {
            axon_remote_runtime::RemoteStateStoreRuntime::close_path(&watermark)
        })
    }

    fn watermark(request_id: &str, source_created_at_ms: u64) -> RemoteFeishuIngressWatermark {
        RemoteFeishuIngressWatermark {
            request_id: request_id.to_owned(),
            source_created_at_ms,
            recorded_at_ms: 300,
        }
    }
}
