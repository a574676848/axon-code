//! Feishu processing reaction 当前状态存储。

use std::path::{Path, PathBuf};

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};
use axon_remote_runtime::{RemoteStateStoreIndex, RemoteStateStoreRuntime};

use super::REMOTE_FEISHU_PROCESSING_STATE_FILE;

/// Processing reaction 状态记录。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFeishuProcessingStateRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// message_id 的 SHA-256。
    pub message_id_sha256: String,
    /// reaction_id。
    pub reaction_id: String,
    /// reaction 类型。
    pub reaction_type: String,
    /// 是否仍然处于活跃态。
    pub active: bool,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// Processing 状态存储 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuProcessingStateStoreRuntime;

impl RemoteFeishuProcessingStateStoreRuntime {
    /// 解析 processing state 存储路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_FEISHU_PROCESSING_STATE_FILE)
    }

    /// 写入一条消息的当前 processing reaction 状态。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &RemoteFeishuProcessingStateRecord,
    ) -> AxonResult<()> {
        validate_record(record)?;
        RemoteStateStoreRuntime::upsert(
            path,
            &record.message_id_sha256,
            state_index(record),
            record,
        )
    }

    /// 按消息读取当前活跃 reaction。
    pub fn active_reaction(
        path: impl AsRef<Path>,
        message_id: &str,
    ) -> AxonResult<Option<RemoteFeishuProcessingStateRecord>> {
        let record = RemoteStateStoreRuntime::read(path, &sha256_hex(message_id))?;
        Ok(record.filter(|record: &RemoteFeishuProcessingStateRecord| record.active))
    }

    /// 清理保留期之前已结束的 processing reaction。
    pub fn remove_inactive_records_before(
        path: impl AsRef<Path>,
        cutoff_ms: u64,
    ) -> AxonResult<usize> {
        RemoteStateStoreRuntime::delete_indexed_before(
            path,
            "feishu_processing",
            &["inactive"],
            cutoff_ms,
        )
    }
}

fn state_index(record: &RemoteFeishuProcessingStateRecord) -> RemoteStateStoreIndex {
    RemoteStateStoreIndex::new(
        "feishu_processing",
        record.message_id_sha256.clone(),
        if record.active { "active" } else { "inactive" },
        record.recorded_at_ms,
    )
}

fn validate_record(record: &RemoteFeishuProcessingStateRecord) -> AxonResult<()> {
    if record.message_id_sha256.trim().is_empty()
        || record.reaction_id.trim().is_empty()
        || record.reaction_type.trim().is_empty()
        || record.recorded_at_ms == 0
    {
        return Err(AxonError::runtime_failure(
            "Feishu processing state 记录无效",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(active: bool, recorded_at_ms: u64) -> RemoteFeishuProcessingStateRecord {
        RemoteFeishuProcessingStateRecord {
            schema_version: 1,
            message_id_sha256: sha256_hex("om_message"),
            reaction_id: "reaction-1".to_owned(),
            reaction_type: "Typing".to_owned(),
            active,
            recorded_at_ms,
        }
    }

    #[test]
    fn inactive_current_state_does_not_resurrect_the_prior_active_reaction() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("processing.sqlite3");
        // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
        let _close_pool = axon_test_support::DeferredCleanup::new({
            let path = path.clone();
            move || axon_remote_runtime::RemoteStateStoreRuntime::close_path(&path)
        });
        RemoteFeishuProcessingStateStoreRuntime::append_record(&path, &record(true, 1))
            .expect("write active state");
        RemoteFeishuProcessingStateStoreRuntime::append_record(&path, &record(false, 2))
            .expect("write inactive state");

        assert_eq!(
            RemoteFeishuProcessingStateStoreRuntime::active_reaction(&path, "om_message")
                .expect("read active state"),
            None
        );
    }
}
