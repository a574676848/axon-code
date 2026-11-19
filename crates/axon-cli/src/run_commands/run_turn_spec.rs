//! 一轮对话的规格：装载端定下的来源口径与结果去向。
//!
//! 两个 turn 入口的全部差异收敛在这里，且只有两处：按哪套口径装载与收口（`profile`），
//! 以及结果去哪里（`completion`）。执行段不区分来源，只认请求本身。

use axon_session_runtime::{TurnOrigin, TurnProfile};

/// 一轮对话的规格。
pub(crate) struct TurnSpec {
    pub(crate) profile: TurnProfile,
    pub(crate) completion: TurnCompletion,
}

impl TurnSpec {
    /// 用户可见的主 turn：上下文全量装载，结果回到调用方表面。
    pub(crate) fn primary() -> Self {
        Self {
            profile: TurnProfile::for_origin(TurnOrigin::Primary),
            completion: TurnCompletion::Surface,
        }
    }

    /// 模型子代理：派生上下文，结果交给它自己的执行会话，父轮经回执看到。
    pub(crate) fn subagent() -> Self {
        Self {
            profile: TurnProfile::for_subagent(),
            completion: TurnCompletion::ChildSession,
        }
    }
}

/// 一轮的结果去哪里。
///
/// 这是运行后唯一的差异点：回到调用方表面的一轮要自己承担人格结算与记忆反馈，
/// 交给执行子会话的一轮不重复做这些决定，只把事实落进自己的会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnCompletion {
    /// 回传调用方表面。
    Surface,
    /// 交给执行子会话；父轮通过与普通会话一致的逐字稿与回执看到结果。
    ChildSession,
}
