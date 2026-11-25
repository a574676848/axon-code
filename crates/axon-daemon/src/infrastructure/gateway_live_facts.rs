//! 运行态在跑与排队轮次的定向读取。
//!
//! `/sessions/{id}/live` 原先以 `inbox` 为驱动表：先按会话把整个入站分区扫一遍，再逐行
//! 回表 `work_items` 过滤 `running`，排队计数同理。这条成本等于会话的消息数，而运行态
//! 事实本身只在 `work_items` 上：两态判定都只依赖 `status` 与租约列。
//!
//! 现在改由 `work_items_session_claim(session_id, status, ...)` 驱动：只取该会话
//! `running` 与 `queued` 两态的行，`inbox` 仅按主键回表补一个条件（派发轮已收口的
//! 排队行不算排队）。行数是在跑与排队轮次数，不是消息数。

use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore as _, RuntimeStoreError, SqliteRuntimeStore};

/// 一条会话的轮次事实：在跑轮次的租约上界与排队轮次数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveTurnFacts {
    /// 在跑轮次里最晚的租约到期时刻；没有在跑轮次时为空。
    ///
    /// 「在跑」由调用方按当前时刻判定：租约过期即没有执行者，本层不替它做时间比较，
    /// 免得判定时刻与写侧各取一次时间。
    pub running_lease_expires_at_ms: Option<u64>,
    /// 已被接受、还没有任何执行者领取的排队轮次数。
    pub queued_turns: u64,
}

/// 运行态轮次事实的读取端口。
///
/// 只有持有运行态库的实现能回答这两个数：它们不是领域推导，而是 `work_items` 上的
/// 两态计数。网关因此按具体存储实现取事实，不从通用端口绕行。
pub trait LiveTurnFactsStore {
    /// 读该会话此刻的在跑与排队轮次事实。
    fn live_turn_facts(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<LiveTurnFacts, RuntimeStoreError>;
}

impl LiveTurnFactsStore for SqliteRuntimeStore {
    fn live_turn_facts(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<LiveTurnFacts, RuntimeStoreError> {
        // 与既有读路径同口径：先确认 schema 版本，避免在旧库上读出不存在的列。
        self.initialize()?;
        self.with_read_connection(|connection| {
            // 一条语句同时给出两个数：`MAX(CASE ...)` 取在跑行的租约上界，
            // `SUM(CASE ...)` 数排队行。没有候选行时两列都是空，下面各自归一。
            let (lease_expires_at_ms, queued_turns) = connection.query_row(
                "SELECT MAX(CASE WHEN w.status = 'running' THEN w.lease_expires_at_ms END),
                        COALESCE(SUM(CASE WHEN w.status = 'queued' THEN 1 ELSE 0 END), 0)
                   FROM work_items AS w
                   JOIN inbox AS i ON i.interaction_id = w.inbox_id
                  WHERE w.session_id = ?1
                    AND i.session_id = ?1
                    AND w.status IN ('running','queued')
                    AND (w.status = 'running' OR i.terminal_kind IS NULL)",
                [session_id.as_str()],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
            )?;
            Ok(LiveTurnFacts {
                running_lease_expires_at_ms: lease_expires_at_ms
                    .and_then(|value| u64::try_from(value).ok()),
                queued_turns: u64::try_from(queued_turns).unwrap_or(0),
            })
        })
    }
}

#[cfg(test)]
#[path = "gateway_live_facts_equivalence_tests.rs"]
mod equivalence_tests;
