//! 会话事件日志端口。
//!
//! 与 [`super::RuntimeStore`] 的分工：本端口只负责事件的追加与读取；租约、队列、
//! 状态机仍留在 `RuntimeStore`。拆分的目的见 `docs/storage-architecture.md`：
//! 事件流要走 append-only 文件，状态机要继续留在 SQLite，两者不能再共用一套事务。

use axon_domain::{DaemonSessionId, InteractionId};

use super::runtime_store::RuntimeStoreError;
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};

/// 会话事件日志的持久化端口。
pub trait EventLogStore: Send + Sync {
    /// 追加一条事件并校验 Host generation。
    fn append_event(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        event_type: &str,
        payload_json: &str,
        now_ms: u64,
    ) -> Result<SessionEventRecord, RuntimeStoreError>;

    /// 在同一批次中按输入顺序追加多条事件，并只校验一次 Host generation。
    fn append_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        events: &[(String, String)],
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;

    /// 校验 Interaction processing lease 之后按顺序追加一组事件。
    fn append_interaction_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        interaction_id: &InteractionId,
        lease_token: &str,
        events: &[(String, String)],
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;

    /// 读取序号严格大于 `after_sequence` 的至多 `limit` 条事件。
    fn replay_events(
        &self,
        session_id: &DaemonSessionId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;

    /// 读取某条 Interaction 自身的尾窗帧，按序号升序。
    fn interaction_tail_window(
        &self,
        session_id: &DaemonSessionId,
        interaction_id: &InteractionId,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
}
