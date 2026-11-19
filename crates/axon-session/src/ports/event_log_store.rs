//! 会话事件日志端口。
//!
//! 与 [`super::RuntimeStore`] 的分工：本端口只负责事件的追加与读取；租约、队列、
//! 状态机仍留在 `RuntimeStore`。拆分的目的见 `docs/dev/storage-architecture.md`：
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

    /// 按 Interaction 归属追加一组事件，不校验租约。
    ///
    /// 逐轮指标与终态徽标都在轮次收口之后写入，那时租约早就释放了。本入口只写归属列，
    /// 不碰状态机：收口后的事实因此仍能落到所属轮次上，而无租约写入也改不了任何状态。
    fn append_attributed_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        interaction_id: &InteractionId,
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

    /// 读取某条 Interaction 在 `after_sequence` 之后的至多 `limit` 条事件。
    ///
    /// 与 [`Self::replay_events`] 的区别是只返回该 Interaction 自己的帧：一次历史
    /// 分页不该为了一条交互把整段会话都解压一遍。实现可以借助帧索引的交互指纹缩小
    /// 解压范围，但命中后仍必须逐条确认归属。
    fn replay_interaction_events(
        &self,
        session_id: &DaemonSessionId,
        interaction_id: &InteractionId,
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

    /// 读取一组 Interaction 各自最近一条 `turn_metrics` 事件，按入参顺序返回。
    ///
    /// 历史索引页要为每条轮次给出一行读数（用时、token、首字、速率），权威事实是每轮
    /// 收口时写下的那条 `turn_metrics`；逐轮整段回放等于把一页正文全部解压一遍。
    /// 实现可以借助帧索引的交互指纹从尾部倒推，命中后仍必须逐条确认归属；没有该事件的
    /// 轮次不出现在结果里，由调用方按「读数未知」降级。
    fn latest_turn_metrics(
        &self,
        session_id: &DaemonSessionId,
        interaction_ids: &[InteractionId],
    ) -> Result<Vec<(String, axon_eventlog::TurnMetrics)>, RuntimeStoreError>;
}
