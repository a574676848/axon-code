//! 子代理执行面的工具收口。
//!
//! 主 turn 专属能力（计划、Goal、委派、提问）绝不能出现在子代理可见的工具表里，
//! 也不能只在执行时被"提示词劝住"。收敛点只有这一处：
//!
//! * payload 侧按 [`MAIN_TURN_ONLY_TOOL_QUALIFIED_NAMES`] 与 Goal 工具统一裁剪，
//!   保证**同类子代理**拿到逐字节相同的 `tools` 字段（唯一差异是再派发能力：
//!   root 路径的子代理看得到 `spawn_agent`，更深一层看不到，这条差异无法既保留
//!   分层派发又做到全域同形）；
//! * 执行侧同步从注册表里移除同一批工具，越权调用会得到明确的未知工具错误，
//!   而不是静默改坏主 turn 的计划、目标或委派状态。

use std::sync::{Arc, RwLock};

use axon_agent_runtime::{AgentPath, ToolRegistry};

/// 只属于主 turn 的工具：计划与结构化决定是主 turn 的决策权。
///
/// 子代理只执行被委托的工作，其 Provider payload 必须过滤这些工具，
/// 与不装配 `<tool_orchestration_protocol>` 片段保持同一语义。
pub(super) const MAIN_TURN_ONLY_TOOL_QUALIFIED_NAMES: &[&str] = &[
    axon_agent_runtime::UPDATE_PLAN_TOOL_QUALIFIED_NAME,
    axon_agent_runtime::ASK_USER_QUESTION_TOOL_QUALIFIED_NAME,
    // PAR-1 §8.2：委派与控制类工具只属于主代理面。
    "builtin.send_message",
    "builtin.list_agents",
    "builtin.delegate_to_fast",
];

/// 子代理是否仍可派发下一级：只有 root 路径的子代理可以。
fn may_spawn_children(task_path: &AgentPath) -> bool {
    task_path.is_root()
}

/// 该工具是否允许出现在子代理面上。
pub(super) fn subagent_tool_visible(qualified_name: &str, task_path: &AgentPath) -> bool {
    if MAIN_TURN_ONLY_TOOL_QUALIFIED_NAMES.contains(&qualified_name)
        || axon_agent_runtime::GoalToolRuntime::is_goal_tool(qualified_name)
    {
        return false;
    }
    if !may_spawn_children(task_path) {
        return qualified_name != spawn_agent_qualified_name().as_str();
    }
    true
}

fn spawn_agent_qualified_name() -> String {
    format!(
        "{}.{}",
        axon_agent_runtime::AGENT_TOOL_NAMESPACE,
        axon_agent_runtime::SPAWN_AGENT_TOOL_NAME
    )
}

/// 把子代理的可执行工具收敛到「主 turn 专属不可执行、非 root 不可再派发」的集合。
///
/// 与 payload 侧共用 [`subagent_tool_visible`]：两侧判据同源，才不会出现
/// 「看得见却调不动」或「看不见却能调」的错位。
pub(super) fn restrict_subagent_execution_tools(
    registry: Arc<RwLock<ToolRegistry>>,
    task_path: &AgentPath,
) -> Result<Arc<RwLock<ToolRegistry>>, String> {
    let restricted = {
        let guard = registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut restricted = ToolRegistry::new();
        for tool in guard.tools() {
            let qualified_name = format!("{}.{}", tool.namespace, tool.name);
            if !subagent_tool_visible(&qualified_name, task_path) {
                continue;
            }
            restricted
                .register(tool.clone())
                .map_err(|error| format!("子代理执行工具收敛失败：{error:?}"))?;
        }
        restricted
    };
    Ok(Arc::new(RwLock::new(restricted)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_path() -> AgentPath {
        AgentPath::root()
    }

    fn child_path() -> AgentPath {
        AgentPath::parse("/root/nested").expect("合法子代理路径")
    }

    /// 主 turn 专属能力对任何子代理都不可见。
    #[test]
    fn main_turn_only_tools_are_never_visible_to_subagents() {
        for name in MAIN_TURN_ONLY_TOOL_QUALIFIED_NAMES {
            assert!(
                !subagent_tool_visible(name, &root_path()),
                "{name} 不得下放"
            );
            assert!(
                !subagent_tool_visible(name, &child_path()),
                "{name} 不得下放"
            );
        }
    }

    /// Goal 工具同样只属于主 turn。
    #[test]
    fn goal_tools_are_never_visible_to_subagents() {
        assert!(!subagent_tool_visible("builtin.goal_create", &root_path()));
        assert!(!subagent_tool_visible("builtin.goal_create", &child_path()));
    }

    /// 再派发能力只对 root 路径的子代理开放；普通工具对所有子代理可见。
    #[test]
    fn spawn_agent_is_limited_to_root_path_subagents() {
        let spawn = spawn_agent_qualified_name();
        assert!(subagent_tool_visible(&spawn, &root_path()));
        assert!(!subagent_tool_visible(&spawn, &child_path()));
        assert!(subagent_tool_visible("builtin.read_file", &root_path()));
        assert!(subagent_tool_visible("builtin.read_file", &child_path()));
    }

    /// root 与嵌套子代理的工具面只允许差一个 `spawn_agent`。
    ///
    /// 这条差异是分层派发的必要条件，但它同时意味着两层子代理的 `tools` 字段不同形；
    /// 一旦差异超过这一条，前缀缓存共享就会在无声中失效，所以这里把差异集合钉死。
    #[test]
    fn nested_subagents_differ_from_root_only_by_spawn_agent() {
        let spawn = spawn_agent_qualified_name();
        let samples = [
            "builtin.update_plan",
            "builtin.ask_user_question",
            "builtin.send_message",
            "builtin.list_agents",
            "builtin.delegate_to_fast",
            "builtin.goal_create",
            "builtin.read_file",
            "builtin.shell_command",
            spawn.as_str(),
        ];
        let root = samples
            .iter()
            .filter(|name| subagent_tool_visible(name, &root_path()))
            .collect::<Vec<_>>();
        let nested = samples
            .iter()
            .filter(|name| subagent_tool_visible(name, &child_path()))
            .collect::<Vec<_>>();
        let only_root = root
            .iter()
            .filter(|name| !nested.contains(name))
            .collect::<Vec<_>>();
        assert_eq!(only_root.len(), 1, "差异集合必须只有一条：{only_root:?}");
        assert_eq!(**only_root[0], spawn_agent_qualified_name());
        assert!(
            nested.iter().all(|name| root.contains(name)),
            "嵌套子代理的工具面必须是 root 的子集"
        );
    }
}
