//! Agent context section 构造与预算映射。

use crate::agent_context::{
    AgentContextSection, AgentContextSectionKind, AgentContextSectionPayload,
};
use crate::agent_context_redaction::redact_agent_context_text;
use crate::{ContextSectionInput, ContextSectionKind};
use axon_crypto::sha256_hex;

const TOKEN_BYTES_PER_UNIT: u64 = 4;
const MIN_TOKEN_ESTIMATE: u64 = 1;
const SUMMARY_TOKEN_ESTIMATE: u64 = 64;
const MEMORY_RECALL_SOURCE_PREFIX: &str = "memory://recall";

pub(super) fn section(payload: AgentContextSectionPayload, selected: bool) -> AgentContextSection {
    let content = payload.content();
    let token_estimate = estimate_tokens(&content);
    AgentContextSection {
        payload,
        token_estimate,
        selected,
        rendered_content: None,
    }
}

pub(super) fn context_section_input(context_section: &AgentContextSection) -> ContextSectionInput {
    ContextSectionInput {
        kind: budget_kind(context_section.kind()),
        label: context_section.label().to_owned(),
        token_estimate: context_section.token_estimate,
        summary_token_estimate: SUMMARY_TOKEN_ESTIMATE.min(context_section.token_estimate),
        compressible: matches!(
            context_section.kind(),
            AgentContextSectionKind::ShellHistory
                | AgentContextSectionKind::CommandEvent
                | AgentContextSectionKind::CommandActivity
                | AgentContextSectionKind::AgentApplyHistory
                | AgentContextSectionKind::AgentChatHistory
                | AgentContextSectionKind::MemoryRecall
                | AgentContextSectionKind::LedgerContext
                | AgentContextSectionKind::RepoStructure
        ),
        ref_only: false,
        source_ref: Some(context_section.source_ref()),
    }
}

impl AgentContextSectionPayload {
    pub(super) const fn kind(&self) -> AgentContextSectionKind {
        match self {
            Self::Cwd { .. } => AgentContextSectionKind::Cwd,
            Self::Git { .. } => AgentContextSectionKind::Git,
            Self::ShellHistory { .. } => AgentContextSectionKind::ShellHistory,
            Self::CommandEvent { .. } => AgentContextSectionKind::CommandEvent,
            Self::CommandActivity { .. } => AgentContextSectionKind::CommandActivity,
            Self::AgentApplyHistory { .. } => AgentContextSectionKind::AgentApplyHistory,
            Self::AgentChatHistory { .. } => AgentContextSectionKind::AgentChatHistory,
            Self::MemoryRecall { .. } => AgentContextSectionKind::MemoryRecall,
            Self::LedgerContext { .. } => AgentContextSectionKind::LedgerContext,
            Self::RepoStructure { .. } => AgentContextSectionKind::RepoStructure,
        }
    }

    pub(super) const fn label(&self) -> &'static str {
        match self {
            Self::Cwd { .. } => "cwd",
            Self::Git { .. } => "git",
            Self::ShellHistory { .. } => "history",
            Self::CommandEvent { .. } => "command_event",
            Self::CommandActivity { .. } => "command_activity",
            Self::AgentApplyHistory { .. } => "apply_history",
            Self::AgentChatHistory { .. } => "chat_history",
            Self::MemoryRecall { .. } => "memory_recall",
            Self::LedgerContext { .. } => "ledger_context",
            Self::RepoStructure { .. } => "repo_structure",
        }
    }

    pub(super) fn source_ref(&self) -> String {
        match self {
            Self::Cwd { pane_id, .. } => format!("cwd://pane/{}", pane_id.0),
            Self::Git { pane_id, .. } => format!("git://pane/{}", pane_id.0),
            Self::ShellHistory { pane_id, .. } => format!("history://pane/{}", pane_id.0),
            Self::CommandEvent { pane_id, .. } => format!("command-event://pane/{}", pane_id.0),
            Self::CommandActivity { pane_id, .. } => {
                format!("command-activity://pane/{}", pane_id.0)
            }
            Self::AgentApplyHistory { pane_id, .. } => {
                format!("agent-apply-history://pane/{}", pane_id.0)
            }
            Self::AgentChatHistory { pane_id, .. } => {
                format!("agent-chat-history://pane/{}", pane_id.0)
            }
            Self::MemoryRecall {
                hits,
                code_symbol_hits,
                ..
            } => memory_recall_source_ref(hits, code_symbol_hits),
            Self::LedgerContext { projection } => {
                format!("ledger://scope/{}", scope_label(&projection.scope))
            }
            Self::RepoStructure { report } => {
                super::history_fragments::repo_structure_source_ref(&report.source_refs)
            }
        }
    }

    pub(super) fn content(&self) -> String {
        let content = match self {
            Self::Cwd { cwd, .. } => cwd.clone(),
            Self::Git { branch, dirty, .. } => {
                let status = if *dirty { "dirty" } else { "clean" };
                format!("branch={branch} status={status}")
            }
            Self::ShellHistory { commands, .. } => {
                super::history_fragments::history_summary(commands).unwrap_or_default()
            }
            Self::CommandEvent { commands, .. } => {
                super::command_fragments::command_event_summary(commands).unwrap_or_default()
            }
            Self::CommandActivity { activity, .. } => {
                super::command_fragments::command_activity_summary(Some(activity))
                    .unwrap_or_default()
            }
            Self::AgentApplyHistory { records, .. } => {
                super::history_fragments::apply_history_summary(records).unwrap_or_default()
            }
            Self::AgentChatHistory { records, .. } => {
                super::history_fragments::chat_history_summary(records).unwrap_or_default()
            }
            Self::MemoryRecall {
                working,
                hits,
                code_symbol_hits,
            } => super::super::agent_context_memory::memory_recall_summary_from_parts(
                working,
                hits,
                code_symbol_hits,
            )
            .unwrap_or_default(),
            Self::LedgerContext { projection } => ledger_context_summary(projection),
            Self::RepoStructure { report } => report.summary.clone(),
        };
        redact_agent_context_text(&content)
    }
}

