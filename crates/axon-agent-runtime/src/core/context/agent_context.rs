//! Agent 上下文快照。

#[path = "agent_context/command_fragments.rs"]
mod command_fragments;
#[path = "agent_context/history_fragments.rs"]
mod history_fragments;
#[path = "agent_context/sections.rs"]
mod sections;

use axon_domain::{
    AudienceContext, DecisionRecord, DisclosurePolicy, GoalRecord, GoalStatus, PaneId,
    ResultRecord, ScopeRef, TokenBudget,
};
use axon_memory::MEMORY_GLOBAL_NAMESPACE_REF;
use axon_memory::MemoryHybridHit;

use crate::{
    AgentPaneApplyHistoryRecord, AgentPaneChatHistoryRecord, AgentPaneGitState, ContextBudgetPlan,
    ContextBudgetRequest, ContextBudgetRuntime, MemoryCodeSymbolHit, RecentCommandState,
    RepoStructureQueryReport,
};

use self::command_fragments::{push_command_activity_section, push_command_event_section};
use self::sections::{context_section_input, section};

/// 命令活动状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandActivityKind {
    /// 命令仍在运行，只有流式输出或错误片段。
    Running,
    /// 命令已完成，可作为完成历史或命令事件消费。
    Completed,
}

/// 命令活动上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandActivityContext {
    /// 命令文本。
    pub command: String,
    /// 活动状态。
    pub activity: CommandActivityKind,
    /// 有界输出尾部。
    pub output_tail: String,
    /// 是否允许远控读取完整上下文。
    pub remote_full_context_allowed: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
}

/// Agent 上下文快照输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContextSnapshotInput {
    /// pane ID。
    pub pane_id: PaneId,
    /// 工作目录。
    pub cwd: String,
    /// git state 摘要。
    pub git_state: Option<AgentPaneGitState>,
    /// shell history。
    pub shell_history: Vec<RecentCommandState>,
    /// 当前命令活动。
    pub command_activity: Option<CommandActivityContext>,
    /// Agent Pane apply 历史。
    pub apply_history: Vec<AgentPaneApplyHistoryRecord>,
    /// Agent Pane 聊天历史。
    pub chat_history: Vec<AgentPaneChatHistoryRecord>,
    /// 总上下文限制。
    pub total_limit: TokenBudget,
    /// 预留工具预算。
    pub reserved_for_tools: TokenBudget,
}

/// Agent 上下文快照。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentContextSnapshot {
    /// pane ID。
    pub pane_id: PaneId,
    /// 工作目录。
    pub cwd: String,
    /// 上下文片段。
    pub sections: Vec<AgentContextSection>,
    /// 预算计划。
    pub budget_plan: ContextBudgetPlan,
}

/// Agent Memory recall 注入输入。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentMemoryRecallInput {
    /// Hybrid recall 命中。
    pub hits: Vec<MemoryHybridHit>,
    /// 代码符号 ref-only 召回命中。
    pub code_symbol_hits: Vec<MemoryCodeSymbolHit>,
    /// 当前交互的受众披露边界。
    pub audience: AudienceContext,
}

/// 当前作用域的 Work Ledger 上下文投影。
///
/// 记录必须由组合根从持久化端口按 `scope` 有界读取；runtime 只负责再次执行
/// fail-closed 校验与渲染，不直接依赖 SQLite 或其他基础设施。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerContextProjection {
    /// 当前交互作用域。
    pub scope: ScopeRef,
    /// 读取时的时间，用于 Decision 有效性判断。
    pub now_ms: u64,
    /// 当前作用域的 active Goal 候选。
    pub active_goals: Vec<GoalRecord>,
    /// 当前作用域最新的等待 Decision。
    pub waiting_decision: Option<DecisionRecord>,
    /// 当前作用域的未读 Result。
    pub unread_results: Vec<ResultRecord>,
}

/// Agent 上下文片段。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentContextSection {
    /// 类型化上下文 payload。
    pub payload: AgentContextSectionPayload,
    /// token 估算。
    pub token_estimate: u64,
    /// 是否来自用户 selection。
    pub selected: bool,
    /// 预算裁剪后的 Provider 可见内容；None 表示使用 payload 原文。
    pub rendered_content: Option<String>,
}

impl AgentContextSection {
    /// 片段类型。
    #[must_use]
    pub const fn kind(&self) -> AgentContextSectionKind {
        self.payload.kind()
    }

    /// 稳定标签。
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.payload.label()
    }

    /// 本机事实引用。
    #[must_use]
    pub fn source_ref(&self) -> String {
        self.payload.source_ref()
    }

    /// 有界可渲染内容。
    #[must_use]
    pub fn content(&self) -> String {
        self.rendered_content
            .clone()
            .unwrap_or_else(|| self.payload.content())
    }
}

