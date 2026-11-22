//! 静默轮次的判死与显式收口：WP-04 的服务端一半。
//!
//! 「界面说在跑、服务端说已结束」的僵持，根因是库里留着一批「非终态但没有执行者」的
//! 轮次：执行进程消失后租约还在，或者 provider 接上了却迟迟不吐首字。这类轮次既不会
//! 自己终结，也没有任何人会再去写它的终态，于是订阅方永远等不到收口帧。
//!
//! 本用例负责三件事，且只做应用层编排：
//!
//! 1. 判死：[`SilentTurnSettlementService::inspect`] 按会话读出在途轮次与它的租约、
//!    活动事实，给出「收口 / 保持」的结论。只读，不改任何事实。
//! 2. 收口：[`SilentTurnSettlementService::settle`] 与
//!    [`SilentTurnSettlementService::settle_turn`] 把判死的轮次收敛为 `interrupted`，
//!    走存储层的终态唯一出口，因此与被动隔离产出同一种终态、同一条流水账事件。
//! 3. 周期收口：[`SilentTurnSettlementService::settle_pending`] 供 supervisor tick
//!    按会话批量执行第 2 步，保证「没有执行者却还在跑」的轮次不会被漏掉。
//!
//! 输入输出契约固定为 [`SettleSilentTurnRequest`]、[`SettleSilentTurnReceipt`] 与
//! [`SilentTurnSettlementOutcome`]：HTTP 路由由网关侧接入，本模块不关心传输层，
//! 也不做纯前端状态改写。

use axon_domain::{DaemonSessionId, InteractionId};
use axon_session::{InFlightTurnFacts, InteractionSettlementStore, InteractionStoreError};
use thiserror::Error;

/// 默认首字期限：租约仍有效、但领取之后一直没有事件帧超过这么久，本轮判死。
///
/// 取两分钟：正常的工具调用与思考都会写事件帧，两分钟没有任何帧只可能是 provider
/// 静默或执行者已被杀但没有释放租约。
pub const DEFAULT_FIRST_TOKEN_DEADLINE_MS: u64 = 120_000;

/// 一条在途轮次为什么可以收口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentTurnReason {
    /// 租约已过期：负责这一轮的执行者已经不在了。
    LeaseExpired,
    /// 超过首字期限仍没有任何事件帧：provider 静默，再等也不会产出。
    FirstTokenDeadlineExceeded,
}

impl SilentTurnReason {
    /// 判死原因的稳定取值：HTTP 回执与界面文案共用同一套词。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LeaseExpired => "lease_expired",
            Self::FirstTokenDeadlineExceeded => "first_token_deadline_exceeded",
        }
    }
}

/// 一条在途轮次的判死结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightTurnVerdict {
    /// 轮次标识。
    pub interaction_id: InteractionId,
    /// 租约到期时刻；没有租约事实时为空。
    pub lease_expires_at_ms: Option<u64>,
    /// 最后一次可观测活动的时刻：会话最后事件时间与领取时间取较大者。
    pub silent_since_ms: u64,
    /// 判死原因；为空表示仍有执行者，不得收口。
    pub reason: Option<SilentTurnReason>,
}

impl InFlightTurnVerdict {
    /// 这一轮是否可以由服务端收口。
    #[must_use]
    pub const fn is_settleable(&self) -> bool {
        self.reason.is_some()
    }
}

/// 判死快照：只读回执，给界面与网关判断「要不要调收口」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SilentTurnSnapshot {
    /// 被检查的会话。
    pub session_id: DaemonSessionId,
    /// 观测时刻。
    pub observed_at_ms: u64,
    /// 该会话当前在途的轮次，按接收顺序排列。
    pub turns: Vec<InFlightTurnVerdict>,
}

impl SilentTurnSnapshot {
    /// 可以收口的轮次。
    pub fn settle_offers(&self) -> impl Iterator<Item = &InFlightTurnVerdict> {
        self.turns.iter().filter(|turn| turn.is_settleable())
    }
}

/// 显式收口请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleSilentTurnRequest {
    /// 收口哪个会话的在途轮次。
    pub session_id: DaemonSessionId,
    /// 请求发生在哪一刻；判定与落库用同一个时刻。
    pub now_ms: u64,
    /// 首字期限；`None` 用 [`DEFAULT_FIRST_TOKEN_DEADLINE_MS`]。
    pub first_token_deadline_ms: Option<u64>,
}

impl SettleSilentTurnRequest {
    /// 以默认首字期限构造请求。
    #[must_use]
    pub fn new(session_id: DaemonSessionId, now_ms: u64) -> Self {
        Self {
            session_id,
            now_ms,
            first_token_deadline_ms: None,
        }
    }

