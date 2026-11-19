//! 一次 turn 的口径：来源、上下文装载与终态收口责任人。
//!
//! 所有 turn 入口（主 turn、目标执行、后续的 cron 与子代理）共用同一条会话路径，差异只允许
//! 表达在本模块的结构里，禁止再靠「另一条管线」承载。取值全部来自持久 Action 与 transport
//! 上下文，可重放、可断言。
//!
//! 接线进度：B0 只引入口径与派生，尚未被装配层消费；装配、记账与投影各自在后续批次读取本结构。

use crate::application::ProviderTurnRequest;

/// 用户可见主 turn 的持久 Action 类型。
///
/// 主 turn 与会话路径的第一判据就是这个值：定义放在会话运行时一侧，daemon 与 CLI 引用同一处，
/// 不允许各自再写一份字面量。
pub const PRIMARY_TURN_ACTION_KIND: &str = "user_input";

/// transport 上下文里标识本轮来源的键；与目标执行装配侧写入的键同源。
pub const ORIGIN_CHANNEL_KEY: &str = "originChannel";
/// 目标执行的来源标记。
pub const ORIGIN_CHANNEL_GOAL: &str = "goal";

/// 本轮 turn 的来源。
///
/// 只区分真实存在的来源：用户可见的主 turn、目标执行、模型子代理，以及其余后台执行。
/// 没有来源标记的一律落到 `Background`，不留没有生产者的分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOrigin {
    /// 用户可见的主 turn：交互、TUI、桌面与 webhook 入站都走这里。
    Primary,
    /// 目标执行：由目标账本派发，来源标记为 `goal`。
    GoalExecution,
    /// 模型子代理：由主 turn 的派发工具创建，在来源会话下拥有自己的执行会话。
    Subagent,
    /// 其余后台执行：当前没有独立来源标记的后台 turn。
    Background,
}

impl TurnOrigin {
    /// 按持久 Action 类型与 transport 上下文判定来源。
    ///
    /// 判据只看两个既有事实：Action 类型是否为 `user_input`，以及 transport 是否打了目标标记。
    /// 不做任何字符串前缀拼接或启发式推断。
    #[must_use]
    pub fn from_action(action_kind: &str, transport_context: Option<&serde_json::Value>) -> Self {
        if action_kind == PRIMARY_TURN_ACTION_KIND {
            return Self::Primary;
        }
        let origin_channel = transport_context
            .and_then(|context| context.get(ORIGIN_CHANNEL_KEY))
            .and_then(serde_json::Value::as_str);
        if origin_channel == Some(ORIGIN_CHANNEL_GOAL) {
            return Self::GoalExecution;
        }
        Self::Background
    }
}

/// 上下文装载口径：与主会话相比，本轮要多装或少装哪几段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnContextProfile {
    /// 是否装载用户级 Soul。
    pub user_soul: bool,
    /// 是否装载个人记忆。
    pub personal_memory: bool,
    /// 是否装载委派目录（可派发的子代理选型事实）。
    pub delegation_catalog: bool,
    /// 是否允许继承父轮历史（无既有会话时作为起点）。
    pub fork_history: bool,
}

impl TurnContextProfile {
    /// 主 turn：三段全装，历史由会话本身提供。
    #[must_use]
    pub const fn primary() -> Self {
        Self {
            user_soul: true,
            personal_memory: true,
            delegation_catalog: true,
            fork_history: false,
        }
    }

    /// 派生 turn：人格与个人记忆由主 turn 决定，执行侧只接被委托的工作。
    #[must_use]
    pub const fn child() -> Self {
        Self {
            user_soul: false,
            personal_memory: false,
            delegation_catalog: false,
            fork_history: true,
        }
    }
}

/// 失败终态的收口责任人。
///
/// 一轮只能有一个 owner：两侧同时写会让同一轮出现两张失败卡。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalFailureOwner {
    /// 运行侧收口：失败事实随本轮结果一并落库。
    Runtime,
    /// 调度侧收口：由派发方结算失败并排重试，运行侧不得再写。
    Scheduler,
}

/// 终态收口口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalAccounting {
    /// 是否写会话级轮次指标。所有来源都必须写：指标缺行是投影缺口，不是来源差异。
    pub metrics: bool,
    /// 是否派生并投影会话标题。
    pub title: bool,
    /// 是否触发轮后治理（记忆反思与自迭代）。
    pub governance: bool,
    /// 失败终态的责任人。
    pub failure_owner: TerminalFailureOwner,
}

impl TerminalAccounting {
    /// 主 turn：三项全开，失败由运行侧收口。
    #[must_use]
    pub const fn primary() -> Self {
        Self {
            metrics: true,
            title: true,
            governance: true,
            failure_owner: TerminalFailureOwner::Runtime,
        }
    }

    /// 后台执行：指标照写；标题与轮后治理属于主 turn 的决策权，不随后台执行触发。
    ///
    /// 失败终态由调度侧结算：重试、幂等与失败卡都在派发方，运行侧再写一张会让同一轮出现两张。
    #[must_use]
    pub const fn background() -> Self {
        Self {
            metrics: true,
            title: false,
            governance: false,
            failure_owner: TerminalFailureOwner::Scheduler,
        }
    }

    /// 模型子代理：指标照写；标题与轮后治理不随子代理触发。
    ///
    /// 子代理由父轮内联等待或异步回写完成，失败事实要落进它自己的会话，父轮拿到的是一份
    /// 失败回执而不是调度重试，因此失败由运行侧收口。
    #[must_use]
    pub const fn subagent() -> Self {
        Self {
            metrics: true,
            title: false,
            governance: false,
            failure_owner: TerminalFailureOwner::Runtime,
        }
    }
}