/// Agent 上下文片段 payload。
#[derive(Debug, Clone, PartialEq)]
pub enum AgentContextSectionPayload {
    /// 当前工作目录。
    Cwd {
        /// 当前目录。
        cwd: String,
        /// Pane ID。
        pane_id: PaneId,
    },
    /// git state。
    Git {
        /// 当前分支。
        branch: String,
        /// 是否 dirty。
        dirty: bool,
        /// Pane ID。
        pane_id: PaneId,
    },
    /// shell history 摘要。
    ShellHistory {
        /// 最近命令。
        commands: Vec<RecentCommandState>,
        /// Pane ID。
        pane_id: PaneId,
    },
    /// 命令事件摘要。
    CommandEvent {
        /// 失败或带输出的命令事件。
        commands: Vec<RecentCommandState>,
        /// Pane ID。
        pane_id: PaneId,
    },
    /// 当前命令活动。
    CommandActivity {
        /// 当前命令活动。
        activity: CommandActivityContext,
        /// Pane ID。
        pane_id: PaneId,
    },
    /// Agent Pane apply 历史。
    AgentApplyHistory {
        /// apply 历史记录。
        records: Vec<AgentPaneApplyHistoryRecord>,
        /// Pane ID。
        pane_id: PaneId,
    },
    /// Agent Pane 聊天历史。
    AgentChatHistory {
        /// 聊天历史记录。
        records: Vec<AgentPaneChatHistoryRecord>,
        /// Pane ID。
        pane_id: PaneId,
    },
    /// Memory recall 引用摘要。
    MemoryRecall {
        /// 工作记忆投影。
        working: Vec<MemoryWorkingRecallItem>,
        /// 长期 Memory 命中。
        hits: Vec<MemoryHybridHit>,
        /// 代码符号 ref-only 命中。
        code_symbol_hits: Vec<MemoryCodeSymbolHit>,
    },
    /// Work Ledger 摘要。
    LedgerContext {
        /// 当前作用域。
        projection: Box<LedgerContextProjection>,
    },
    /// Repo Structure Model 摘要。
    RepoStructure {
        /// 仓库结构查询报告。
        report: Box<RepoStructureQueryReport>,
    },
}

/// Memory recall 的工作记忆投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWorkingRecallItem {
    /// 工作记忆标签。
    pub label: String,
    /// 工作记忆来源。
    pub source_ref: String,
    /// token 估算。
    pub token_estimate: u64,
}

/// Agent 上下文片段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentContextSectionKind {
    /// 当前工作目录。
    Cwd,
    /// git state。
    Git,
    /// shell history 摘要。
    ShellHistory,
    /// 命令事件摘要。
    CommandEvent,
    /// 当前命令活动。
    CommandActivity,
    /// Agent Pane apply 历史。
    AgentApplyHistory,
    /// Agent Pane 聊天历史。
    AgentChatHistory,
    /// Memory recall 引用摘要。
    MemoryRecall,
    /// Work Ledger 摘要。
    LedgerContext,
    /// Repo Structure Model 摘要。
    RepoStructure,
}

/// Agent 上下文 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentContextRuntime;

impl AgentContextRuntime {
    /// 生成 Agent 上下文快照。
    #[must_use]
    pub fn snapshot(&self, input: AgentContextSnapshotInput) -> AgentContextSnapshot {
        let mut sections = Vec::new();
        sections.push(section(
            AgentContextSectionPayload::Cwd {
                cwd: input.cwd.clone(),
                pane_id: input.pane_id,
            },
            false,
        ));

        if let Some(git_state) = input.git_state {
            sections.push(section(
                AgentContextSectionPayload::Git {
                    branch: git_state.branch,
                    dirty: git_state.dirty,
                    pane_id: input.pane_id,
                },
                false,
            ));
        }

        if !input.shell_history.is_empty() {
            sections.push(section(
                AgentContextSectionPayload::ShellHistory {
                    commands: input.shell_history.clone(),
                    pane_id: input.pane_id,
                },
                false,
            ));
        }

        push_command_event_section(&mut sections, input.pane_id, &input.shell_history);
        push_command_activity_section(
            &mut sections,
            input.pane_id,
            input.command_activity.as_ref(),
        );

        if !input.apply_history.is_empty() {
            sections.push(section(
                AgentContextSectionPayload::AgentApplyHistory {
                    records: input.apply_history,
                    pane_id: input.pane_id,
                },
                false,
            ));
        }

        if !input.chat_history.is_empty() {
            sections.push(section(
                AgentContextSectionPayload::AgentChatHistory {
                    records: input.chat_history,
                    pane_id: input.pane_id,
                },
                false,
            ));
        }

        let budget_sections = sections.iter().map(context_section_input).collect();
        let budget_plan = ContextBudgetRuntime.plan(ContextBudgetRequest {
            total_limit: input.total_limit,
            reserved_for_tools: input.reserved_for_tools,
            sections: budget_sections,
        });

        AgentContextSnapshot {
            pane_id: input.pane_id,
            cwd: input.cwd,
            sections,
            budget_plan,
        }
    }

