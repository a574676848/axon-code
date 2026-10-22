//! 提问单元渲染事实数据（UQ-203）。

use axon_domain::{UserAnswerItem, UserQuestionItem};

/// TUI 提问单元数据模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderQuestionCell {
    pub(crate) id: String,
    pub(crate) call_id: String,
    pub(crate) status: String,
    pub(crate) questions: Vec<UserQuestionItem>,
    pub(crate) answers: Vec<UserAnswerItem>,
    pub(crate) handoff: Option<String>,
    pub(crate) deadline_at_ms: i64,
}

impl AgentRenderQuestionCell {
    #[must_use]
    pub(crate) fn new(
        id: impl Into<String>,
        call_id: impl Into<String>,
        status: impl Into<String>,
        questions: Vec<UserQuestionItem>,
        answers: Vec<UserAnswerItem>,
        handoff: Option<String>,
        deadline_at_ms: i64,
    ) -> Self {
        Self {
            id: id.into(),
            call_id: call_id.into(),
            status: status.into(),
            questions,
            answers,
            handoff,
            deadline_at_ms,
        }
    }
}
