//! 自我迭代 hook 规划运行时。

use serde::{Deserialize, Serialize};

use crate::{
    ReflectionCard, ReflectionTier, SemanticSalienceAssessment, SubagentDelegationInput,
    SubagentDelegationMergeReport, SubagentDelegationPlan, SubagentDelegationRequest,
    SubagentDelegationRuntime, SubagentInheritance, SubagentRunResult, SubagentRunStatus,
    TurnOutcomeEnvelope,
};
use axon_domain::{AxonResult, SessionId};

/// 记忆更新最长运行时间。
pub const SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS: u64 = 30_000;
/// 记忆更新 subagent 固定任务消息。
pub const SELF_ITERATION_MEMORY_TASK_MESSAGE: &str = "仅复盘随附的 turn outcome 与 evidence refs；随附的是反思索引卡时，先按 trace_id 或 receipt_ref 调用 builtin.turn_trace 取回执原文，再使用 builtin.memory_search 核对相关既有记忆，最后判断候选是否形成可跨会话复用的 profile、preference、experience、knowledge 或 procedure。不要调用 builtin.memory_write：写入由运行时统一执行，你只交候选。只有候选通过 owner、scope、稳定性、证据与冲突门禁时才写进候选清单；当前证据明确证明已召回的具体记忆失效或错误时，用 targetMemoryId 指向那条真实 Memory ID，由运行时决定淘汰还是归档。作用域按地点划分：profile 与 preference 属于个人记忆；experience、knowledge 与 procedure 永远属于工作区记忆，普通 Workspace 与其 Git 分支各成桶，而在 ~/.axon（Axon 主目录）里产生的一律写入「Axon 主目录」这一固定桶，不得因为它们发生在主目录就改写成个人记忆，也不得因为那里没有普通 Workspace 就放弃写入。用户明确授权长期保存的敏感内容允许沿用普通 Memory 明文链路；偶然出现在源码、环境、日志或工具输出中的凭据不得自动记忆。不得继续父任务，不得猜测 Memory ID，不得写入 prompt 原文、未验证猜测、临时日志或一次性信息。表达风格类偏好由人格系统承接，不写入 Memory。最终回复只输出一个 JSON 对象，不要加解释文字：{\"candidates\":[{\"kind\":\"knowledge\",\"summary\":\"记忆正文\",\"evidence\":\"证据说明\",\"evidenceRefs\":[\"receipt:1\"],\"targetMemoryId\":null}],\"skipped\":[{\"kind\":\"procedure\",\"reason\":\"跳过理由\"}]}。";
/// 记忆更新 prompt mode。
pub const SELF_ITERATION_MEMORY_PROMPT_MODE: &str = "memory_update";
/// 轻量反思档的任务消息：只放宽是否值得跑的门槛，不放宽写入标准。
pub const SELF_ITERATION_MEMORY_WEAK_TASK_MESSAGE: &str = "仅复盘随附的 turn outcome 与 evidence refs。随附的是反思索引卡时，先按 trace_id 或 receipt_ref 调用 builtin.turn_trace 取回执原文。不要调用 builtin.memory_write：写入由运行时统一执行，你只交候选。本轮语义显著度处于弱档：弱档只放宽「是否值得跑」的门槛，不放宽写入标准。候选必须与强档一样同时满足跨会话可复用、证据充分、不与既有更强记忆冲突这三项，才允许写进候选清单；任何一项不满足就写进 skipped 并给出理由，不得产出低置信候选。不得继续父任务，不得猜测 Memory ID，不得写入 prompt 原文、未验证猜测、临时日志或一次性信息。表达风格类偏好由人格系统承接，不写入 Memory。最终回复只输出一个 JSON 对象，不要加解释文字：{\"candidates\":[{\"kind\":\"knowledge\",\"summary\":\"记忆正文\",\"evidence\":\"证据说明\",\"evidenceRefs\":[\"receipt:1\"],\"targetMemoryId\":null}],\"skipped\":[{\"kind\":\"procedure\",\"reason\":\"跳过理由\"}]}。";

const SELF_ITERATION_CONTEXT_HEADER: &str =
    "\n\n以下 JSON 是运行时构建的有界事实包，禁止补造其中不存在的执行事实：\n";