    /// 覆盖首字期限。
    #[must_use]
    pub const fn with_first_token_deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.first_token_deadline_ms = Some(deadline_ms);
        self
    }

    /// 本次生效的首字期限。
    fn deadline_ms(&self) -> Result<u64, SilentTurnSettlementError> {
        match self.first_token_deadline_ms {
            None => Ok(DEFAULT_FIRST_TOKEN_DEADLINE_MS),
            Some(0) => Err(SilentTurnSettlementError::InvalidDeadline),
            Some(deadline_ms) => Ok(deadline_ms),
        }
    }
}

/// 显式收口回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleSilentTurnReceipt {
    /// 被收口的会话。
    pub session_id: DaemonSessionId,
    /// 本次真正收口为 `interrupted` 的轮次。
    pub settled: Vec<InteractionId>,
    /// 判定仍有执行者、按约定不动它的轮次。
    pub kept_running: Vec<InteractionId>,
}

/// 单条轮次的显式收口结论。
///
/// 判据取值同时是 HTTP 回执里的 `verdict`：界面据此区分「已收口」「还有人持有租约」
/// 「这条轮次根本不在途」三种情况，而不是把它们统一成一个失败提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SilentTurnSettlementOutcome {
    /// 本轮已收口为 `interrupted`，携带判死原因。
    Settled(SilentTurnReason),
    /// 已有其他收口路径先一步改到这行：本次什么都没写。
    AlreadySettled,
    /// 仍在途，但判据显示还有执行者：按约定不得收口。
    StillRunning,
    /// 该会话当前没有这条在途轮次（已收口，或从未领取）。
    NotInFlight,
}

impl SilentTurnSettlementOutcome {
    /// 回执里的稳定判据取值。
    #[must_use]
    pub const fn verdict(&self) -> &'static str {
        match self {
            Self::Settled(reason) => reason.as_str(),
            Self::AlreadySettled => "already_settled",
            Self::StillRunning => "still_running",
            Self::NotInFlight => "no_in_flight_turn",
        }
    }

    /// 本次是否真的写了终态。
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        matches!(self, Self::Settled(_))
    }
}

/// 一轮维护周期批量收口的统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SilentTurnBatchReport {
    /// 本轮检查过的会话数。
    pub sessions_checked: u32,
    /// 本轮真正收口为 `interrupted` 的轮次数。
    pub turns_settled: u32,
    /// 判定仍有执行者、按约定不动的轮次数。
    pub turns_kept: u32,
}

/// 静默轮次判定与收口失败。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SilentTurnSettlementError {
    /// 在途轮次事实读写失败。
    #[error(transparent)]
    Store(#[from] InteractionStoreError),
    /// 首字期限必须大于 0：0 等于「领取即判死」，不是可用的期限。
    #[error("首字期限必须大于 0")]
    InvalidDeadline,
}

/// 静默轮次的判死与收口服务。
#[derive(Debug, Clone)]
pub struct SilentTurnSettlementService<S> {
    store: S,
}

