//! 上下文预算运行时。

use axon_domain::TokenBudget;
use serde::{Deserialize, Serialize};

/// 默认摘要 token 估算上限。
pub const DEFAULT_SUMMARY_TOKEN_ESTIMATE: u64 = 128;

/// 上下文片段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSectionKind {
    /// system/developer policy。
    Policy,
    /// selected terminal range。
    Selection,
    /// current file / diff。
    CurrentDiff,
    /// recent tool output。
    ToolOutput,
    /// Work Ledger context，优先于普通 Memory。
    Ledger,
    /// memory recall summary。
    Memory,
    /// recent command output excerpt。
    CommandOutput,
    /// low-priority history summary。
    History,
    /// attachment ref。
    Attachment,
}

/// 上下文片段预算输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSectionInput {
    /// 片段类型。
    pub kind: ContextSectionKind,
    /// 稳定名称。
    pub label: String,
    /// 原始 token 估算。
    pub token_estimate: u64,
    /// 有界摘要或引用 token 估算。
    pub summary_token_estimate: u64,
    /// 是否可压缩为摘要。
    pub compressible: bool,
    /// 是否必须作为 ref 进入上下文。
    pub ref_only: bool,
    /// 本机 timeline 或对象引用。
    pub source_ref: Option<String>,
}

/// 上下文预算请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetRequest {
    /// 总上下文限制。
    pub total_limit: TokenBudget,
    /// 预留工具预算。
    pub reserved_for_tools: TokenBudget,
    /// 输入片段。
    pub sections: Vec<ContextSectionInput>,
}

/// 上下文预算计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetPlan {
    /// 总上下文限制。
    pub total_limit: TokenBudget,
    /// 预留工具预算。
    pub reserved_for_tools: TokenBudget,
    /// 可用于输入上下文的 token。
    pub available_for_context: u64,
    /// 片段决策。
    pub sections: Vec<ContextSectionBudget>,
    /// 逐段裁剪回执。
    pub receipts: Vec<ContextSectionReceipt>,
    /// 溢出处理动作。
    pub overflow_action: OverflowAction,
    /// 是否发生溢出。
    pub overflowed: bool,
    /// 是否禁止把摘要当作原文证据。
    pub summary_is_original_evidence: bool,
    /// 预算观测事件。
    pub events: Vec<ContextBudgetEvent>,
}

/// 单个 section 的裁剪回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSectionReceipt {
    /// 片段类型。
    pub kind: ContextSectionKind,
    /// 稳定名称。
    pub label: String,
    /// 独立预算上限。
    pub budget_token_limit: u64,
    /// 原始 token 估算。
    pub original_token_estimate: u64,
    /// 实际保留 token 估算。
    pub included_token_estimate: u64,
    /// 最终决策。
    pub decision: ContextSectionDecision,
    /// 裁剪原因。
    pub reason: ContextSectionDecisionReason,
    /// 来源引用。
    pub source_ref: Option<String>,
}

/// 单个片段预算结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSectionBudget {
    /// 片段类型。
    pub kind: ContextSectionKind,
    /// 稳定名称。
    pub label: String,
    /// 原始 token 估算。
    pub original_token_estimate: u64,
    /// 进入 prompt 的 token 估算。
    pub included_token_estimate: u64,
    /// 片段决策。
    pub decision: ContextSectionDecision,
    /// 独立预算上限。
    pub budget_token_limit: u64,
    /// 可审计裁剪回执。
    pub receipt: ContextSectionReceipt,
    /// 本机 timeline 或对象引用。
    pub source_ref: Option<String>,
}

/// 单个片段决策。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSectionDecision {
    /// 原文进入上下文。
    IncludeOriginal,
    /// 用固定结构摘要替代原文。
    Summarize {
        /// 摘要不能作为原始证据。
        original_evidence_allowed: bool,
    },
    /// 只保留引用。
    KeepRef,
    /// 因预算不足被移除。
    Drop,
}

/// 单个 section 的裁剪原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSectionDecisionReason {
    /// 原文在该 section 的独立预算内。
    WithinBudget,
    /// 原文超出预算，已使用有界摘要。
    SummarizedForBudget,
    /// 原文超出预算，仅保留可回查引用。
    RefOnlyForBudget,
    /// 非保护 section 因预算不足被移除。
    DroppedForBudget,
    /// 保护 section 无法满足预算，阻断本次装配。
    ProtectedBudgetExceeded,
}

/// 溢出处理动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowAction {
    /// 未溢出。
    NotNeeded,
    /// 摘要头部或低优先级内容，并保护高优先级 tail。
    SummarizeLowPriorityKeepProtected,
    /// 剥离媒体或大对象，只保留引用。
    StripMediaKeepRefs,
    /// 高优先级内容也无法放入，需要本机用户选择。
    BlockUntilUserChooses,
}

/// 上下文预算事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextBudgetEvent {
    /// 预算计划已生成。
    PlanCreated {
        /// 可用上下文 token。
        available_for_context: u64,
    },
    /// 片段已压缩。
    SectionSummarized {
        /// 片段名称。
        label: String,
    },
    /// 片段已转为 ref。
    SectionRefOnly {
        /// 片段名称。
        label: String,
    },
    /// 片段被丢弃。
    SectionDropped {
        /// 片段名称。
        label: String,
    },
    /// 需要本机用户选择。
    UserChoiceRequired {
        /// 中文原因。
        reason: String,
    },
}

/// 上下文预算 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ContextBudgetRuntime;

