//! 在途轮次的判死读取与收口端口。
//!
//! 「界面说在跑、服务端说已结束」的僵持来自「非终态但没有执行者」的轮次：执行进程
//! 消失、轮次租约过期、provider 长时间不吐首字，都会留下一条仍在 `processing` 的
//! 轮次，而没有任何人会再去写它的终态。本端口只做两件事：读出在途轮次与它的判死
//! 事实，以及按轮次收口。判据留在应用层，写终态的路径留在存储层的终态唯一出口。

use super::InteractionStoreError;
use axon_domain::{DaemonSessionId, InteractionId};

/// 一条在途轮次的判死事实。
///
/// 三个时刻是判定的全部输入：领取时刻回答「这一轮跑了多久」，会话最后活动时刻回答
/// 「它最近有没有写过事件」，租约到期时刻回答「还有没有执行者」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightTurnFacts {
    /// 轮次标识。
    pub interaction_id: InteractionId,
    /// 本轮领取时刻；工作项没有记录开始时间时为空。
    pub started_at_ms: Option<u64>,
    /// 租约到期时刻；没有租约时为空。
    pub lease_expires_at_ms: Option<u64>,
    /// 所属会话最后一次写入事件的时刻；从未写过事件时为 0。
    pub session_last_activity_at_ms: u64,
}

/// 一条排队轮次的等待事实。
///
/// 排队是刚提交的正常中间态：消息已经落库，主 turn 还没领取。它与「在途但已死」是
/// 两件事，读侧必须能分开，否则刚发出消息就会被读成「没有执行者」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedTurnFacts {
    /// 轮次标识。
    pub interaction_id: InteractionId,
    /// 入站事实被接受的时刻：等待时长以它起算，不用每次释放的更新时间。
    pub received_at_ms: u64,
}

/// 在途轮次的收口端口。
pub trait InteractionSettlementStore: Send + Sync {
    /// 有在途轮次的会话，按各自最早一条在途轮次的接收时间排序，至多 `limit` 条。
    ///
    /// 周期维护按会话逐条判死：判据是会话级的最后活动时间，因此必须先知道「哪些会话
    /// 还有在途轮次」，否则只能全表扫会话或漏掉真在跑的轮次。只读，不改任何事实。
    fn in_flight_turn_sessions(
        &self,
        limit: u32,
    ) -> Result<Vec<DaemonSessionId>, InteractionStoreError>;

    /// 读该会话当前仍在跑的轮次及其判死事实。
    ///
    /// 只读：判定「该不该收口」由应用层做，这里不改任何事实。
    fn in_flight_turns(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Vec<InFlightTurnFacts>, InteractionStoreError>;

    /// 把一条仍在跑的轮次收口为 `interrupted`。
    ///
    /// 走存储层的终态唯一出口：改状态、写终态类别、归档、追流水账一次完成，
    /// 因此被动隔离与显式收口产出的是同一种终态。
    /// 返回本次是否真的收口：已被其他事务收口的轮次返回 `false`，不重复写终态。
    fn settle_in_flight_turn(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
    ) -> Result<bool, InteractionStoreError>;

    /// 该会话里「已被接受、但还没有任何执行者」的轮次数。
    ///
    /// 这一类轮次是刚提交的正常中间态：入站事实已经落库，主 turn 还没领取。读侧据此
    /// 区分「还没开始」与「已经没有执行者」，不得把它读成运行证据过期。
    fn queued_turn_count(&self, session_id: &DaemonSessionId)
    -> Result<u64, InteractionStoreError>;

    /// 排队超过期限、仍未被任何执行者领取的轮次，按接收时间升序、有界返回。
    ///
    /// 只扫主 turn 驱动的会话（`primary` / `cron` / `goal`）：子代理的入站事实只作记录，
    /// 收口它会抹掉父轮的派发痕迹。
    fn stale_queued_turns(
        &self,
        received_before_ms: u64,
        limit: u32,
    ) -> Result<Vec<QueuedTurnFacts>, InteractionStoreError>;

    /// 收口一条从未开始执行的排队轮次。
    ///
    /// 守卫只有「仍停在排队态」：已被领取的行一定有人持有租约，收口它等于从活着的
    /// 执行者手里抢走终态。返回本次是否真的收到了一条。
    fn settle_queued_turn(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
    ) -> Result<bool, InteractionStoreError>;
}
