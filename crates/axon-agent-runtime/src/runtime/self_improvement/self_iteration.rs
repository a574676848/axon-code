//! 自我迭代 hook 规划运行时。

use serde::{Deserialize, Serialize};

use crate::{
    SemanticSalienceAssessment, SubagentDelegationInput, SubagentDelegationMergeReport,
    SubagentDelegationPlan, SubagentDelegationRequest, SubagentDelegationRuntime,
    SubagentInheritance, SubagentRunResult, SubagentRunStatus, TurnOutcomeEnvelope,
};
use axon_domain::{AxonResult, SessionId};

/// skill 反思最长运行时间。
pub const SELF_ITERATION_SKILL_REFLECTION_MAX_RUNTIME_MS: u64 = 30_000;
/// 记忆更新最长运行时间。
pub const SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS: u64 = 30_000;
/// 记忆更新 subagent 固定任务消息。
pub const SELF_ITERATION_MEMORY_TASK_MESSAGE: &str = "仅复盘随附的 turn outcome 与 evidence refs，先使用 builtin.memory_search 核对相关既有记忆，再判断候选是否形成可跨会话复用的 profile、preference、experience、knowledge 或 procedure。只有候选通过 owner、scope、稳定性、证据与冲突门禁时才调用 builtin.memory_write；当前证据明确证明已召回的具体记忆失效或错误时，才能通过 supersedes_memory_ids 标注其真实 Memory ID。用户明确授权长期保存的敏感内容允许沿用普通 Memory 明文链路；偶然出现在源码、环境、日志或工具输出中的凭据不得自动记忆。不得继续父任务，不得猜测 Memory ID，不得写入 prompt 原文、未验证猜测、临时日志或一次性信息。表达风格类偏好由人格系统承接，不写入 Memory。完成后只简述本次反思结果。";
/// 记忆更新 prompt mode。
pub const SELF_ITERATION_MEMORY_PROMPT_MODE: &str = "memory_update";
/// skill 反思 prompt mode。
pub const SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE: &str = "skill_reflection";
/// skill 反思任务消息前缀。
pub const SELF_ITERATION_SKILL_REFLECTION_TASK_PREFIX: &str = "仅基于随附的 verified evidence refs 复盘本次 Skill 的实际执行与验证结果。先调用 builtin.invoke_skill 读取当前 Skill 正文，再输出可直接用于改进 Skill 的准确摘要；不得把模型自述、工具次数或写盘次数当成执行证据。若需更新 Skill，必须保持标准 SKILL.md：文件以 YAML frontmatter 包裹，至少包含字符串 name 与 description，name 使用小写字母、数字和非连续连字符；正文放在结束分隔线之后。Skill：";

const SELF_ITERATION_CONTEXT_HEADER: &str =
    "\n\n以下 JSON 是运行时构建的有界事实包，禁止补造其中不存在的执行事实：\n";

/// 自我迭代规划输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationInput {
    /// 当前是否运行在 subagent 内。
    pub is_subagent: bool,
    /// 本轮有界结果事实包。
    pub turn_outcome: TurnOutcomeEnvelope,
    /// 轻量语义显著性评估；缺失或无效时保守跳过 Memory 反思。
    pub semantic_assessment: Option<SemanticSalienceAssessment>,
    /// 最近一次 skill 执行上下文。
    pub skill_execution: Option<SelfIterationSkillExecution>,
    /// 父 agent 当前累计成本，单位微美元。
    pub parent_total_cost_micros: u64,
    /// 已完成的自我迭代 subagent 结果。
    pub subagent_results: Vec<SelfIterationSubagentResult>,
}

/// Skill 执行上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationSkillExecution {
    /// skill 名称。
    pub skill_name: String,
    /// skill 来源。
    pub source: SelfIterationSkillSource,
    /// 是否存在运行时验证的实际执行证据。
    pub execution_verified: bool,
    /// 支撑实际执行结论的证据引用。
    pub evidence_refs: Vec<String>,
}

