//! AgentContext Memory recall 分层投影。

use axon_domain::MemorySourceOrigin;

use crate::{AgentContextSection, AgentContextSectionKind, MemoryCodeSymbolHit};

/// Memory recall 分层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRecallLayer {
    /// 当前 turn/session 的短期工作记忆，不写入长期 store。
    Working,
    /// 带时间、任务和证据坐标的情景记忆。
    Episodic,
    /// 稳定偏好、规范或项目事实。
    Semantic,
    /// skill、playbook 或工具流程。
    Procedural,
    /// 代码符号 ref-only 索引。
    CodeSymbol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingMemoryItem {
    pub label: String,
    pub source_ref: String,
    pub token_estimate: u64,
}

/// 当前上下文工作记忆缓冲区投影 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkingMemoryBufferRuntime;

impl WorkingMemoryBufferRuntime {
    /// 从当前快照中提取有界工作记忆引用。
    #[must_use]
    pub fn project(&self, sections: &[AgentContextSection]) -> Vec<WorkingMemoryItem> {
        let mut prioritized = sections
            .iter()
            .filter_map(|section| {
                working_memory_priority(section.kind()).map(|priority| (priority, section))
            })
            .collect::<Vec<_>>();
        prioritized.sort_by_key(|(priority, _)| *priority);
        prioritized
            .into_iter()
            .map(|(_, section)| WorkingMemoryItem {
                label: section.label().to_owned(),
                source_ref: section.source_ref(),
                token_estimate: section.token_estimate,
            })
            .collect()
    }
}

#[must_use]
pub fn classify_long_term_origin(origin: Option<MemorySourceOrigin>) -> MemoryRecallLayer {
    match origin {
        None => MemoryRecallLayer::Working,
        Some(
            MemorySourceOrigin::LocalTimeline
            | MemorySourceOrigin::AgentExperience
            | MemorySourceOrigin::DebuggingExperience
            | MemorySourceOrigin::CommandCompletionFeedback,
        ) => MemoryRecallLayer::Episodic,
        Some(MemorySourceOrigin::AgentProcedure | MemorySourceOrigin::SkillReflection) => {
            MemoryRecallLayer::Procedural
        }
        Some(
            MemorySourceOrigin::AgentUserContext
            | MemorySourceOrigin::AgentKnowledge
            | MemorySourceOrigin::ExplicitUserPreference
            | MemorySourceOrigin::RemoteUpload,
        ) => MemoryRecallLayer::Semantic,
    }
}

#[must_use]
pub fn code_symbol_layer(_hit: &MemoryCodeSymbolHit) -> MemoryRecallLayer {
    MemoryRecallLayer::CodeSymbol
}

#[must_use]
pub const fn memory_recall_layer_label(layer: MemoryRecallLayer) -> &'static str {
    match layer {
        MemoryRecallLayer::Working => "working_memory",
        MemoryRecallLayer::Episodic => "episodic_memory",
        MemoryRecallLayer::Semantic => "semantic_memory",
        MemoryRecallLayer::Procedural => "procedural_memory",
        MemoryRecallLayer::CodeSymbol => "code_symbol_memory",
    }
}

fn working_memory_priority(kind: AgentContextSectionKind) -> Option<u8> {
    match kind {
        AgentContextSectionKind::CommandActivity => Some(0),
        AgentContextSectionKind::CommandEvent => Some(1),
        AgentContextSectionKind::AgentChatHistory => Some(3),
        AgentContextSectionKind::AgentApplyHistory => Some(4),
        AgentContextSectionKind::Cwd
        | AgentContextSectionKind::Git
        | AgentContextSectionKind::ShellHistory
        | AgentContextSectionKind::LedgerContext
        | AgentContextSectionKind::MemoryRecall
        | AgentContextSectionKind::RepoStructure => None,
    }
}
