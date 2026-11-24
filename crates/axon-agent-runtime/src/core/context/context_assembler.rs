//! 统一上下文装配器：把快照采集、Memory 召回注入、预算规划和 payload 构造收敛到单次调用。

use crate::agent_context::{AgentContextRuntime, AgentContextSnapshot, AgentContextSnapshotInput};
use crate::agent_model_payload::{
    AgentModelPayload, AgentModelPayloadInput, AgentModelPayloadRuntime, AgentModelStablePrefix,
};
use crate::agent_pane::AgentPaneProjection;
use crate::environment_context::EnvironmentContext;
use crate::focus_resolver::{FocusResolutionInput, FocusResolverRuntime};
use crate::{AgentPromptContext, RepoStructureQueryReport};
use axon_context_fragments::PromptFragment;

use crate::AgentMemoryRecallInput;
use axon_domain::{AxonResult, TokenBudget};

const MINIMUM_CONTEXT_RESERVE_TOKENS: u64 = 1;

/// 上下文装配器配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextAssemblerConfig {
    /// 当前选中的 skill 名称。
    pub selected_skill_name: Option<String>,
    /// 已命中的 workspace instruction 路径。
    pub workspace_instruction_paths: Vec<String>,
}

/// 上下文装配产出。
#[derive(Debug, Clone, PartialEq)]
pub struct ContextAssemblyResult {
    /// 最终模型 payload。
    pub payload: AgentModelPayload,
    /// 上下文快照（含 budget plan，可用于审计）。
    pub snapshot: AgentContextSnapshot,
    /// Repo Structure Model 查询结果。
    pub repo_structure: Option<RepoStructureQueryReport>,
}

/// 上下文装配输入。
pub struct ContextAssemblerInput {
    /// 上下文装配配置。
    pub config: ContextAssemblerConfig,
    /// Snapshot 采集输入。
    pub snapshot_input: AgentContextSnapshotInput,
    /// Memory 召回输入。
    pub memory_recall_input: Option<AgentMemoryRecallInput>,
    /// 当前作用域的 Work Ledger 投影。
    pub ledger_context: Option<crate::LedgerContextProjection>,
    /// 首屏预编译的稳定 system/developer prefix。
    pub stable_prefix: AgentModelStablePrefix,
    /// 跨 Workspace 保持完整且顺序固定的稳定白名单上下文。
    pub stable_context_messages: Vec<crate::AgentModelMessage>,
    /// Workspace 与运行环境相关、必须放在历史后的动态上下文消息。
    pub dynamic_context_messages: Vec<crate::AgentModelMessage>,
    /// 当前 turn 的环境补充；不得重复 CWD、Workspace、Git 或运行约束。
    pub environment_context: Option<EnvironmentContext>,
    /// 当前用户输入之前的 Provider 可见会话历史。
    pub history_messages: Vec<crate::AgentModelMessage>,
    /// 提交后并行准备的 Repo Structure 查询结果。
    pub repo_structure: Option<RepoStructureQueryReport>,
    /// 用户原始 prompt。
    pub user_prompt: String,
    /// 运行时约束。
    pub runtime_constraints: Option<String>,
    /// Agent Pane 投影。
    pub pane_projection: AgentPaneProjection,
    /// 调用方提供的动态 prompt fragments。
    pub additional_fragments: Vec<Box<dyn PromptFragment>>,
}

/// 按真实模型窗口构造 Snapshot 预算。
///
/// Snapshot 只预算动态上下文片段；模型输出由自动换窗统一治理，工具 schema
/// 由 provider 请求层治理，因此这里只保留工具预算所需的最小非零占位。
pub fn model_context_snapshot_budgets(
    context_window: u64,
) -> AxonResult<(TokenBudget, TokenBudget)> {
    Ok((
        TokenBudget::new(context_window)?,
        TokenBudget::new(MINIMUM_CONTEXT_RESERVE_TOKENS)?,
    ))
}

/// 统一 Context Assembler runtime：单次调用完成从原始输入到 AgentModelPayload 的全链路装配。
#[derive(Debug, Default, Clone, Copy)]
pub struct ContextAssemblerRuntime;

impl ContextAssemblerRuntime {
    /// 装配完整 payload。
    pub fn assemble_input(
        &self,
        input: ContextAssemblerInput,
    ) -> AxonResult<ContextAssemblyResult> {
        let context_runtime = AgentContextRuntime;
        let mut snapshot = context_runtime.snapshot(input.snapshot_input);

        if let Some(ledger_context) = input.ledger_context.as_ref() {
            let focus = FocusResolverRuntime.resolve(FocusResolutionInput {
                interaction_content: &input.user_prompt,
                scope: &ledger_context.scope,
                active_goals: &ledger_context.active_goals,
            });
            let mut focused_ledger = ledger_context.clone();
            focused_ledger.active_goals = focus
                .goal_id
                .as_ref()
                .into_iter()
                .flat_map(|goal_id| {
                    ledger_context
                        .active_goals
                        .iter()
                        .filter(move |task| task.goal_id == *goal_id)
                        .cloned()
                })
                .collect();
            snapshot = context_runtime.inject_ledger_context(snapshot, &focused_ledger);
        }

        if let Some(recall_input) = input.memory_recall_input.as_ref() {
            snapshot = context_runtime.inject_memory_recall(snapshot, recall_input);
        }
        let repo_structure = input.repo_structure;
        if let Some(report) = repo_structure.as_ref() {
            snapshot = context_runtime.inject_repo_structure_summary(snapshot, report);
        }

        let mut dynamic_context_messages = input.dynamic_context_messages;
        if let Some(environment_context) = input.environment_context {
            dynamic_context_messages.push(environment_context.to_message());
        }

        let payload_input = AgentModelPayloadInput {
            user_prompt: input.user_prompt,
            stable_prefix: input.stable_prefix,
            stable_context_messages: input.stable_context_messages,
            dynamic_context_messages,
            history_messages: input.history_messages,
            prompt_context: AgentPromptContext {
                selected_skill_name: input.config.selected_skill_name.clone(),
                workspace_instruction_paths: input.config.workspace_instruction_paths.clone(),
            },
            runtime_constraints: input.runtime_constraints.clone(),
            pane_projection: input.pane_projection,
            context_snapshot: snapshot.clone(),
        };

        // 构造 fragments 并注入到 payload
        let fragment_builder = crate::context::FragmentBuilder;
        let mut fragments =
            fragment_builder.build_fragments(&crate::context::FragmentBuilderConfig {
                context_snapshot: &snapshot,
                prompt_context: &payload_input.prompt_context,
                runtime_constraints: input.runtime_constraints.as_deref(),
                user_prompt: Some(&payload_input.user_prompt),
                document_symbol_provider: None,
            });
        fragments.extend(input.additional_fragments);

        let payload = AgentModelPayloadRuntime.build_with_fragments(&payload_input, fragments)?;

        Ok(ContextAssemblyResult {
            payload,
            snapshot,
            repo_structure,
        })
    }
}