/// Skill 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfIterationSkillSource {
    /// 用户或项目自定义 skill。
    User,
    /// 项目内 skill。
    Project,
    /// 系统默认 skill。
    Default,
    /// 品牌或平台内置 skill。
    Brand,
}

/// 自我迭代 subagent 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationSubagentResult {
    /// hook 类型。
    pub hook: SelfIterationHookKind,
    /// 子任务标签。
    pub label: String,
    /// subagent 成本，单位微美元。
    pub cost_micros: u64,
    /// 运行状态。
    pub status: SelfIterationSubagentStatus,
    /// bounded 摘要。
    pub bounded_summary: String,
}

/// 自我迭代 subagent 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelfIterationSubagentStatus {
    /// 已完成且未产生持久写入。
    CompletedNoWrite,
    /// 已完成并产生持久写入。
    CompletedWithWrite,
    /// 非中断错误；不得遮蔽父任务结果。
    FailedNonFatal,
    /// 用户中断；必须向父任务传播。
    Interrupted,
}

/// 自我迭代计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationPlan {
    /// 需要执行的 hook。
    pub hooks: Vec<SelfIterationHookPlan>,
    /// 跳过原因。
    pub skipped: Vec<SelfIterationSkip>,
    /// 成本合并。
    pub cost_merge: SelfIterationCostMerge,
    /// 是否必须传播中断。
    pub interrupt_should_propagate: bool,
    /// 是否展示 memory updated 信息。
    pub memory_updated_info_visible: bool,
    /// task complete 是否必须排在 hook 之后。
    pub complete_after_hooks: bool,
    /// 远控是否允许触发自我迭代。
    pub remote_trigger_allowed: bool,
    /// 可选子代理委派计划。
    pub delegation_plan: Option<SubagentDelegationPlan>,
}

/// 自我迭代 hook 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationHookPlan {
    /// hook 类型。
    pub kind: SelfIterationHookKind,
    /// prompt mode。
    pub prompt_mode: &'static str,
    /// 任务消息。
    pub task_message: String,
    /// 是否继承父 agent 的模型、工具和历史。
    pub inherits_model_tools_history: bool,
    /// 最大运行时间。
    pub max_runtime_ms: Option<u64>,
    /// 可审计原因。
    pub reason: SelfIterationPlanReason,
}

/// 自我迭代 hook 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelfIterationHookKind {
    /// 长期记忆更新。
    MemoryUpdate,
    /// Skill 执行反思。
    SkillReflection,
}

/// 自我迭代计划原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfIterationPlanReason {
    /// root 任务完成后执行通用记忆反思。
    MemoryTaskQualified,
    /// 父 Agent 已完成选中 skill 的闭环运行，可自动反思。
    SkillReflectionQualified,
}

/// 自我迭代跳过原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfIterationSkipReason {
    /// 当前处于 subagent，阻断递归。
    SubagentRecursionBlocked,
    /// 轻量语义评估缺失或输出无效。
    SemanticAssessmentMissing,
    /// 语义显著性或确定性证据未达到完整反思门槛。
    SemanticSignalBelowThreshold,
    /// 没有 skill 执行上下文。
    SkillExecutionMissing,
    /// Skill 只有选择状态，没有实际执行证据。
    SkillExecutionUnverified,
    /// 系统拥有的 skill 不自动反思。
    SkillSystemOwned,
}

/// 自我迭代跳过记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationSkip {
    /// 被跳过的 hook 类型。
    pub hook: SelfIterationHookKind,
    /// 跳过原因。
    pub reason: SelfIterationSkipReason,
}

/// 成本合并结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfIterationCostMerge {
    /// 父 agent 原始累计成本。
    pub parent_total_cost_micros: u64,
    /// subagent 成本合计。
    pub subagent_cost_micros: u64,
    /// 合并后的累计成本。
    pub merged_total_cost_micros: u64,
}

/// 自我迭代 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SelfIterationRuntime;

