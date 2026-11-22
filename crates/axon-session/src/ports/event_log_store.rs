//! 会话事件日志端口。
//!
//! 与 [`super::RuntimeStore`] 的分工：本端口只负责事件的追加与读取；租约、队列、
//! 状态机仍留在 `RuntimeStore`。拆分的目的见 `docs/dev/storage-architecture.md`：
//! 事件流要走 append-only 文件，状态机要继续留在 SQLite，两者不能再共用一套事务。

use axon_domain::{DaemonSessionId, InteractionId};
use std::sync::atomic::{AtomicU64, Ordering};

use super::runtime_store::RuntimeStoreError;
use crate::domain::SessionKind;
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};

/// 带轮次语义、落盘时必须带 Interaction 归属的事件类型（INV-4）。
///
/// `wire_event` 是逐轮正文：没有归属就意味着它在任何轮次视图里都读不到。会话级事件
/// （会话与作业生命周期、会话累计指标、回合之外的迟到作答）不属于任何一轮，仍可走
/// 无归属入口。
const ATTRIBUTION_REQUIRED_EVENT_TYPES: &[&str] = &["wire_event"];

/// 该事件类型是否必须带 Interaction 归属（INV-4）。
///
/// 存量修复工具按这一条判定「无归属帧」：会话级事件（会话生命周期、累计指标、
/// 回合之外的迟到作答）本来就不属于任何一轮，把它们算成缺归属会给出假的缺口，
/// 回填还会给它们写上一条与语义不符的归属。
#[must_use]
pub fn requires_attribution(event_type: &str) -> bool {
    ATTRIBUTION_REQUIRED_EVENT_TYPES.contains(&event_type)
}

/// 无归属入口的合法用途白名单：这些事实只属于会话，不属于某一轮。
///
/// 逐条对应真实调用方：作业生命周期由 CLI 直接写会话流，其余是会话级投影与收口事实。
/// 新增类型时先在这里登记，再确认它确实无法归属到某一轮。
const SESSION_LEVEL_EVENT_TYPES: &[&str] = &[
    "job_started",
    "job_completed",
    "session_created",
    "session_recovered",
    "session_cancelled",
    "session_status",
    "turn_metrics",
];

/// 允许「会话级 `wire_event`」的会话类型：会话级 wire 写入只发生在这些车道上。
///
/// 目前唯一的写入方是回合之外的**迟到作答**（`DurableQuestionEventWriter`）：提问在上一轮
/// 结束时没有结算，答案在回合之外才到，此时车道上没有任何活着的轮次可以承接这一帧，
/// 信封里也没有 interactionId。它属于「这条会话的输入事实」，按会话级事件处理。
///
/// 子代理会话不在名单里：子代理的逐字稿**只**通过 Interaction 视图读取（子代理详情按
/// Interaction 订阅正文），而且不存在回合之外的会话级 wire 写入方，因此它的逐轮正文
/// 必须带归属，否则整段逐字稿不可见。
const SESSION_LEVEL_WIRE_EVENT_KINDS: &[SessionKind] =
    &[SessionKind::Primary, SessionKind::Cron, SessionKind::Goal];

/// 放行的无归属 `wire_event` 累计次数。
///
/// 放行是为了不打断迟到作答，但必须可观测：这条路径一旦出问题（例如以后有人把它当成
/// 默认写法），计数与日志是第一现场证据。
static SESSION_LEVEL_WIRE_WRITES: AtomicU64 = AtomicU64::new(0);

/// 会话事件日志的持久化端口。
pub trait EventLogStore: Send + Sync {
    /// 校验一批事件能否走「无归属」入口（INV-4 的运行期断言）。
    ///
    /// 拒绝口径：事件类型带轮次语义（[`ATTRIBUTION_REQUIRED_EVENT_TYPES`]），且写入的会话
    /// 类型不在会话级 wire 白名单（[`SESSION_LEVEL_WIRE_EVENT_KINDS`]）里。子代理会话因此
    /// 一条无归属的逐轮正文都写不进来；会话级 wire 写入（迟到作答）照常放行，但按会话与
    /// 事件类型留结构化日志并计数，避免「被放行过」这件事无人知晓。
    ///
    /// 白名单之外的非逐轮事件仍放行，但留一条告警，避免日后新增的逐轮事件静默落成孤儿帧。
    fn ensure_session_level_events(
        session_id: &DaemonSessionId,
        session_kind: SessionKind,
        events: &[(String, String)],
    ) -> Result<(), RuntimeStoreError> {
        for (event_type, _) in events {
            if !ATTRIBUTION_REQUIRED_EVENT_TYPES.contains(&event_type.as_str()) {
                if !SESSION_LEVEL_EVENT_TYPES.contains(&event_type.as_str()) {
                    axon_logger::warning(&format!(
                        "会话级事件入口收到不在白名单内的事件类型 {event_type}：请确认它不属于任何一轮"
                    ));
                }
                continue;
            }
            if !SESSION_LEVEL_WIRE_EVENT_KINDS.contains(&session_kind) {
                return Err(RuntimeStoreError::Storage(format!(
                    "会话 {} 的 {event_type} 必须写 Interaction 归属：无归属入口只接受会话级事件（{}）",
                    session_id.as_str(),
                    SESSION_LEVEL_EVENT_TYPES.join(" / ")
                )));
            }
            let writes = SESSION_LEVEL_WIRE_WRITES.fetch_add(1, Ordering::AcqRel) + 1;
            axon_logger::info(&format!(
                "[event-log] session={} kind={} event_type={event_type} 无归属放行（累计 {writes} 次，属会话级 wire 写入）",
                session_id.as_str(),
                session_kind.as_str()
            ));
        }
        Ok(())
    }

    /// 读取累计放行的无归属 `wire_event` 次数。
    ///
    /// 只为可观测与回归断言：任何增长都说明会话级 wire 写入仍在发生（迟到作答）。
    fn session_level_wire_writes() -> u64 {
        SESSION_LEVEL_WIRE_WRITES.load(Ordering::Acquire)
    }

    /// 追加一条事件并校验 Host generation。
    ///
    /// **仅会话级事件**：逐轮正文必须走 [`Self::append_attributed_events`]，否则它不会
    /// 出现在任何轮次视图里。运行期由 [`Self::ensure_session_level_events`] 拒绝子代理
    /// 会话上的 `wire_event`。
    fn append_event(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        event_type: &str,
        payload_json: &str,
        now_ms: u64,
    ) -> Result<SessionEventRecord, RuntimeStoreError>;

    /// 在同一批次中按输入顺序追加多条事件，并只校验一次 Host generation。
    ///
    /// **仅会话级事件**：语义与 [`Self::append_event`] 相同，见那里的归属要求。
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