fn budget_kind(kind: AgentContextSectionKind) -> ContextSectionKind {
    match kind {
        AgentContextSectionKind::Cwd | AgentContextSectionKind::Git => {
            ContextSectionKind::CurrentDiff
        }
        AgentContextSectionKind::ShellHistory
        | AgentContextSectionKind::CommandEvent
        | AgentContextSectionKind::CommandActivity
        | AgentContextSectionKind::AgentApplyHistory
        | AgentContextSectionKind::AgentChatHistory => ContextSectionKind::History,
        AgentContextSectionKind::MemoryRecall => ContextSectionKind::Memory,
        AgentContextSectionKind::LedgerContext => ContextSectionKind::Ledger,
        AgentContextSectionKind::RepoStructure => ContextSectionKind::CurrentDiff,
    }
}

fn scope_label(scope: &axon_domain::ScopeRef) -> String {
    match scope {
        axon_domain::ScopeRef::Global => "global".to_owned(),
        axon_domain::ScopeRef::Workspace(workspace) => workspace.as_str().to_owned(),
    }
}

fn ledger_context_summary(projection: &crate::LedgerContextProjection) -> String {
    let mut lines = Vec::new();
    for goal in &projection.active_goals {
        let steps = goal
            .plan
            .steps
            .iter()
            .map(|step| format!("- {step}"))
            .collect::<Vec<_>>()
            .join("\n");
        lines.push(format!(
            "<active-goal id=\"{}\" revision=\"{}\">{}
goal={}
plan:
{}
</active-goal>",
            goal.goal_id, goal.revision, goal.title, goal.plan.objective, steps
        ));
    }
    if let Some(decision) = projection.waiting_decision.as_ref() {
        lines.push(format!(
            "<waiting-decision id=\"{}\" revision=\"{}\" effect=\"{:?}\" actor=\"{}\" />",
            decision.decision_id, decision.revision, decision.effect, decision.actor
        ));
    }
    for result in &projection.unread_results {
        lines.push(format!(
            "<unread-result id=\"{}\" goal=\"{}\" execution=\"{}\" succeeded=\"{}\">{}</unread-result>",
            result.result_id,
            result.goal_id,
            result.execution_id,
            result.succeeded,
            result.summary
        ));
    }
    lines.join("\n")
}

fn estimate_tokens(text: &str) -> u64 {
    let bytes = u64::try_from(text.len()).unwrap_or(u64::MAX);
    let tokens = bytes
        .saturating_add(TOKEN_BYTES_PER_UNIT - 1)
        .saturating_div(TOKEN_BYTES_PER_UNIT);
    tokens.max(MIN_TOKEN_ESTIMATE)
}

fn memory_recall_source_ref(
    hits: &[axon_memory::MemoryHybridHit],
    code_symbol_hits: &[crate::MemoryCodeSymbolHit],
) -> String {
    let refs = hits
        .iter()
        .filter_map(|hit| {
            hit.record
                .record
                .content_ref
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|_| format!("memory:{}", hit.record.record.id.0))
        })
        .chain(
            code_symbol_hits
                .iter()
                .filter(|hit| hit.ref_only)
                .filter_map(|hit| {
                    let symbol_ref = hit.symbol_ref.trim();
                    (!symbol_ref.is_empty()).then(|| format!("symbol:{symbol_ref}"))
                }),
        )
        .collect::<Vec<_>>();
    if refs.is_empty() {
        MEMORY_RECALL_SOURCE_PREFIX.to_owned()
    } else {
        format!(
            "{MEMORY_RECALL_SOURCE_PREFIX}/{}",
            sha256_hex(refs.join("\n"))
        )
    }
}