const SELF_ITERATION_CARD_CONTEXT_HEADER: &str = "\n\n以下 JSON 是运行时构建的反思索引卡，只含引用、指纹与显著度，不含任何正文：需要原文时按 trace_id 或 receipt_ref 调用 builtin.turn_trace，禁止补造卡片里不存在的执行事实：\n";

/// 自我迭代规划输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationInput {
    /// 当前是否运行在 subagent 内。
    pub is_subagent: bool,
    /// 本轮有界结果事实包。
    pub turn_outcome: TurnOutcomeEnvelope,
    /// 反思索引卡；非空时任务消息改发卡片，摘要事实包退为兜底。
    pub reflection_cards: Vec<ReflectionCard>,
    /// 轻量语义显著性评估；缺失或无效时保守跳过 Memory 反思。
    pub semantic_assessment: Option<SemanticSalienceAssessment>,
    /// 父 agent 当前累计成本，单位微美元。
    pub parent_total_cost_micros: u64,
    /// 已完成的自我迭代 subagent 结果。
    pub subagent_results: Vec<SelfIterationSubagentResult>,
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
    /// 本次 hook 的反思分档；弱档只放宽启动门槛，写入门槛不变。
    pub reflection_tier: ReflectionTier,
}

/// 自我迭代 hook 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelfIterationHookKind {
    /// 长期记忆更新。
    MemoryUpdate,
}

/// 自我迭代计划原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfIterationPlanReason {
    /// root 任务完成后执行通用记忆反思。
    MemoryTaskQualified,
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

        let subagent_cost_micros = input
            .subagent_results
            .iter()
            .map(|result| result.cost_micros)
            .sum();
        let interrupt_should_propagate = input
            .subagent_results
            .iter()
            .any(|result| result.status == SelfIterationSubagentStatus::Interrupted);

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
    let tier = assessment.reflection_tier(&input.turn_outcome);
    if tier == ReflectionTier::Skip {
        skipped.push(skip(
            SelfIterationHookKind::MemoryUpdate,
            SelfIterationSkipReason::SemanticSignalBelowThreshold,
        ));
        return;
    }

    let prefix = match tier {
        ReflectionTier::Weak => SELF_ITERATION_MEMORY_WEAK_TASK_MESSAGE,
        ReflectionTier::Strong => SELF_ITERATION_MEMORY_TASK_MESSAGE,
        ReflectionTier::Skip => return,
    };
    hooks.push(SelfIterationHookPlan {
        kind: SelfIterationHookKind::MemoryUpdate,
        prompt_mode: SELF_ITERATION_MEMORY_PROMPT_MODE,
        task_message: bounded_task_message(prefix, input, tier),
        inherits_model_tools_history: false,
        max_runtime_ms: Some(SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS),
        reason: SelfIterationPlanReason::MemoryTaskQualified,
        reflection_tier: tier,
    });
}

/// 拼装反思任务消息。
///
/// 有索引卡时只发卡片：卡片是索引，正文一律靠 `builtin.turn_trace` 回取，
/// 这样输入尺寸与任务长度解耦。没有卡片时退回有界事实包，保证历史行为不变。
fn bounded_task_message(prefix: &str, input: &SelfIterationInput, tier: ReflectionTier) -> String {
    if input.reflection_cards.is_empty() {
        let context = serde_json::json!({
            "turnOutcome": input.turn_outcome,
            "semanticAssessment": input.semantic_assessment,
            "reflectionTier": tier.as_str(),
        });
        return format!("{prefix}{SELF_ITERATION_CONTEXT_HEADER}{context}");
    }
    let context = serde_json::json!({
        "reflectionCards": input.reflection_cards,
        "reflectionTier": tier.as_str(),
    });
    format!("{prefix}{SELF_ITERATION_CARD_CONTEXT_HEADER}{context}")
}

const fn skip(hook: SelfIterationHookKind, reason: SelfIterationSkipReason) -> SelfIterationSkip {
    SelfIterationSkip { hook, reason }
}

const fn hook_label(hook: &SelfIterationHookPlan) -> &'static str {
    match hook.kind {
        SelfIterationHookKind::MemoryUpdate => "memory_update",
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
