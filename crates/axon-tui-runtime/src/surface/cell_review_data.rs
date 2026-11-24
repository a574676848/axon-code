//! Review 状态面的结构化渲染数据。

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderReviewCell {
    pub(crate) id: String,
    pub(crate) subject: String,
    pub(crate) status: Option<AgentRenderReviewStatus>,
    pub(crate) details: Vec<AgentRenderReviewDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderReviewDetail {
    pub(crate) kind: AgentRenderReviewDetailKind,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderReviewDetailKind {
    Command,
    Diagnostic,
    Delta,
    Message,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRenderReviewStatus {
    Approved,
    Denied,
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
    Stopped,
    Unknown,
}

const REVIEW_DETAIL_PREFIXES: [&str; 3] = ["工具 ", "状态 ", "错误 "];
const REVIEW_LIFECYCLE_DETAILS: [&str; 21] = [
    "approved",
    "blocked",
    "cancelled",
    "canceled",
    "completed",
    "denied",
    "error",
    "failed",
    "in_progress",
    "interrupted",
    "passed",
    "pending",
    "running",
    "stopped",
    "success",
    "unknown",
    "失败",
    "完成",
    "停止",
    "通过",
    "阻断",
];
const VERIFY_SUMMARY_LABELS: [&str; 3] = ["verify passed:", "verify failed:", "verify stopped:"];

impl AgentRenderReviewCell {
    #[cfg(test)]
    pub(crate) fn new(
        subject: impl Into<String>,
        status: crate::AgentTuiReviewStatus,
        details: Vec<String>,
    ) -> Self {
        let subject = subject.into();
        Self {
            id: format!("test:review-cell:{subject}"),
            subject,
            status: Some(AgentRenderReviewStatus::from_tui_status(status)),
            details: details
                .iter()
                .filter_map(|detail| AgentRenderReviewDetail::normalize(detail))
                .collect(),
        }
    }

    pub(crate) fn from_tui_line(line: &crate::AgentTuiReviewLine) -> Self {
        Self {
            id: line.id.clone(),
            subject: line.subject.clone(),
            status: Some(AgentRenderReviewStatus::from_tui_status(line.status)),
            details: line
                .details
                .iter()
                .filter_map(|detail| AgentRenderReviewDetail::normalize(detail))
                .collect(),
        }
    }

    pub(crate) fn merge_from(&mut self, next: Self) {
        self.id = next.id;
        self.subject = next.subject;
        self.status = next.status;
        for next_detail in next.details {
            if !self.details.iter().any(|detail| detail == &next_detail) {
                self.details.push(next_detail);
            }
        }
    }
}

impl AgentRenderReviewDetail {
    #[cfg(test)]
    pub(crate) fn text(value: impl Into<String>) -> Self {
        Self {
            kind: AgentRenderReviewDetailKind::Message,
            text: value.into(),
        }
    }

    fn normalize(detail: &str) -> Option<Self> {
        let compact = compact_review_detail(detail);
        if compact.is_empty()
            || is_lifecycle_review_detail(&compact)
            || is_verify_machine_summary(&compact)
        {
            return None;
        }
        Some(Self {
            kind: review_detail_kind(&compact),
            text: compact,
        })
    }
}

impl AgentRenderReviewStatus {
    fn from_tui_status(status: crate::AgentTuiReviewStatus) -> Self {
        match status {
            crate::AgentTuiReviewStatus::Approved => Self::Approved,
            crate::AgentTuiReviewStatus::Denied => Self::Denied,
            crate::AgentTuiReviewStatus::Pending => Self::Pending,
            crate::AgentTuiReviewStatus::Running => Self::Running,
            crate::AgentTuiReviewStatus::Completed => Self::Completed,
            crate::AgentTuiReviewStatus::Failed => Self::Failed,
            crate::AgentTuiReviewStatus::Blocked => Self::Blocked,
            crate::AgentTuiReviewStatus::Stopped => Self::Stopped,
            crate::AgentTuiReviewStatus::Unknown => Self::Unknown,
        }
    }
}

fn review_detail_kind(detail: &str) -> AgentRenderReviewDetailKind {
    if detail.starts_with("诊断 ") || detail.starts_with("失败 ") {
        return AgentRenderReviewDetailKind::Delta;
    }
    if detail.contains("cargo ") || detail.contains(" test") || detail.contains(" check") {
        return AgentRenderReviewDetailKind::Command;
    }
    if detail.contains("diagnostic") || detail.contains("诊断") {
        return AgentRenderReviewDetailKind::Diagnostic;
    }
    AgentRenderReviewDetailKind::Message
}

fn compact_review_detail(detail: &str) -> String {
    let trimmed = detail.trim();
    REVIEW_DETAIL_PREFIXES
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))
        .unwrap_or(trimmed)
        .trim()
        .to_owned()
}

fn is_lifecycle_review_detail(detail: &str) -> bool {
    REVIEW_LIFECYCLE_DETAILS
        .iter()
        .any(|status| detail.eq_ignore_ascii_case(status))
}

fn is_verify_machine_summary(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    VERIFY_SUMMARY_LABELS
        .iter()
        .any(|label| lower.starts_with(label))
        && lower.contains(" status=")
}

#[cfg(test)]
mod tests {
    use super::{AgentRenderReviewCell, AgentRenderReviewDetailKind, AgentRenderReviewStatus};
    use crate::AgentTuiReviewStatus;

    #[test]
    fn review_detail_normalization_drops_lifecycle_noise() {
        let cell = AgentRenderReviewCell::new(
            "验证",
            AgentTuiReviewStatus::Completed,
            vec![
                "工具 lsp_diagnostics".to_owned(),
                "状态 completed".to_owned(),
                "错误 diagnostic".to_owned(),
                "verify passed: status=passed convergence=clean cycles=1 diagnostics=0 failing_steps=0"
                    .to_owned(),
            ],
        );

        assert_eq!(
            cell.details
                .iter()
                .map(|detail| detail.text.as_str())
                .collect::<Vec<_>>(),
            vec!["lsp_diagnostics", "diagnostic"]
        );
        assert_eq!(
            cell.details[1].kind,
            AgentRenderReviewDetailKind::Diagnostic
        );
        assert_eq!(cell.status, Some(AgentRenderReviewStatus::Completed));
    }

    #[test]
    fn stopped_verify_machine_summary_stays_quiet_even_with_reason() {
        let cell = AgentRenderReviewCell::new(
            "停止 · 停滞",
            AgentTuiReviewStatus::Stopped,
            vec![
                "verify stopped: status=stopped convergence=stalled cycles=3 diagnostics=0 failing_steps=0 reason=收敛护栏"
                    .to_owned(),
                "stopped".to_owned(),
            ],
        );

        assert!(cell.details.is_empty());
        assert_eq!(cell.status, Some(AgentRenderReviewStatus::Stopped));
    }

    #[test]
    fn stopped_verify_without_reason_stays_quiet() {
        let cell = AgentRenderReviewCell::new(
            "停止 · 停滞",
            AgentTuiReviewStatus::Stopped,
            vec![
                "verify stopped: status=stopped convergence=stalled cycles=3 diagnostics=0 failing_steps=0"
                    .to_owned(),
                "停止".to_owned(),
            ],
        );

        assert!(cell.details.is_empty());
    }
}
