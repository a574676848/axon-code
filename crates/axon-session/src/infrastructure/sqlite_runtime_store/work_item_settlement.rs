//! 工作项终态的唯一出口。
//!
//! 一条工作项离开在途状态时要发生四件事，缺一条就会出现「状态说结束了，历史里没有
//! 结论」或「历史里有结论，派生列却是旧词」：改 `status`、写 `terminal_kind`、
//! 归档、追流水账。四步的**顺序**也是契约的一部分：`terminal_kind` 必须先于归档
//! 写入，归档把 `COALESCE(terminal_kind,status)` 搬进历史表，漏写就会让历史行的
//! 派生列带回队列口径的旧词，含该轮的整条车道历史都可能整体读不出来。
//!
//! 被动隔离（`quarantine_interrupted_executions` /
//! `quarantine_interrupted_interactions`）、正常处理收口与应用层的显式收口都必须
//! 经过 [`settle_work_item`]，不允许任何路径自己再写一份 UPDATE。

use super::execution_repository::status_code;
use super::helpers::{i64_value, storage};
use super::{SqliteRuntimeStore, work_item_archive};
use crate::domain::interaction_status;
use crate::ports::{
    InFlightTurnFacts, InteractionSettlementStore, InteractionStoreError, QueuedTurnFacts,
    RuntimeStore, RuntimeStoreError,
};
use axon_domain::{
    DaemonSessionId, ExecutionStatus, InteractionId, InteractionStatus, LedgerEventId,
};
use rusqlite::{Connection, TransactionBehavior, params};

/// 写进 `work_items.terminal_kind` 的中断类别。
///
/// 队列口径的 `failed` / `cancelled` / `uncertain` 在 Interaction 口径下都收敛到它，
/// 取词来自领域枚举，不在本模块各写一份字面量。
pub(super) const TERMINAL_INTERRUPTED: &str =
    interaction_status::token(InteractionStatus::Interrupted);

/// 结论状态却没有 `terminal_kind` 的行数：INV-2 要求它恒为零。
///
/// 存储层与 `axon doctor` 共用这一份定义：状态词表由 `ExecutionStatus` 的结论集合
/// 派生，领域枚举新增结论状态时这里自动跟随，不需要再改一处字面量。查 `work_items_all`
/// 而不是在途表：归档行的 `terminal_kind` 由历史表约束保证非空，两个物理表必须一起成立。
#[must_use]
pub fn conclusive_work_items_without_terminal_kind_sql() -> String {
    format!(
        "SELECT COUNT(*) FROM work_items_all WHERE status IN ({}) AND terminal_kind IS NULL",
        status_list(&interaction_status::CONCLUSIVE_EXECUTION_STATUSES)
    )
}

/// 一条推理过程之外的终态结论。
pub(super) struct Settlement<'a> {
    /// 写入 `work_items.status` 的结论状态。
    status: ExecutionStatus,
    /// 写入 `work_items.terminal_kind` 的终态类别（Interaction 口径）。
    terminal_kind: &'a str,
    /// 需要追加的流水账事件；没有流水账语义的路径为 `None`。
    ledger: Option<LedgerTrace<'a>>,
}

impl<'a> Settlement<'a> {
    /// 只写终态的结论结算。
    pub(super) const fn concluded(status: ExecutionStatus, terminal_kind: &'a str) -> Self {
        Self {
            status,
            terminal_kind,
            ledger: None,
        }
    }

    /// 在终态之上再追一条流水账事件。
    #[must_use]
    pub(super) const fn with_ledger(mut self, ledger: LedgerTrace<'a>) -> Self {
        self.ledger = Some(ledger);
        self
    }
}

/// 一条流水账事件。
///
/// 事件名是升级期核对脚本的输入，取值固定为既有的 `execution.uncertain` 与
/// `interaction.interrupted`，不得为了「统一」而改名。
pub(super) struct LedgerTrace<'a> {
    /// 既有事件类型。
    pub(super) event_type: &'a str,
    /// 事件主语：执行标识或交互标识。
    pub(super) subject_id: &'a str,
    /// 事件载荷。
    pub(super) payload: &'a str,
}

