//! 一轮对话的规格：装载端定下的来源口径与结果去向。
//!
//! 两个 turn 入口的全部差异收敛在这里：按哪套口径装载与收口（`profile`）、
//! 结果去哪里（`completion`）、以及这一轮的输入事实由谁落（`input`）。
//! 执行段不区分来源，只认请求本身。

use axon_session_runtime::{TurnOrigin, TurnProfile};

/// 一轮对话的规格。
pub(crate) struct TurnSpec {
    pub(crate) profile: TurnProfile,
    pub(crate) completion: TurnCompletion,
    pub(crate) input: TurnInput,
}

impl TurnSpec {
    /// 用户可见主 turn 的规格；只在测试里用，测试不该重复拼口径。
    #[cfg(test)]
    pub(crate) fn primary() -> Self {
        Self::for_origin(TurnOrigin::Primary, TurnInput::Submitted)
    }

    /// 模型子代理：派生上下文，结果交给它自己的执行会话，输入事实由运行侧落。
    pub(crate) fn subagent() -> Self {
        Self {
            profile: TurnProfile::for_subagent(),
            completion: TurnCompletion::ChildSession,
            input: TurnInput::Generated {
                channel: axon_domain::AGENT_DISPATCH_SOURCE_CHANNEL,
                client: None,
            },
        }
    }

    /// 按来源取规格：装载与收口口径来自 `TurnProfile`，结果去向只按有没有父轮区分。
    pub(crate) fn for_origin(origin: TurnOrigin, input: TurnInput) -> Self {
        Self {
            profile: TurnProfile::for_origin(origin),
            completion: TurnCompletion::Surface,
            input,
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

/// 这一轮的输入事实由谁落。
///
/// 用户入站与 cron 唤醒在到达时就已经写过 `inbox`；子代理派发与目标执行没有外部
/// 入站，但每个会话都要有完整的输入事实，否则前端只能给它们另写「没有首条消息」
/// 的渲染分支，因此由运行侧在起跑时补一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnInput {
    /// 入站事实已经存在，运行侧不再写。
    Submitted,
    /// 运行侧自己派发的轮次：输入内容就是本轮交给模型的完整提示。
    Generated {
        /// 落库渠道；执行侧派发统一走 `agent`，不与任何用户渠道混用。
        channel: &'static str,
        /// 归因客户端：`origin_channel` 取它优先，因此目标执行保留 `goal` 归因。
        client: Option<&'static str>,
    },
}

impl TurnInput {
    /// 运行侧是否需要为本轮补写输入事实。
    pub(crate) const fn generated(self) -> Option<(&'static str, Option<&'static str>)> {
        match self {
            Self::Submitted => None,
            Self::Generated { channel, client } => Some((channel, client)),
        }
    }
}
