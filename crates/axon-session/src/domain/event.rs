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
        /// 收口时补合成的历史项（崩溃遗留的工具结果等）。
        ///
        /// 与终态放在同一条事件里落地，避免「先补历史、再写终态」的两次非原子追加：
        /// 中间崩溃会留下「有结果无终态」或「有终态无结果」的半截历史。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        appended_items: Vec<super::history::ThreadHistoryItem>,
    },
    /// Turn 执行失败。
    TurnFailed {
        /// Turn ID。
        turn_id: String,
        /// 结构化失败事实。
        failure: RuntimeFailure,
    },
    /// 点对点补写某一轮 metadata 里由轮后治理产生的两项事实。
    ///
    /// 治理回写不再复制整份轮次记录（实测单条记录 676 KB 到数 MB），只追加这一条增量；
    /// 投影时套用到该轮，因此读侧拿到的 metadata 与整份重写完全一致。
    TurnMetadataPatched {
        /// 该轮的 assistant part 标识：轮次在 thread 里的稳定身份。
        assistant_part_id: String,
        /// 记忆反思提示；`None` 表示这一项不改。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notice: Option<String>,
        /// 本轮的 affect 序列；空表示这一项不改。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        affect_sequence: Vec<String>,
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
    /// 标注子会话来源。
    ///
    /// 来源只决定界面名册列不列这条会话：运行侧内务（自迭代、反思、语义显著度）
    /// 不是用户要的子代理，缺省（未标注）按委派处理，以兼容旧日志。
    ThreadAnnotated {
        /// 子会话来源。
        child_origin: Option<String>,
    },
    /// 归档。
    ThreadArchived,
    /// 取消归档。
    ThreadUnarchived,
    /// 关闭。
    ThreadClosed,
}