impl SelfIterationRuntime {
    /// 规划任务完成前的自我迭代 hook。
    #[must_use]
    pub fn plan(&self, input: &SelfIterationInput) -> SelfIterationPlan {
        let mut hooks = Vec::new();
        let mut skipped = Vec::new();

        plan_memory_update(input, &mut hooks, &mut skipped);
        plan_skill_reflection(input, &mut hooks, &mut skipped);

        let subagent_cost_micros = input
            .subagent_results
            .iter()
            .map(|result| result.cost_micros)
            .sum();
        let interrupt_should_propagate = input
            .subagent_results
            .iter()
            .any(|result| result.status == SelfIterationSubagentStatus::Interrupted);
        let memory_updated_info_visible = input.subagent_results.iter().any(|result| {
            result.hook == SelfIterationHookKind::MemoryUpdate
                && result.status == SelfIterationSubagentStatus::CompletedWithWrite
        });

        SelfIterationPlan {
            complete_after_hooks: !hooks.is_empty(),
            hooks,
            skipped,
            cost_merge: SelfIterationCostMerge {
                parent_total_cost_micros: input.parent_total_cost_micros,
                subagent_cost_micros,
                merged_total_cost_micros: input
                    .parent_total_cost_micros
                    .saturating_add(subagent_cost_micros),
            },
            interrupt_should_propagate,
            memory_updated_info_visible,
            remote_trigger_allowed: false,
            delegation_plan: None,
        }
    }

    /// 规划并桥接为真实 subagent delegation plan。
    pub fn plan_with_delegation(
        &self,
        session_id: SessionId,
        remote_requested: bool,
        input: &SelfIterationInput,
    ) -> AxonResult<SelfIterationPlan> {
        let mut plan = self.plan(input);
        if plan.hooks.is_empty() {
            return Ok(plan);
        }

        let requests = plan
            .hooks
            .iter()
            .map(|hook| SubagentDelegationRequest {
                label: hook_label(hook).to_owned(),
                task_message: hook.task_message.clone(),
                inheritance: if hook.inherits_model_tools_history {
                    SubagentInheritance::ModelToolsHistory
                } else {
                    SubagentInheritance::ModelToolsOnly
                },
                max_runtime_ms: hook.max_runtime_ms,
            })
            .collect();
        let delegation_plan = SubagentDelegationRuntime.plan(SubagentDelegationInput {
            session_id,
            is_subagent: input.is_subagent,
            remote_requested,
            requests,
        })?;
        plan.remote_trigger_allowed = delegation_plan.remote_trigger_allowed;
        plan.delegation_plan = Some(delegation_plan);
        Ok(plan)
    }

    /// 把真实 subagent delegation 结果映射回自我迭代结果域。
    pub fn merge_delegation_results(
        &self,
        plan: &SelfIterationPlan,
        parent_total_cost_micros: u64,
        results: Vec<SelfIterationSubagentResult>,
    ) -> AxonResult<SubagentDelegationMergeReport> {
        let delegation_plan =
            plan.delegation_plan
                .as_ref()
                .ok_or(axon_domain::AxonError::runtime_failure(
                    "self_iteration delegation_plan 缺失",
                ))?;
        let subagent_results = results
            .into_iter()
            .enumerate()
            .map(|(dispatch_order, result)| SubagentRunResult {
                dispatch_order,
                label: result.label,
                status: subagent_status(result.status),
                bounded_summary: result.bounded_summary,
                cost_micros: result.cost_micros,
            })
            .collect();
        SubagentDelegationRuntime.merge_results(
            delegation_plan,
            parent_total_cost_micros,
            subagent_results,
        )
    }
}