impl<S> SilentTurnSettlementService<S>
where
    S: InteractionSettlementStore,
{
    /// 用给定存储构造服务。
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    /// 只读判死：读出该会话在途轮次并给出「收口 / 保持」的结论。
    pub fn inspect(
        &self,
        session_id: &DaemonSessionId,
        now_ms: u64,
        first_token_deadline_ms: Option<u64>,
    ) -> Result<SilentTurnSnapshot, SilentTurnSettlementError> {
        let deadline_ms = match first_token_deadline_ms {
            None => DEFAULT_FIRST_TOKEN_DEADLINE_MS,
            Some(0) => return Err(SilentTurnSettlementError::InvalidDeadline),
            Some(deadline_ms) => deadline_ms,
        };
        let turns = self
            .store
            .in_flight_turns(session_id)?
            .iter()
            .map(|facts| judge(facts, now_ms, deadline_ms))
            .collect();
        Ok(SilentTurnSnapshot {
            session_id: session_id.clone(),
            observed_at_ms: now_ms,
            turns,
        })
    }

    /// 收口判死的轮次：仍有执行者的轮次原样保留，并在回执里列明。
    pub fn settle(
        &self,
        request: &SettleSilentTurnRequest,
    ) -> Result<SettleSilentTurnReceipt, SilentTurnSettlementError> {
        request.deadline_ms()?;
        let snapshot = self.inspect(
            &request.session_id,
            request.now_ms,
            request.first_token_deadline_ms,
        )?;
        let mut settled = Vec::new();
        let mut kept_running = Vec::new();
        for verdict in snapshot.turns {
            if !verdict.is_settleable() {
                kept_running.push(verdict.interaction_id);
                continue;
            }
            // 收口走存储层的终态唯一出口；并发的两个收口只有一个能改到行。
            if self
                .store
                .settle_in_flight_turn(&verdict.interaction_id, request.now_ms)?
            {
                settled.push(verdict.interaction_id);
            }
        }
        Ok(SettleSilentTurnReceipt {
            session_id: request.session_id.clone(),
            settled,
            kept_running,
        })
    }

    /// 一轮维护周期的批量收口：按会话枚举在途轮次，逐条判死并收口。
    ///
    /// supervisor tick 调用它，因此必须是有界的：会话数由 `limit` 封顶，轮次数由各会话
    /// 自己的在途轮次数封顶。判据与显式收口完全一致，两条路径因此产出同一种终态。
    ///
    /// 存储层失败整批放弃：同一轮里后续会话的判读建立在同一条连接池与同一份事务语义上，
    /// 半批成功只会让「谁被收口了」变得不可复述；失败由下一轮重试。
    pub fn settle_pending(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<SilentTurnBatchReport, SilentTurnSettlementError> {
        let mut report = SilentTurnBatchReport::default();
        for session_id in self.store.in_flight_turn_sessions(limit)? {
            report.sessions_checked = report.sessions_checked.saturating_add(1);
            let snapshot = self.inspect(&session_id, now_ms, None)?;
            for verdict in snapshot.turns {
                if !verdict.is_settleable() {
                    report.turns_kept = report.turns_kept.saturating_add(1);
                    continue;
                }
                // 收口走存储层的终态唯一出口；并发收口只有一个能改到行。
                if self
                    .store
                    .settle_in_flight_turn(&verdict.interaction_id, now_ms)?
                {
                    report.turns_settled = report.turns_settled.saturating_add(1);
                }
            }
        }
        Ok(report)
    }

    /// 显式收口一条指定轮次：桌面端按下 ESC 之后的服务端出口。
    ///
    /// 只收口判据成立的轮次：仍有执行者的轮次原样保留并如实给出 `still_running`，
    /// 不存在的轮次给出 `no_in_flight_turn`。重复调用是幂等的：第二次已经没有在途轮次。
    pub fn settle_turn(
        &self,
        request: &SettleSilentTurnRequest,
        interaction_id: &InteractionId,
    ) -> Result<SilentTurnSettlementOutcome, SilentTurnSettlementError> {
        let snapshot = self.inspect(
            &request.session_id,
            request.now_ms,
            request.first_token_deadline_ms,
        )?;
        let Some(verdict) = snapshot
            .turns
            .iter()
            .find(|turn| &turn.interaction_id == interaction_id)
        else {
            return Ok(SilentTurnSettlementOutcome::NotInFlight);
        };
        let Some(reason) = verdict.reason else {
            return Ok(SilentTurnSettlementOutcome::StillRunning);
        };
        if self
            .store
            .settle_in_flight_turn(interaction_id, request.now_ms)?
        {
            Ok(SilentTurnSettlementOutcome::Settled(reason))
        } else {
            Ok(SilentTurnSettlementOutcome::AlreadySettled)
        }
    }
}

/// 判死一条在途轮次。
///
/// 判据只有两条，都来自库里的客观事实：
///
/// - 租约已过期或根本没有租约：没有执行者。
/// - 租约仍有效，但领取之后一个事件帧都没写过（会话最后活动时间不晚于领取时间），
///   且已经超过首字期限：provider 静默。
///
/// 已经写过帧的轮次即使很慢也不算静默：它在跑，只是慢，收口它会抹掉真实的在途工作。
fn judge(facts: &InFlightTurnFacts, now_ms: u64, deadline_ms: u64) -> InFlightTurnVerdict {
    let silent_since_ms = facts
        .session_last_activity_at_ms
        .max(facts.started_at_ms.unwrap_or(0));
    let reason = if facts
        .lease_expires_at_ms
        .is_none_or(|expires| expires <= now_ms)
    {
        Some(SilentTurnReason::LeaseExpired)
    } else {
        match facts.started_at_ms {
            Some(started_at_ms)
                if facts.session_last_activity_at_ms <= started_at_ms
                    && now_ms.saturating_sub(started_at_ms) >= deadline_ms =>
            {
                Some(SilentTurnReason::FirstTokenDeadlineExceeded)
            }
            _ => None,
        }
    };
    InFlightTurnVerdict {
        interaction_id: facts.interaction_id.clone(),
        lease_expires_at_ms: facts.lease_expires_at_ms,
        silent_since_ms,
        reason,
    }
}

#[cfg(test)]
#[path = "silent_turn_settlement_tests.rs"]
mod tests;