    /// 将 Memory recall 命中以 ref-only 摘要注入上下文快照。
    #[must_use]
    pub fn inject_memory_recall(
        &self,
        mut snapshot: AgentContextSnapshot,
        input: &AgentMemoryRecallInput,
    ) -> AgentContextSnapshot {
        let working = crate::agent_context_memory_layers::WorkingMemoryBufferRuntime
            .project(&snapshot.sections);
        let hits = filter_memory_hits_for_audience(&input.hits, &input.audience);
        if !working.is_empty() || !hits.is_empty() || !input.code_symbol_hits.is_empty() {
            snapshot.sections.push(section(
                AgentContextSectionPayload::MemoryRecall {
                    working: working
                        .into_iter()
                        .map(|item| MemoryWorkingRecallItem {
                            label: item.label,
                            source_ref: item.source_ref,
                            token_estimate: item.token_estimate,
                        })
                        .collect(),
                    hits,
                    code_symbol_hits: input.code_symbol_hits.clone(),
                },
                false,
            ));
            snapshot.budget_plan = ContextBudgetRuntime.plan(ContextBudgetRequest {
                total_limit: snapshot.budget_plan.total_limit,
                reserved_for_tools: snapshot.budget_plan.reserved_for_tools,
                sections: snapshot
                    .sections
                    .iter()
                    .map(context_section_input)
                    .collect(),
            });
        }
        snapshot
    }

    /// 将 Work Ledger 投影注入上下文；Ledger 始终位于普通 Memory 之前。
    #[must_use]
    pub fn inject_ledger_context(
        &self,
        mut snapshot: AgentContextSnapshot,
        projection: &LedgerContextProjection,
    ) -> AgentContextSnapshot {
        let active_goals = projection
            .active_goals
            .iter()
            .filter(|task| task.scope == projection.scope && task.status == GoalStatus::Active)
            .cloned()
            .collect::<Vec<_>>();
        let waiting_decision = projection.waiting_decision.as_ref().and_then(|decision| {
            (decision.scope == projection.scope && decision.is_effective_at(projection.now_ms))
                .then(|| decision.clone())
        });
        if active_goals.is_empty()
            && waiting_decision.is_none()
            && projection.unread_results.is_empty()
        {
            return snapshot;
        }
        snapshot.sections.push(section(
            AgentContextSectionPayload::LedgerContext {
                projection: Box::new(LedgerContextProjection {
                    scope: projection.scope.clone(),
                    now_ms: projection.now_ms,
                    active_goals,
                    waiting_decision,
                    unread_results: projection.unread_results.clone(),
                }),
            },
            false,
        ));
        snapshot.budget_plan = ContextBudgetRuntime.plan(ContextBudgetRequest {
            total_limit: snapshot.budget_plan.total_limit,
            reserved_for_tools: snapshot.budget_plan.reserved_for_tools,
            sections: snapshot
                .sections
                .iter()
                .map(context_section_input)
                .collect(),
        });
        snapshot
    }

    /// 将 Repo Structure Model 摘要注入上下文快照。
    #[must_use]
    pub fn inject_repo_structure_summary(
        &self,
        mut snapshot: AgentContextSnapshot,
        report: &RepoStructureQueryReport,
    ) -> AgentContextSnapshot {
        if report.summary.trim().is_empty() {
            return snapshot;
        }
        snapshot.sections.push(section(
            AgentContextSectionPayload::RepoStructure {
                report: Box::new(report.clone()),
            },
            false,
        ));
        snapshot.budget_plan = ContextBudgetRuntime.plan(ContextBudgetRequest {
            total_limit: snapshot.budget_plan.total_limit,
            reserved_for_tools: snapshot.budget_plan.reserved_for_tools,
            sections: snapshot
                .sections
                .iter()
                .map(context_section_input)
                .collect(),
        });
        snapshot
    }
}

/// 按当前交互的披露策略过滤 Memory 命中。
#[must_use]
pub fn filter_memory_hits_for_audience(
    hits: &[MemoryHybridHit],
    audience: &AudienceContext,
) -> Vec<MemoryHybridHit> {
    hits.iter()
        .filter(|hit| match audience.disclosure() {
            DisclosurePolicy::PrivateOnly => true,
            DisclosurePolicy::Workspace => !is_global_memory_hit(hit),
            DisclosurePolicy::Global => is_global_memory_hit(hit),
        })
        .cloned()
        .collect()
}

fn is_global_memory_hit(hit: &MemoryHybridHit) -> bool {
    let record = &hit.record;
    record.user_ref.is_none()
        && record.namespace_ref.as_deref() == Some(MEMORY_GLOBAL_NAMESPACE_REF)
}
