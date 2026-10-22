//! Feishu Agent 回复 artifact。
//!
//! 通用 remote notification outbox 只保存 bounded summary。本模块保存同机
//! Feishu gateway 生成的完整 Agent 回复和 reply 目标，供 delivery worker
//! 发送真实会话回复，不把正文塞进通用远控通知协议。

use std::path::{Path, PathBuf};

use axon_display::normalize_markdown_source;
use axon_domain::{AxonError, AxonResult};
use axon_remote_runtime::{RemoteStateStoreIndex, RemoteStateStoreRuntime};
use serde::{Deserialize, Serialize};

use crate::RemoteFeishuMessageTarget;

/// Feishu Agent 回复 artifact 文件名。
pub const REMOTE_FEISHU_AGENT_REPLY_FILE: &str = "remote_feishu_agent_replies.sqlite3";
/// Feishu Agent 回复 artifact schema 版本。
pub const REMOTE_FEISHU_AGENT_REPLY_SCHEMA_VERSION: u32 = 2;
/// Feishu Agent 回复空内容兜底。
pub const REMOTE_FEISHU_AGENT_REPLY_EMPTY_FALLBACK: &str = "Agent 未生成可发送回复。";
const REMOTE_FEISHU_AGENT_REPLY_ID_MAX_CHARS: usize = 128;
const FEISHU_AGENT_REPLY_STATE_NAMESPACE: &str = "feishu_agent_reply";

/// Feishu Agent 回复 artifact 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuAgentReplyArtifactInput {
    /// dispatch id。
    pub dispatch_id: String,
    /// 逻辑运行会话 ID。
    pub run_session_id: String,
    /// 原始入站消息 ID，用于最终投递后的 processing 清理。
    pub processing_message_id: Option<String>,
    /// Feishu 发送目标。
    pub target: RemoteFeishuMessageTarget,
    /// 可选 thread id，仅用于审计和后续扩展。
    pub thread_id: Option<String>,
    /// Agent 回复正文。
    pub body: String,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// Feishu Agent 回复 artifact 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFeishuAgentReplyArtifactRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// dispatch id。
    pub dispatch_id: String,
    /// 逻辑运行会话 ID。
    pub run_session_id: String,
    /// 原始入站消息 ID，用于最终投递后的 processing 清理。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing_message_id: Option<String>,
    /// create API receive_id。
    pub receive_id: String,
    /// create API receive_id_type。
    pub receive_id_type: String,
    /// reply API 目标消息 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    /// Feishu thread/root id。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Agent 回复正文。
    pub body: String,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// Feishu Agent 回复 artifact runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuAgentReplyArtifactRuntime;

impl RemoteFeishuAgentReplyArtifactRuntime {
    /// 解析默认 artifact 存储路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_FEISHU_AGENT_REPLY_FILE)
    }

    /// 从输入构造 artifact 记录。
    pub fn record_from_input(
        input: RemoteFeishuAgentReplyArtifactInput,
    ) -> AxonResult<RemoteFeishuAgentReplyArtifactRecord> {
        let body = normalized_body(&input.body);
        let record = RemoteFeishuAgentReplyArtifactRecord {
            schema_version: REMOTE_FEISHU_AGENT_REPLY_SCHEMA_VERSION,
            dispatch_id: input.dispatch_id,
            run_session_id: input.run_session_id,
            processing_message_id: input.processing_message_id,
            receive_id: input.target.receive_id,
            receive_id_type: input.target.receive_id_type,
            reply_to_message_id: input.target.reply_to_message_id,
            thread_id: input.thread_id,
            body,
            recorded_at_ms: input.recorded_at_ms,
        };
        validate_record(&record)?;
        Ok(record)
    }

    /// 写入 artifact 当前状态。
    pub fn store_record(
        path: impl AsRef<Path>,
        record: &RemoteFeishuAgentReplyArtifactRecord,
    ) -> AxonResult<()> {
        validate_record(record)?;
        RemoteStateStoreRuntime::upsert(path, &record.dispatch_id, artifact_index(record), record)
    }

    /// 读取所有 artifact 当前状态。
    pub fn read_records(
        path: impl AsRef<Path>,
    ) -> AxonResult<Vec<RemoteFeishuAgentReplyArtifactRecord>> {
        RemoteStateStoreRuntime::read_all(path)?
            .into_iter()
            .map(|record| {
                validate_record(&record)?;
                Ok(record)
            })
            .collect()
    }

    /// 查询指定 dispatch 的 artifact 当前状态。
    pub fn latest_for_dispatch(
        path: impl AsRef<Path>,
        dispatch_id: &str,
    ) -> AxonResult<Option<RemoteFeishuAgentReplyArtifactRecord>> {
        validate_id(dispatch_id, "Feishu Agent 回复 dispatch id 无效")?;
        RemoteStateStoreRuntime::read(path, dispatch_id)?.map_or(Ok(None), |record| {
            validate_record(&record)?;
            Ok(Some(record))
        })
    }

    /// 查询指定逻辑运行会话关联的 dispatch ID。
    pub fn dispatches_for_run_sessions(
        path: impl AsRef<Path>,
        run_session_ids: &[String],
    ) -> AxonResult<Vec<String>> {
        if run_session_ids
            .iter()
            .any(|session_id| session_id.trim().is_empty())
        {
            return Err(AxonError::runtime_failure(
                "Feishu Agent 回复运行会话 ID 无效",
            ));
        }
        if run_session_ids.is_empty() || !path.as_ref().exists() {
            return Ok(Vec::new());
        }
        RemoteStateStoreRuntime::keys_for_indexed_partitions(
            path,
            FEISHU_AGENT_REPLY_STATE_NAMESPACE,
            run_session_ids,
        )
    }

    /// 删除指定 dispatch 的 artifact 当前状态。
    pub fn remove_for_dispatches(
        path: impl AsRef<Path>,
        dispatch_ids: &[String],
    ) -> AxonResult<usize> {
        RemoteStateStoreRuntime::delete_keys(path, dispatch_ids)
    }
}

