//! 模型 payload 的动态文本片段构造。
//!
//! 这些片段逐轮变化、不进缓存：异常状态段、Pane 投影与运行时约束块。
//! 与载荷编排分离后，`run_model_payload` 只负责装配顺序与预算。

use axon_agent_runtime::{
    AgentModelMessage, AgentPaneProjection, AgentPaneProjectionInput, MemoryHybridHit,
    agent_model_runtime_constraints,
};
use axon_domain::{PaneId, PaneSecurityBadge, PaneSecurityBadgeLevel, PermissionSnapshotId};

use crate::runtime_workspace_instructions::WorkspaceInstruction;

use super::HOME_WITHOUT_WORKSPACE;

/// 由最近助手输出与问答对推导异常状态段；未命中阈值时返回 `None`。
pub(super) fn anomaly_state_text(prompt: &str, history: &[AgentModelMessage]) -> Option<String> {
    let recent_assistant = history
        .iter()
        .rev()
        .filter(|message| message.role == "assistant")
        .map(|message| message.content.as_str())
        .take(axon_agent_runtime::anomaly::ANOMALY_STYLE_WINDOW)
        .collect::<Vec<_>>();
    let recent_pairs = recent_user_assistant_pairs(history);
    axon_agent_runtime::anomaly::detect(prompt, &recent_assistant, &recent_pairs).state_text()
}

/// 按时间正序配对 user 与紧随其后的 assistant，再反转为最近优先。
fn recent_user_assistant_pairs(history: &[AgentModelMessage]) -> Vec<(&str, &str)> {
    let mut pairs = Vec::new();
    let mut pending_user: Option<&str> = None;
    for message in history {
        match message.role.as_str() {
            "user" => pending_user = Some(message.content.as_str()),
            "assistant" => {
                if let Some(user) = pending_user.take() {
                    pairs.push((user, message.content.as_str()));
                }
            }
            _ => {}
        }
    }
    pairs.reverse();
    pairs
}

pub(super) fn cli_pane_projection(
    pane_id: PaneId,
    cwd: &str,
    git_state: Option<axon_agent_runtime::AgentPaneGitState>,
) -> AgentPaneProjection {
    AgentPaneProjection::from_input(AgentPaneProjectionInput {
        pane_id,
        cwd: cwd.to_owned(),
        shell_kind: "cli".to_owned(),
        git_state,
        recent_command: None,
        permission_snapshot_id: PermissionSnapshotId(0),
        backend_badge: PaneSecurityBadge {
            level: PaneSecurityBadgeLevel::Normal,
            label: "cli".to_owned(),
            reasons: Vec::new(),
        },
        runtime_cell: None,
        response: None,
        apply_execution: None,
    })
}

pub(super) fn cli_runtime_constraints(
    selected_skill_name: Option<&str>,
    home_root: &str,
    workspace_root: Option<&str>,
    workspace_instructions: &[WorkspaceInstruction],
    memory_hits: &[MemoryHybridHit],
    max_subagent_threads: usize,
    max_subagent_active_turns: usize,
) -> String {
    let mut constraints =
        agent_model_runtime_constraints(axon_agent_runtime::AgentModelRuntimeConstraintsInput {
            surface: "cli_run",
            selected_skill_name,
            home_root,
            workspace_root: workspace_root.unwrap_or(HOME_WITHOUT_WORKSPACE),
            workspace_instruction_paths: workspace_instructions
                .iter()
                .map(|instruction| instruction.path.clone())
                .collect(),
            capability_lines: Vec::new(),
            memory_hits,
        });
    constraints.push('\n');
    constraints.push_str(&axon_agent_runtime::agent_delegation_runtime_guidance(
        max_subagent_threads,
        0,
        max_subagent_active_turns,
        0,
    ));
    constraints
}