impl ContextBudgetRuntime {
    /// 生成上下文预算计划。
    #[must_use]
    pub fn plan(&self, request: ContextBudgetRequest) -> ContextBudgetPlan {
        let available_for_context = request
            .total_limit
            .max_tokens
            .saturating_sub(request.reserved_for_tools.max_tokens);
        let mut remaining = available_for_context;
        let original_total = request
            .sections
            .iter()
            .map(|section| section.token_estimate)
            .sum::<u64>();
        let mut overflowed = original_total > available_for_context;
        let mut events = vec![ContextBudgetEvent::PlanCreated {
            available_for_context,
        }];
        let mut state = BudgetPlanningState::default();
        let mut sections = request.sections;

        sections.sort_by_key(|section| section_priority(section.kind));

        let section_budgets = sections
            .into_iter()
            .map(|section| {
                let decision =
                    decide_section(&section, &mut remaining, &mut overflowed, &mut state);
                let receipt = ContextSectionReceipt {
                    kind: section.kind,
                    label: section.label.clone(),
                    budget_token_limit: decision.included_tokens,
                    original_token_estimate: section.token_estimate,
                    included_token_estimate: decision.included_tokens,
                    decision: decision.decision.clone(),
                    reason: decision.reason,
                    source_ref: section.source_ref.clone(),
                };
                ContextSectionBudget {
                    kind: section.kind,
                    label: section.label,
                    original_token_estimate: section.token_estimate,
                    included_token_estimate: decision.included_tokens,
                    budget_token_limit: decision.included_tokens,
                    decision: decision.decision,
                    receipt,
                    source_ref: section.source_ref,
                }
            })
            .collect::<Vec<_>>();
        let receipts = section_budgets
            .iter()
            .map(|section| section.receipt.clone())
            .collect::<Vec<_>>();

        events.append(&mut state.events);
        if state.blocked {
            events.push(ContextBudgetEvent::UserChoiceRequired {
                reason: "高优先级上下文超过可用预算".to_owned(),
            });
        }

        ContextBudgetPlan {
            total_limit: request.total_limit,
            reserved_for_tools: request.reserved_for_tools,
            available_for_context,
            sections: section_budgets,
            receipts,
            overflow_action: overflow_action(&state),
            overflowed,
            summary_is_original_evidence: false,
            events,
        }
    }
}

struct SectionDecision {
    decision: ContextSectionDecision,
    included_tokens: u64,
    reason: ContextSectionDecisionReason,
}

#[derive(Default)]
struct BudgetPlanningState {
    used_ref_only: bool,
    summarized: bool,
    dropped: bool,
    blocked: bool,
    events: Vec<ContextBudgetEvent>,
}

fn decide_section(
    section: &ContextSectionInput,
    remaining: &mut u64,
    overflowed: &mut bool,
    state: &mut BudgetPlanningState,
) -> SectionDecision {
    if section.ref_only {
        if section.summary_token_estimate <= *remaining {
            *remaining -= section.summary_token_estimate;
            state.used_ref_only = true;
            state.events.push(ContextBudgetEvent::SectionRefOnly {
                label: section.label.clone(),
            });
            return SectionDecision {
                decision: ContextSectionDecision::KeepRef,
                included_tokens: section.summary_token_estimate,
                reason: ContextSectionDecisionReason::RefOnlyForBudget,
            };
        }
        *overflowed = true;
    } else {
        if section.token_estimate <= *remaining {
            *remaining -= section.token_estimate;
            return SectionDecision {
                decision: ContextSectionDecision::IncludeOriginal,
                included_tokens: section.token_estimate,
                reason: ContextSectionDecisionReason::WithinBudget,
            };
        }

        *overflowed = true;
        if section.compressible && section.summary_token_estimate <= *remaining {
            *remaining -= section.summary_token_estimate;
            state.summarized = true;
            state.events.push(ContextBudgetEvent::SectionSummarized {
                label: section.label.clone(),
            });
            return SectionDecision {
                decision: ContextSectionDecision::Summarize {
                    original_evidence_allowed: false,
                },
                included_tokens: section.summary_token_estimate,
                reason: ContextSectionDecisionReason::SummarizedForBudget,
            };
        }
    }

    if protected_kind(section.kind) {
        state.blocked = true;
        SectionDecision {
            decision: ContextSectionDecision::Drop,
            included_tokens: 0,
            reason: ContextSectionDecisionReason::ProtectedBudgetExceeded,
        }
    } else {
        state.dropped = true;
        state.events.push(ContextBudgetEvent::SectionDropped {
            label: section.label.clone(),
        });
        SectionDecision {
            decision: ContextSectionDecision::Drop,
            included_tokens: 0,
            reason: ContextSectionDecisionReason::DroppedForBudget,
        }
    }
}

fn overflow_action(state: &BudgetPlanningState) -> OverflowAction {
    if state.blocked {
        OverflowAction::BlockUntilUserChooses
    } else if state.used_ref_only {
        OverflowAction::StripMediaKeepRefs
    } else if state.summarized || state.dropped {
        OverflowAction::SummarizeLowPriorityKeepProtected
    } else {
        OverflowAction::NotNeeded
    }
}

const fn section_priority(kind: ContextSectionKind) -> u8 {
    match kind {
        ContextSectionKind::Policy => 0,
        ContextSectionKind::Selection => 1,
        ContextSectionKind::CurrentDiff => 2,
        ContextSectionKind::ToolOutput => 3,
        ContextSectionKind::Ledger => 4,
        ContextSectionKind::Memory => 5,
        ContextSectionKind::CommandOutput => 6,
        ContextSectionKind::History => 7,
        ContextSectionKind::Attachment => 8,
    }
}

const fn protected_kind(kind: ContextSectionKind) -> bool {
    matches!(
        kind,
        ContextSectionKind::Policy | ContextSectionKind::Selection
    )
}
