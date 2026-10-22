//! Thread 事件。

use axon_domain::RuntimeFailure;
use serde::{Deserialize, Serialize};

use super::{ThreadCompaction, ThreadDetail, ThreadId, ThreadSettings, ThreadSource, Turn};

/// Thread 事件记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEventRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 事件序号。
    pub sequence: u64,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 事件内容。
    pub event: ThreadEvent,
}

/// Thread 事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadEvent {
    /// 创建 thread。
    ThreadStarted {
        /// 会话树根 ID。
        session_root_id: ThreadId,
        /// 父 thread ID。
        parent_thread_id: Option<ThreadId>,
        /// fork 来源 ID。
        forked_from_thread_id: Option<ThreadId>,
        /// 来源。
        source: ThreadSource,
        /// 是否仅内存态。
        ephemeral: bool,
        /// 设置快照。
        settings: ThreadSettings,
    },
    /// 完成一个 turn。
    TurnCompleted {
        /// Turn 数据。
        turn: Turn,
    },
    /// 开始一个 turn。
    TurnStarted {
        /// Turn 数据。
        turn: Turn,
    },
    /// 追加 active turn 的新增历史 item。
    TurnItemsAppended {
        /// Turn ID。
        turn_id: String,
        /// 本次新增的结构化历史 item。
        items: Vec<super::ThreadHistoryItem>,
    },
    /// 中断一个 turn。
    TurnInterrupted {
        /// Turn ID。
        turn_id: String,
    },
    /// Turn 执行失败。
    TurnFailed {
        /// Turn ID。
        turn_id: String,
        /// 结构化失败事实。
        failure: RuntimeFailure,
    },
    /// 恢复 thread。
    ThreadResumed,
    /// 安装压缩后的上下文历史。
    ThreadCompacted {
        /// 压缩快照。
        compaction: ThreadCompaction,
    },
    /// 持久化完整 thread checkpoint，用于从短尾部事件恢复。
    ThreadCheckpoint {
        /// checkpoint 时的完整 thread 详情。
        detail: ThreadDetail,
    },
    /// fork thread。
    ThreadForked {
        /// 来源 thread。
        source_thread_id: ThreadId,
    },
    /// 命名 thread。
    ThreadNamed {
        /// 新名称。
        name: Option<String>,
    },
    /// 归档。
    ThreadArchived,
    /// 取消归档。
    ThreadUnarchived,
    /// 关闭。
    ThreadClosed,
}
