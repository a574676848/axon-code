//! Plan 状态面的 source-backed 渲染数据。

use crate::agent_tui::AgentTuiPlanProgress;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderPlanCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) explanation: Option<String>,
    pub(crate) steps: Vec<AgentRenderPlanStep>,
    pub(crate) source: String,
    pub(crate) source_kind: AgentRenderPlanSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderPlanStep {
    pub(crate) id: Option<String>,
    pub(crate) status: AgentRenderPlanStatus,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderPlanSourceKind {
    Markdown,
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderPlanStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

const PLAN_STATUS_PENDING_MARKER: &str = "[ ]";
const PLAN_STATUS_RUNNING_MARKER: &str = "[~]";
const PLAN_STATUS_COMPLETED_MARKER: &str = "[x]";
const PLAN_STATUS_FAILED_MARKER: &str = "[!]";
const PLAN_STATUS_SKIPPED_MARKER: &str = "[-]";

impl AgentRenderPlanCell {
    pub(crate) fn progress(&self) -> Option<AgentTuiPlanProgress> {
        let total = self.steps.len();
        (total > 0).then(|| AgentTuiPlanProgress {
            completed: self
                .steps
                .iter()
                .filter(|step| step.status == AgentRenderPlanStatus::Completed)
                .count(),
            total,
        })
    }

    pub(crate) fn from_tui_line(line: &crate::AgentTuiPlanLine) -> Self {
        Self::from_tui_line_with_source_kind(line, AgentRenderPlanSourceKind::Markdown)
    }

    pub(crate) fn stream_from_tui_line(line: &crate::AgentTuiPlanLine) -> Self {
        Self::from_tui_line_with_source_kind(line, AgentRenderPlanSourceKind::Stream)
    }

    pub(crate) fn into_markdown_source_backed(mut self) -> Self {
        self.source_kind = AgentRenderPlanSourceKind::Markdown;
        self.refresh_source();
        self
    }

    fn from_tui_line_with_source_kind(
        line: &crate::AgentTuiPlanLine,
        source_kind: AgentRenderPlanSourceKind,
    ) -> Self {
        let steps = line
            .steps
            .iter()
            .map(AgentRenderPlanStep::from_tui_step)
            .collect::<Vec<_>>();
        Self {
            id: line.id.clone(),
            title: line.title.clone(),
            explanation: line.explanation.clone(),
            source: plan_markdown_source(&line.title, line.explanation.as_deref(), &steps),
            steps,
            source_kind,
        }
    }

    fn refresh_source(&mut self) {
        self.source = plan_markdown_source(&self.title, self.explanation.as_deref(), &self.steps);
    }
}

impl AgentRenderPlanStep {
    fn from_tui_step(step: &crate::AgentTuiPlanStep) -> Self {
        Self {
            id: step.id.clone(),
            status: AgentRenderPlanStatus::from_tui_status(step.status),
            summary: step.summary.clone(),
        }
    }
}

impl AgentRenderPlanStatus {
    fn from_tui_status(status: crate::AgentTuiPlanStatus) -> Self {
        match status {
            crate::AgentTuiPlanStatus::Pending => Self::Pending,
            crate::AgentTuiPlanStatus::Running => Self::Running,
            crate::AgentTuiPlanStatus::Completed => Self::Completed,
            crate::AgentTuiPlanStatus::Failed => Self::Failed,
            crate::AgentTuiPlanStatus::Skipped => Self::Skipped,
        }
    }

    const fn marker(self) -> &'static str {
        match self {
            Self::Pending => PLAN_STATUS_PENDING_MARKER,
            Self::Running => PLAN_STATUS_RUNNING_MARKER,
            Self::Completed => PLAN_STATUS_COMPLETED_MARKER,
            Self::Failed => PLAN_STATUS_FAILED_MARKER,
            Self::Skipped => PLAN_STATUS_SKIPPED_MARKER,
        }
    }
}

fn plan_markdown_source(
    title: &str,
    explanation: Option<&str>,
    steps: &[AgentRenderPlanStep],
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# {}", title.trim()));
    if let Some(explanation) = explanation.map(str::trim).filter(|value| !value.is_empty()) {
        lines.push(String::new());
        lines.push(explanation.to_owned());
    }
    if !steps.is_empty() {
        lines.push(String::new());
        lines.extend(
            steps
                .iter()
                .map(|step| format!("- {} {}", step.status.marker(), step.summary.trim())),
        );
    }
    lines.join("\n")
}
