//! Agent context snapshot fragment。

use axon_context_fragments::{PromptFragment, PromptFragmentMetadata};
use axon_domain::{AxonError, AxonResult};

use crate::{AgentContextSection, AgentContextSnapshot, ContextSectionDecision};

/// Agent context snapshot fragment，承载运行时采集到的有界上下文。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentContextSnapshotFragment {
    sections: Vec<AgentContextSection>,
}

impl AgentContextSnapshotFragment {
    /// 根据预算计划构造当前 turn 真正发送给 Provider 的动态 fragment。
    pub fn try_from_snapshot(snapshot: &AgentContextSnapshot) -> AxonResult<Self> {
        let sections = budgeted_sections(snapshot)?;
        Ok(Self { sections })
    }

    /// 返回真正进入 Provider payload 的 section 数量。
    pub const fn section_count(&self) -> usize {
        self.sections.len()
    }
}

impl PromptFragment for AgentContextSnapshotFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<agent-context-snapshot>", "</agent-context-snapshot>")
    }

    fn body(&self) -> String {
        let sections = self
            .sections
            .iter()
            .map(|section| {
                format!(
                    "  <section kind=\"{}\" source=\"{}\" selected=\"{}\">\n{}\n  </section>",
                    section.label(),
                    section.source_ref(),
                    section.selected,
                    section.content()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n{sections}\n")
    }
}

fn budgeted_sections(snapshot: &AgentContextSnapshot) -> AxonResult<Vec<AgentContextSection>> {
    let mut rendered = Vec::new();
    for section in &snapshot.sections {
        let budget = snapshot
            .budget_plan
            .sections
            .iter()
            .find(|budget| {
                budget.label == section.label()
                    && budget.source_ref.as_deref() == Some(section.source_ref().as_str())
            })
            .ok_or_else(|| AxonError::runtime_failure("上下文预算计划缺少 section 决策"))?;
        match &budget.decision {
            ContextSectionDecision::Drop => {
                if matches!(
                    section.kind(),
                    crate::AgentContextSectionKind::Cwd | crate::AgentContextSectionKind::Git
                ) {
                    return Err(AxonError::runtime_failure("受保护上下文无法在预算内保留"));
                }
            }
            ContextSectionDecision::KeepRef => {
                rendered.push(section_with_content(
                    section,
                    format!("reference={}", section.source_ref()),
                ));
            }
            ContextSectionDecision::Summarize { .. } => {
                let max_chars = budget.included_token_estimate.saturating_mul(4) as usize;
                rendered.push(section_with_content(
                    section,
                    bounded_summary(&section.content(), max_chars),
                ));
            }
            ContextSectionDecision::IncludeOriginal => rendered.push(section.clone()),
        }
    }
    Ok(rendered)
}

fn section_with_content(section: &AgentContextSection, content: String) -> AgentContextSection {
    let mut rendered = section.clone();
    rendered.rendered_content = Some(content);
    rendered
}

fn bounded_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_owned();
    }
    let marker = "\n[summary truncated]";
    let keep = max_chars.saturating_sub(marker.chars().count());
    let head = keep / 2;
    let tail = keep.saturating_sub(head);
    let prefix = content.chars().take(head).collect::<String>();
    let suffix = content
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}{marker}{suffix}")
}