/// 工作项的定位与守卫条件。
///
/// 终态 UPDATE 只在行仍处于在途时生效，因此每条路径都带一个状态守卫；守卫不同，
/// 语义就不同，这里把四种既有可能列清，调用方不必自己拼 WHERE。
#[derive(Clone, Copy)]
pub(super) enum WorkItemLocator<'a> {
    /// 仍在跑的执行账本行：按执行标识。
    RunningExecution(&'a str),
    /// 尚未形成结论的执行账本行（排队、等待或运行中）：按执行标识。
    InFlightExecution(&'a str),
    /// 从未派发的排队行：恢复收敛专用。
    ///
    /// 守卫只有「仍停在排队态」：这类行既没有租约也没有执行者，而 `running` 行一定有人
    /// 持有租约，收口它等于从活着的执行者手里抢走终态。
    QueuedExecution(&'a str),
    /// 从未开始执行的排队入站轮次：排队判死专用。
    ///
    /// 与 [`Self::QueuedExecution`] 同一守卫，差别只在定位键换成入站事实标识。
    QueuedInbox(&'a str),
    /// 仍在跑的入站轮次行：按入站事实，不校验租约，供判死后收口。
    RunningInbox(&'a str),
    /// 仍在跑的入站轮次行并校验当前 fencing token：正常处理路径用它。
    InboxLease {
        /// 入站事实标识。
        inbox_id: &'a str,
        /// 本次领取的 fencing token。
        lease_token: &'a str,
    },
}

impl<'a> WorkItemLocator<'a> {
    /// 定位子句：定位键固定为 `?4`，校验令牌时追加 `?5`。
    fn predicate(self) -> &'static str {
        match self {
            Self::RunningExecution(_) | Self::InFlightExecution(_) | Self::QueuedExecution(_) => {
                "execution_id=?4"
            }
            Self::RunningInbox(_) | Self::QueuedInbox(_) => "inbox_id=?4",
            Self::InboxLease { .. } => "inbox_id=?4 AND lease_token=?5",
        }
    }

    /// 状态守卫：只把仍在途的行改写成结论。
    fn guard(self) -> String {
        match self {
            Self::InFlightExecution(_) => format!(
                "status IN ({})",
                status_list(&interaction_status::IN_FLIGHT_EXECUTION_STATUSES)
            ),
            Self::QueuedExecution(_) | Self::QueuedInbox(_) => {
                format!("status IN ({})", status_list(&[ExecutionStatus::Queued]))
            }
            Self::RunningExecution(_) | Self::RunningInbox(_) | Self::InboxLease { .. } => {
                format!("status IN ({})", status_list(&[ExecutionStatus::Running]))
            }
        }
    }

    /// 定位键：归档只按它搬运，不重复判断状态。
    fn work_key(self) -> &'a str {
        match self {
            Self::RunningExecution(key)
            | Self::InFlightExecution(key)
            | Self::QueuedExecution(key) => key,
            Self::RunningInbox(key) | Self::QueuedInbox(key) => key,
            Self::InboxLease { inbox_id, .. } => inbox_id,
        }
    }

    /// 是否按入站事实施加租约令牌校验。
    fn lease_token(self) -> Option<&'a str> {
        match self {
            Self::InboxLease { lease_token, .. } => Some(lease_token),
            Self::RunningExecution(_)
            | Self::InFlightExecution(_)
            | Self::QueuedExecution(_)
            | Self::RunningInbox(_)
            | Self::QueuedInbox(_) => None,
        }
    }

    /// 把行搬进历史表：在途表只留排队与运行中的行。
    fn archive(self, connection: &Connection, now: i64) -> rusqlite::Result<()> {
        match self {
            Self::RunningExecution(_) | Self::InFlightExecution(_) | Self::QueuedExecution(_) => {
                work_item_archive::archive_work_item(connection, self.work_key(), now)
            }
            Self::RunningInbox(_) | Self::QueuedInbox(_) | Self::InboxLease { .. } => {
                work_item_archive::archive_work_item_by_inbox(connection, self.work_key(), now)
            }
        }
    }
}

/// 把一条在途工作项收敛为终态；行已不在途时返回 `false`。
///
/// 四件事在同一事务里完成，调用方负责提交。返回 `false` 表示条件守卫没有命中（已被
/// 其他事务收口、或行本来就不在途），此时没有写入任何事实，也不得再归档或追流水账。
pub(super) fn settle_work_item(
    connection: &Connection,
    locator: WorkItemLocator<'_>,
    settlement: &Settlement<'_>,
    now: i64,
) -> Result<bool, RuntimeStoreError> {
    let statement = format!(
        "UPDATE work_items SET status=?1,terminal_kind=?2,lease_token=NULL,lease_json=NULL,\
         claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL,retry_at_ms=NULL,\
         committed_at_ms=COALESCE(committed_at_ms,?3),updated_at_ms=?3 \
         WHERE {} AND {}",
        locator.predicate(),
        locator.guard()
    );
    let changed = match locator.lease_token() {
        Some(lease_token) => connection.execute(
            &statement,
            params![
                status_code(settlement.status),
                settlement.terminal_kind,
                now,
                locator.work_key(),
                lease_token
            ],
        ),
        None => connection.execute(
            &statement,
            params![
                status_code(settlement.status),
                settlement.terminal_kind,
                now,
                locator.work_key()
            ],
        ),
    }
    .map_err(storage)?;
    if changed == 0 {
        return Ok(false);
    }
    // 终态已经落定，才允许搬运：归档读的就是刚写进去的 terminal_kind。
    locator.archive(connection, now).map_err(storage)?;
    if let Some(ledger) = settlement.ledger.as_ref() {
        append_ledger(connection, ledger, now)?;
    }
    Ok(true)
}

/// 追加一条流水账事件：序号由账本自己分配，幂等键与事件标识同源。
fn append_ledger(
    connection: &Connection,
    ledger: &LedgerTrace<'_>,
    now: i64,
) -> Result<(), RuntimeStoreError> {
    let event_id = LedgerEventId::new(format!("event-{}-{}", ledger.event_type, ledger.subject_id))
        .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    connection
        .execute(
            "INSERT INTO ledger_events(event_id,sequence,event_type,idempotency_key,\
             cause_event_id,payload_json,occurred_at_ms) \
             VALUES(?1,(SELECT COALESCE(MAX(sequence),0)+1 FROM ledger_events),?2,?3,NULL,?4,?5)",
            params![
                event_id.as_str(),
                ledger.event_type,
                format!("{}:{}", ledger.event_type, ledger.subject_id),
                ledger.payload,
                now
            ],
        )
        .map_err(storage)?;
    Ok(())
}

/// 状态词表的 SQL 字面量：调用方只给枚举，不写字符串。
fn status_list(statuses: &[ExecutionStatus]) -> String {
    statuses
        .iter()
        .map(|status| format!("'{}'", status_code(*status)))
        .collect::<Vec<_>>()
        .join(",")
}

impl InteractionSettlementStore for SqliteRuntimeStore {
    fn in_flight_turn_sessions(
        &self,
        limit: u32,
    ) -> Result<Vec<DaemonSessionId>, InteractionStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(settlement_error)?;
        let connection = self.connection().map_err(settlement_error)?;
        let mut statement = connection
            .prepare(
                "SELECT i.session_id FROM work_items AS w \
                 JOIN inbox AS i ON i.interaction_id=w.inbox_id \
                 WHERE w.status='running' \
                 GROUP BY i.session_id \
                 ORDER BY MIN(i.received_at_ms) ASC, i.session_id ASC \
                 LIMIT ?1",
            )
            .map_err(settlement_error)?;
        let rows = statement
            .query_map(params![i64::from(limit)], |row| row.get::<_, String>(0))
            .map_err(settlement_error)?;
        let mut sessions = Vec::new();
        for row in rows {
            let raw_session_id = row.map_err(settlement_error)?;
            let session_id = DaemonSessionId::new(raw_session_id.clone()).map_err(|_| {
                InteractionStoreError::Storage(format!(
                    "inbox.session_id 无法识别：{raw_session_id}"
                ))
            })?;
            sessions.push(session_id);
        }
        Ok(sessions)
    }

    fn in_flight_turns(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Vec<InFlightTurnFacts>, InteractionStoreError> {
        self.initialize().map_err(settlement_error)?;
        let connection = self.connection().map_err(settlement_error)?;
        let mut statement = connection
            .prepare(
                "SELECT w.inbox_id,w.started_at_ms,w.lease_expires_at_ms,\
                 COALESCE(s.last_activity_at_ms,0) \
                 FROM work_items AS w \
                 JOIN inbox AS i ON i.interaction_id=w.inbox_id \
                 LEFT JOIN sessions AS s ON s.session_id=i.session_id \
                 WHERE i.session_id=?1 AND w.status='running' \
                 ORDER BY i.received_at_ms ASC, w.inbox_id ASC",
            )
            .map_err(settlement_error)?;
        let rows = statement
            .query_map(params![session_id.as_str()], |row| {
                let raw_interaction_id: String = row.get(0)?;
                Ok((
                    raw_interaction_id,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(settlement_error)?;
        let mut turns = Vec::new();
        for row in rows {
            let (raw_interaction_id, started_at_ms, lease_expires_at_ms, last_activity_at_ms) =
                row.map_err(settlement_error)?;
            let interaction_id = InteractionId::new(raw_interaction_id.clone()).map_err(|_| {
                InteractionStoreError::Storage(format!(
                    "inbox.interaction_id 无法识别：{raw_interaction_id}"
                ))
            })?;
            turns.push(InFlightTurnFacts {
                interaction_id,
                started_at_ms: started_at_ms.and_then(optional_millis),
                lease_expires_at_ms: lease_expires_at_ms.and_then(optional_millis),
                session_last_activity_at_ms: u64::try_from(last_activity_at_ms).unwrap_or(0),
            });
        }
        Ok(turns)
    }

    fn settle_in_flight_turn(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
    ) -> Result<bool, InteractionStoreError> {
        self.initialize().map_err(settlement_error)?;
        let mut connection = self.connection().map_err(settlement_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(settlement_error)?;
        let now = i64_value(now_ms).map_err(settlement_error)?;
        let payload =
            super::interrupted_interaction_repository::interrupted_payload(interaction_id);
        let settlement = Settlement::concluded(ExecutionStatus::Failed, TERMINAL_INTERRUPTED)
            .with_ledger(LedgerTrace {
                event_type: super::interrupted_interaction_repository::LEDGER_EVENT_TYPE,
                subject_id: interaction_id.as_str(),
                payload: &payload,
            });
        let settled = settle_work_item(
            &transaction,
            WorkItemLocator::RunningInbox(interaction_id.as_str()),
            &settlement,
            now,
        )
        .map_err(settlement_error)?;
        transaction.commit().map_err(settlement_error)?;
        Ok(settled)
    }

    fn queued_turn_count(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<u64, InteractionStoreError> {
        self.initialize().map_err(settlement_error)?;
        let connection = self.connection().map_err(settlement_error)?;
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM work_items AS w \
                 JOIN inbox AS i ON i.interaction_id=w.inbox_id \
                 WHERE i.session_id=?1 AND i.terminal_kind IS NULL AND w.status=?2",
                params![session_id.as_str(), status_code(ExecutionStatus::Queued)],
                |row| row.get(0),
            )
            .map_err(settlement_error)?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    fn stale_queued_turns(
        &self,
        received_before_ms: u64,
        limit: u32,
    ) -> Result<Vec<QueuedTurnFacts>, InteractionStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(settlement_error)?;
        let connection = self.connection().map_err(settlement_error)?;
        let mut statement = connection
            .prepare(
                "SELECT w.inbox_id,i.received_at_ms FROM work_items AS w \
                 JOIN inbox AS i ON i.interaction_id=w.inbox_id \
                 LEFT JOIN sessions AS s ON s.session_id=i.session_id \
                 WHERE w.status=?1 AND i.terminal_kind IS NULL \
                   AND w.cancellation_requested_at_ms IS NULL \
                   AND COALESCE(s.kind,'primary') IN ('primary','cron','goal') \
                   AND i.received_at_ms<=?2 \
                 ORDER BY i.received_at_ms ASC, w.inbox_id ASC LIMIT ?3",
            )
            .map_err(settlement_error)?;
        let rows = statement
            .query_map(
                params![
                    status_code(ExecutionStatus::Queued),
                    i64_value(received_before_ms).map_err(settlement_error)?,
                    i64::from(limit)
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(settlement_error)?;
        let mut turns = Vec::new();
        for row in rows {
            let (raw_interaction_id, received_at_ms) = row.map_err(settlement_error)?;
            let interaction_id = InteractionId::new(raw_interaction_id.clone()).map_err(|_| {
                InteractionStoreError::Storage(format!(
                    "inbox.interaction_id 无法识别：{raw_interaction_id}"
                ))
            })?;
            turns.push(QueuedTurnFacts {
                interaction_id,
                received_at_ms: u64::try_from(received_at_ms).unwrap_or(0),
            });
        }
        Ok(turns)
    }

    fn settle_queued_turn(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
    ) -> Result<bool, InteractionStoreError> {
        self.initialize().map_err(settlement_error)?;
        let mut connection = self.connection().map_err(settlement_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(settlement_error)?;
        let now = i64_value(now_ms).map_err(settlement_error)?;
        let payload =
            super::interrupted_interaction_repository::interrupted_payload(interaction_id);
        let settlement = Settlement::concluded(ExecutionStatus::Failed, TERMINAL_INTERRUPTED)
            .with_ledger(LedgerTrace {
                event_type: super::interrupted_interaction_repository::LEDGER_EVENT_TYPE,
                subject_id: interaction_id.as_str(),
                payload: &payload,
            });
        let settled = settle_work_item(
            &transaction,
            WorkItemLocator::QueuedInbox(interaction_id.as_str()),
            &settlement,
            now,
        )
        .map_err(settlement_error)?;
        transaction.commit().map_err(settlement_error)?;
        Ok(settled)
    }
}

/// 非负毫秒列的可选读回：负数按「未知」处理，不冒充时刻。
fn optional_millis(value: i64) -> Option<u64> {
    u64::try_from(value).ok()
}

fn settlement_error(error: impl std::fmt::Display) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

#[cfg(test)]
#[path = "work_item_settlement_tests.rs"]
mod tests;