impl RemoteFeishuAgentReplyArtifactRecord {
    /// 转换为 Feishu 消息目标。
    #[must_use]
    pub fn target(&self) -> RemoteFeishuMessageTarget {
        RemoteFeishuMessageTarget {
            receive_id: self.receive_id.clone(),
            receive_id_type: self.receive_id_type.clone(),
            reply_to_message_id: self.reply_to_message_id.clone(),
        }
    }
}

fn normalized_body(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        REMOTE_FEISHU_AGENT_REPLY_EMPTY_FALLBACK.to_owned()
    } else {
        normalize_markdown_source(trimmed).trim().to_owned()
    }
}

fn validate_record(record: &RemoteFeishuAgentReplyArtifactRecord) -> AxonResult<()> {
    if record.schema_version != REMOTE_FEISHU_AGENT_REPLY_SCHEMA_VERSION
        || record.recorded_at_ms == 0
    {
        return Err(AxonError::runtime_failure(
            "Feishu Agent 回复 artifact 元数据无效",
        ));
    }
    validate_id(&record.dispatch_id, "Feishu Agent 回复 dispatch id 无效")?;
    validate_id(&record.run_session_id, "Feishu Agent 回复运行会话 ID 无效")?;
    validate_id(&record.receive_id, "Feishu Agent 回复 receive_id 无效")?;
    validate_id(
        &record.receive_id_type,
        "Feishu Agent 回复 receive_id_type 无效",
    )?;
    if let Some(processing_message_id) = &record.processing_message_id {
        validate_id(
            processing_message_id,
            "Feishu Agent 回复 processing 消息 ID 无效",
        )?;
    }
    if let Some(reply_to_message_id) = &record.reply_to_message_id {
        validate_id(reply_to_message_id, "Feishu Agent 回复 reply 目标无效")?;
    }
    if let Some(thread_id) = &record.thread_id {
        validate_id(thread_id, "Feishu Agent 回复 thread id 无效")?;
    }
    if record.body.trim().is_empty() {
        return Err(AxonError::runtime_failure("Feishu Agent 回复正文为空"));
    }
    Ok(())
}