fn plan_memory_update(
    input: &SelfIterationInput,
    hooks: &mut Vec<SelfIterationHookPlan>,
    skipped: &mut Vec<SelfIterationSkip>,
) {
    if input.is_subagent {
        skipped.push(skip(
            SelfIterationHookKind::MemoryUpdate,
            SelfIterationSkipReason::SubagentRecursionBlocked,
        ));
        return;
    }
    let Some(assessment) = input.semantic_assessment.as_ref() else {
        skipped.push(skip(
            SelfIterationHookKind::MemoryUpdate,
            SelfIterationSkipReason::SemanticAssessmentMissing,
        ));
        return;
    };
    if !assessment.qualifies_for_reflection(&input.turn_outcome) {
        skipped.push(skip(
            SelfIterationHookKind::MemoryUpdate,
            SelfIterationSkipReason::SemanticSignalBelowThreshold,
        ));
        return;
    }

    hooks.push(SelfIterationHookPlan {
        kind: SelfIterationHookKind::MemoryUpdate,
        prompt_mode: SELF_ITERATION_MEMORY_PROMPT_MODE,
        task_message: bounded_task_message(SELF_ITERATION_MEMORY_TASK_MESSAGE, input),
        inherits_model_tools_history: false,
        max_runtime_ms: Some(SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS),
        reason: SelfIterationPlanReason::MemoryTaskQualified,
    });
}

fn plan_skill_reflection(
    input: &SelfIterationInput,
    hooks: &mut Vec<SelfIterationHookPlan>,
    skipped: &mut Vec<SelfIterationSkip>,
) {
    if input.is_subagent {
        skipped.push(skip(
            SelfIterationHookKind::SkillReflection,
            SelfIterationSkipReason::SubagentRecursionBlocked,
        ));
        return;
    }
    let Some(skill) = input.skill_execution.as_ref() else {
        skipped.push(skip(
            SelfIterationHookKind::SkillReflection,
            SelfIterationSkipReason::SkillExecutionMissing,
        ));
        return;
    };
    if matches!(
        skill.source,
        SelfIterationSkillSource::Default | SelfIterationSkillSource::Brand
    ) {
        skipped.push(skip(
            SelfIterationHookKind::SkillReflection,
            SelfIterationSkipReason::SkillSystemOwned,
        ));
        return;
    }
    if !skill.execution_verified || skill.evidence_refs.is_empty() {
        skipped.push(skip(
            SelfIterationHookKind::SkillReflection,
            SelfIterationSkipReason::SkillExecutionUnverified,
        ));
        return;
    }

    let prefix = format!(
        "{SELF_ITERATION_SKILL_REFLECTION_TASK_PREFIX}{}",
        skill.skill_name
    );
    hooks.push(SelfIterationHookPlan {
        kind: SelfIterationHookKind::SkillReflection,
        prompt_mode: SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE,
        task_message: bounded_task_message(&prefix, input),
        inherits_model_tools_history: false,
        max_runtime_ms: Some(SELF_ITERATION_SKILL_REFLECTION_MAX_RUNTIME_MS),
        reason: SelfIterationPlanReason::SkillReflectionQualified,
    });
}

fn bounded_task_message(prefix: &str, input: &SelfIterationInput) -> String {
    let context = serde_json::json!({
        "turnOutcome": input.turn_outcome,
        "semanticAssessment": input.semantic_assessment,
        "skillEvidenceRefs": input
            .skill_execution
            .as_ref()
            .map(|skill| skill.evidence_refs.as_slice())
            .unwrap_or_default(),
    });
    format!("{prefix}{SELF_ITERATION_CONTEXT_HEADER}{context}")
}

const fn skip(hook: SelfIterationHookKind, reason: SelfIterationSkipReason) -> SelfIterationSkip {
    SelfIterationSkip { hook, reason }
}

const fn hook_label(hook: &SelfIterationHookPlan) -> &'static str {
    match hook.kind {
        SelfIterationHookKind::MemoryUpdate => "memory_update",
        SelfIterationHookKind::SkillReflection => "skill_reflection",
    }
}

const fn subagent_status(status: SelfIterationSubagentStatus) -> SubagentRunStatus {
    match status {
        SelfIterationSubagentStatus::CompletedNoWrite => SubagentRunStatus::CompletedNoWrite,
        SelfIterationSubagentStatus::CompletedWithWrite => SubagentRunStatus::CompletedWithWrite,
        SelfIterationSubagentStatus::FailedNonFatal => SubagentRunStatus::FailedNonFatal,
        SelfIterationSubagentStatus::Interrupted => SubagentRunStatus::Interrupted,
    }
}