/// 一次 turn 的完整口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnProfile {
    /// 本轮来源。
    pub origin: TurnOrigin,
    /// 上下文装载口径。
    pub context: TurnContextProfile,
    /// 终态收口口径。
    pub accounting: TerminalAccounting,
}

impl TurnProfile {
    /// 按来源取口径：主 turn 全量，其余一律按派生 turn 处理。
    #[must_use]
    pub const fn for_origin(origin: TurnOrigin) -> Self {
        match origin {
            TurnOrigin::Primary => Self {
                origin,
                context: TurnContextProfile::primary(),
                accounting: TerminalAccounting::primary(),
            },
            // 目标执行是「不展示在会话列表里的主会话」：装载与收口都按主会话走，
            // 它与会话清单的关系由 `kind` 决定，不由口径决定。
            TurnOrigin::GoalExecution => Self {
                origin,
                context: TurnContextProfile::primary(),
                accounting: TerminalAccounting::primary(),
            },
            TurnOrigin::Background => Self {
                origin,
                context: TurnContextProfile::child(),
                accounting: TerminalAccounting::background(),
            },
            TurnOrigin::Subagent => Self {
                origin,
                context: TurnContextProfile::child(),
                accounting: TerminalAccounting::subagent(),
            },
        }
    }

    /// 模型子代理口径：与后台执行共用上下文装载，但失败由运行侧收口。
    #[must_use]
    pub const fn for_subagent() -> Self {
        Self::for_origin(TurnOrigin::Subagent)
    }

    /// 从规范化请求派生口径。
    #[must_use]
    pub fn for_request(request: &ProviderTurnRequest) -> Self {
        Self::for_origin(TurnOrigin::from_action(
            &request.action_kind,
            request.transport_context.as_ref(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action_kind: &str, transport: Option<serde_json::Value>) -> ProviderTurnRequest {
        ProviderTurnRequest {
            session_id: "session-1".to_owned(),
            action_kind: action_kind.to_owned(),
            prompt: "执行".to_owned(),
            quoted_message: None,
            cwd: ".".to_owned(),
            run_session_id: "run-1".to_owned(),
            runtime_profile: crate::RuntimeProfile::Primary,
            transport_context: transport,
        }
    }

    #[test]
    fn primary_action_gets_primary_profile() {
        let profile = TurnProfile::for_request(&request("user_input", None));
        assert_eq!(profile.origin, TurnOrigin::Primary);
        assert_eq!(profile.context, TurnContextProfile::primary());
        assert_eq!(
            profile.accounting.failure_owner,
            TerminalFailureOwner::Runtime
        );
        assert!(profile.accounting.metrics);
        assert!(profile.accounting.title);
        assert!(profile.accounting.governance);
    }

    #[test]
    fn goal_execution_action_gets_primary_profile() {
        let transport = serde_json::json!({ ORIGIN_CHANNEL_KEY: ORIGIN_CHANNEL_GOAL });
        let profile = TurnProfile::for_request(&request("turn", Some(transport)));
        assert_eq!(profile.origin, TurnOrigin::GoalExecution);
        // 目标执行是「不展示在会话列表里的主会话」：装载与收口都按主会话走，
        // 它与会话清单的关系由 `kind` 决定，不由口径决定。
        assert_eq!(profile.context, TurnContextProfile::primary());
        assert_eq!(profile.accounting, TerminalAccounting::primary());
        assert_eq!(
            profile.accounting.failure_owner,
            TerminalFailureOwner::Runtime
        );
    }

    #[test]
    fn background_action_without_goal_marker_falls_back_to_background() {
        let transport = serde_json::json!({ ORIGIN_CHANNEL_KEY: "cron" });
        let profile = TurnProfile::for_request(&request("turn", Some(transport)));
        assert_eq!(profile.origin, TurnOrigin::Background);
        assert_eq!(profile.context, TurnContextProfile::child());
    }

    #[test]
    fn subagent_profile_owns_failure_in_runtime() {
        let profile = TurnProfile::for_subagent();
        assert_eq!(profile.origin, TurnOrigin::Subagent);
        assert_eq!(profile.context, TurnContextProfile::child());
        assert!(profile.accounting.metrics);
        assert!(!profile.accounting.title);
        assert!(!profile.accounting.governance);
        // 子代理在父轮内联或异步回写完成：失败必须由运行侧落进它自己的会话，
        // 不能像后台执行那样交给调度侧重试。
        assert_eq!(
            profile.accounting.failure_owner,
            TerminalFailureOwner::Runtime
        );
    }

    #[test]
    fn origin_is_decided_by_action_kind_then_marker_only() {
        // 主 turn 即使携带目标标记也不改变来源：Action 类型是唯一的第一判据。
        let transport = serde_json::json!({ ORIGIN_CHANNEL_KEY: ORIGIN_CHANNEL_GOAL });
        assert_eq!(
            TurnOrigin::from_action("user_input", Some(&transport)),
            TurnOrigin::Primary
        );
        // 非字符串或缺失的标记不得推断成目标执行。
        assert_eq!(
            TurnOrigin::from_action("turn", Some(&serde_json::json!({ ORIGIN_CHANNEL_KEY: 1 }))),
            TurnOrigin::Background
        );
        assert_eq!(
            TurnOrigin::from_action("turn", None),
            TurnOrigin::Background
        );
    }
}