fn validate_id(value: &str, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > REMOTE_FEISHU_AGENT_REPLY_ID_MAX_CHARS {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

fn artifact_index(record: &RemoteFeishuAgentReplyArtifactRecord) -> RemoteStateStoreIndex {
    RemoteStateStoreIndex::new(
        FEISHU_AGENT_REPLY_STATE_NAMESPACE,
        record.run_session_id.clone(),
        "ready",
        record.recorded_at_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_reply_artifact_preserves_reply_target_and_body() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-agent-reply-{}.sqlite3",
            unique_suffix()
        ));
        let record = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
            RemoteFeishuAgentReplyArtifactInput {
                dispatch_id: "dispatch-1".to_owned(),
                run_session_id: "session-1".to_owned(),
                processing_message_id: Some("om_input".to_owned()),
                target: RemoteFeishuMessageTarget {
                    receive_id: "oc_chat".to_owned(),
                    receive_id_type: "chat_id".to_owned(),
                    reply_to_message_id: Some("om_parent".to_owned()),
                },
                thread_id: Some("root_1".to_owned()),
                body: "完整 Agent 回复".to_owned(),
                recorded_at_ms: 100,
            },
        )
        .expect("record");

        RemoteFeishuAgentReplyArtifactRuntime::store_record(&path, &record).expect("store");
        let latest =
            RemoteFeishuAgentReplyArtifactRuntime::latest_for_dispatch(&path, "dispatch-1")
                .expect("latest")
                .expect("record exists");

        assert_eq!(latest.body, "完整 Agent 回复");
        assert_eq!(latest.reply_to_message_id.as_deref(), Some("om_parent"));
        assert_eq!(latest.target().receive_id_type, "chat_id");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn removes_only_artifacts_owned_by_expired_run_session() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join(REMOTE_FEISHU_AGENT_REPLY_FILE);
        // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
        let _close_pool = axon_test_support::DeferredCleanup::new({
            let path = path.clone();
            move || axon_remote_runtime::RemoteStateStoreRuntime::close_path(&path)
        });
        for (dispatch_id, run_session_id) in [
            ("dispatch-expired", "expired"),
            ("dispatch-active", "active"),
        ] {
            let record = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
                RemoteFeishuAgentReplyArtifactInput {
                    dispatch_id: dispatch_id.to_owned(),
                    run_session_id: run_session_id.to_owned(),
                    processing_message_id: None,
                    target: RemoteFeishuMessageTarget {
                        receive_id: "oc_chat".to_owned(),
                        receive_id_type: "chat_id".to_owned(),
                        reply_to_message_id: None,
                    },
                    thread_id: None,
                    body: "reply".to_owned(),
                    recorded_at_ms: 100,
                },
            )
            .expect("record");
            RemoteFeishuAgentReplyArtifactRuntime::store_record(&path, &record).expect("store");
        }

        let dispatch_ids = RemoteFeishuAgentReplyArtifactRuntime::dispatches_for_run_sessions(
            &path,
            &["expired".to_owned()],
        )
        .expect("list");
        let removed =
            RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(&path, &dispatch_ids)
                .expect("remove");

        assert_eq!(removed, 1);
        assert!(
            RemoteFeishuAgentReplyArtifactRuntime::latest_for_dispatch(&path, "dispatch-expired")
                .expect("read")
                .is_none()
        );
        assert!(
            RemoteFeishuAgentReplyArtifactRuntime::latest_for_dispatch(&path, "dispatch-active")
                .expect("read")
                .is_some()
        );
    }

    #[test]
    fn agent_reply_artifact_uses_empty_response_fallback() {
        let record = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
            RemoteFeishuAgentReplyArtifactInput {
                dispatch_id: "dispatch-empty".to_owned(),
                run_session_id: "session-empty".to_owned(),
                processing_message_id: None,
                target: RemoteFeishuMessageTarget {
                    receive_id: "oc_chat".to_owned(),
                    receive_id_type: "chat_id".to_owned(),
                    reply_to_message_id: None,
                },
                thread_id: None,
                body: "   ".to_owned(),
                recorded_at_ms: 100,
            },
        )
        .expect("record");

        assert_eq!(record.body, REMOTE_FEISHU_AGENT_REPLY_EMPTY_FALLBACK);
    }

    #[test]
    fn agent_reply_artifact_uses_shared_markdown_projection() {
        let source = "### 任务标题\u{3000}-\u{00a0}状态：进行中 - 任务目标：核验消息链路";
        let record = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
            RemoteFeishuAgentReplyArtifactInput {
                dispatch_id: "dispatch-markdown".to_owned(),
                run_session_id: "session-markdown".to_owned(),
                processing_message_id: None,
                target: RemoteFeishuMessageTarget {
                    receive_id: "oc_chat".to_owned(),
                    receive_id_type: "chat_id".to_owned(),
                    reply_to_message_id: None,
                },
                thread_id: None,
                body: source.to_owned(),
                recorded_at_ms: 100,
            },
        )
        .expect("record");

        assert_eq!(
            record.body,
            normalize_markdown_source(source).trim(),
            "远控回复必须与 TUI 共享 Markdown 语义投影"
        );
    }

    #[test]
    fn agent_reply_artifact_preserves_long_response_without_truncation() {
        let body = "完整 Agent 回复段落。".repeat(4_000);
        let record = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
            RemoteFeishuAgentReplyArtifactInput {
                dispatch_id: "dispatch-long".to_owned(),
                run_session_id: "session-long".to_owned(),
                processing_message_id: None,
                target: RemoteFeishuMessageTarget {
                    receive_id: "oc_chat".to_owned(),
                    receive_id_type: "chat_id".to_owned(),
                    reply_to_message_id: None,
                },
                thread_id: None,
                body: body.clone(),
                recorded_at_ms: 100,
            },
        )
        .expect("record");

        assert_eq!(record.body, body);
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    }
}
