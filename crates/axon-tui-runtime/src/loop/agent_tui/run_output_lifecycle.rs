//! TUI lifecycle、plan、review 状态模型。

/// Runtime 生命周期状态行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiStatusLine {
    /// 稳定 status/command part id。
    pub id: String,
    /// 状态标题。
    pub title: String,
    /// 结构化状态条目。
    pub items: Vec<AgentTuiStatusItem>,
}

/// Runtime 生命周期状态条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiStatusItem {
    /// 事件类别。
    pub kind: String,
    /// 用户可读摘要。
    pub summary: String,
    /// 所属 turn。
    pub turn_id: Option<u64>,
    /// 生命周期阶段。
    pub phase: Option<AgentTuiLifecyclePhase>,
    /// 工具名。
    pub tool_name: Option<String>,
    /// 耗时毫秒。
    pub duration_ms: Option<u64>,
}

/// Runtime 生成生命周期状态行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiLifecycleLine {
    /// 稳定 lifecycle part id。
    pub id: String,
    /// 生命周期标题。
    pub title: String,
    /// 生命周期条目。
    pub items: Vec<AgentTuiLifecycleStatusView>,
}

/// 模型 reasoning 输出行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiReasoningLine {
    /// reasoning id。
    pub id: String,
    /// reasoning 文本。
    pub text: String,
}

/// 历史与上下文生命周期行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiHistoryLine {
    /// 稳定 history part id。
    pub id: String,
    /// 历史标题。
    pub title: String,
    /// 历史条目。
    pub items: Vec<AgentTuiHistoryItem>,
}

/// 历史与上下文生命周期条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiHistoryItem {
    /// 条目名称。
    pub name: String,
    /// 条目类别标签。
    pub kind_label: Option<String>,
    /// 条目状态标签。
    pub status_label: Option<String>,
}

/// 工具生命周期行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolLifecycleLine {
    /// 稳定 tool lifecycle event part id。
    pub id: String,
    /// 被该 lifecycle 更新的 tool transcript part ID。
    pub tool_part_id: String,
    /// provider tool call id。
    pub provider_call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 工具语义类型。
    pub kind: super::run_output_models::AgentTuiToolKind,
    /// 生命周期状态。
    pub status: String,
    /// 简短摘要。
    pub summary: String,
    /// 参数预览。
    pub arguments_preview: Option<String>,
    /// 输出预览。
    pub output_preview: Option<String>,
}

/// 工具生命周期行创建输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolLifecycleLineInput {
    /// 稳定 lifecycle event id。
    pub id: String,
    /// 被更新的 transcript part id。
    pub tool_part_id: String,
    /// Provider 工具调用 id。
    pub provider_call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 工具语义类型。
    pub kind: super::run_output_models::AgentTuiToolKind,
    /// 生命周期状态。
    pub status: String,
    /// 工具摘要。
    pub summary: String,
    /// 参数预览。
    pub arguments_preview: Option<String>,
    /// 输出预览。
    pub output_preview: Option<String>,
}

/// 计划或自修复循环行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiPlanLine {
    /// 稳定 plan part id。
    pub id: String,
    /// 计划标题。
    pub title: String,
    /// 计划更新说明。
    pub explanation: Option<String>,
    /// 计划步骤。
    pub steps: Vec<AgentTuiPlanStep>,
}

/// 计划进度快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentTuiPlanProgress {
    /// 已完成步骤数。
    pub completed: usize,
    /// 总步骤数。
    pub total: usize,
}

/// 计划步骤。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiPlanStep {
    /// 稳定步骤 ID；普通 Markdown plan 可为空。
    pub id: Option<String>,
    /// 步骤状态。
    pub status: AgentTuiPlanStatus,
    /// 步骤摘要。
    pub summary: String,
    /// 循环序号。
    pub cycle: Option<u32>,
    /// 诊断数量。
    pub diagnostic_count: usize,
    /// 失败步骤数量。
    pub failing_step_count: usize,
}

/// TUI 计划步骤状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiPlanStatus {
    /// 等待执行。
    Pending,
    /// 正在执行。
    Running,
    /// 已完成。
    Completed,
    /// 执行失败。
    Failed,
    /// 已跳过。
    Skipped,
}

/// 审阅或验证停止产物行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiReviewLine {
    /// 稳定 review part id。
    pub id: String,
    /// 审阅状态。
    pub status: AgentTuiReviewStatus,
    /// 审阅主题。
    pub subject: String,
    /// 审阅明细。
    pub details: Vec<String>,
}

/// TUI 审阅状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiReviewStatus {
    /// 已批准。
    Approved,
    /// 已拒绝。
    Denied,
    /// 等待审阅。
    Pending,
    /// 审阅中。
    Running,
    /// 审阅完成。
    Completed,
    /// 审阅失败。
    Failed,
    /// 审阅受阻。
    Blocked,
    /// 审阅停止。
    Stopped,
    /// 未识别状态。
    Unknown,
}

/// TUI 生命周期状态视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiLifecycleStatusView {
    /// 稳定 lifecycle event id。
    pub id: String,
    /// 状态标题。
    pub title: String,
    /// 稳定事件类别。
    pub kind: String,
    /// 用户可读摘要。
    pub summary: String,
    /// 生命周期阶段。
    pub phase: AgentTuiLifecyclePhase,
    /// 关联工具名。
    pub tool_name: Option<String>,
    /// 生命周期耗时毫秒。
    pub duration_ms: Option<u64>,
    /// provider transport 尝试序号。
    pub attempt_index: Option<usize>,
}

/// 生命周期状态视图创建输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiLifecycleStatusViewInput {
    /// 稳定状态 id。
    pub id: String,
    /// 状态标题。
    pub title: String,
    /// 状态类别。
    pub kind: String,
    /// 状态摘要。
    pub summary: String,
    /// 生命周期阶段。
    pub phase: String,
    /// 关联工具名。
    pub tool_name: Option<String>,
    /// 耗时毫秒。
    pub duration_ms: Option<u64>,
    /// 尝试序号。
    pub attempt_index: Option<usize>,
}

/// TUI 生命周期阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiLifecyclePhase {
    /// provider streaming 阶段。
    Streaming,
    /// 模型生成阶段。
    Generation,
    /// 普通状态或命令反馈。
    Notice,
    /// 命令面反馈。
    Command,
    /// 未识别阶段。
    Unknown,
}

impl AgentTuiStatusLine {
    /// 创建带稳定 id 的 runtime 状态行。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        title: impl Into<String>,
        items: Vec<AgentTuiStatusItem>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            items,
        }
    }

    /// 创建测试 fixture runtime 状态行。
    #[cfg(test)]
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<AgentTuiStatusItem>) -> Self {
        let title = title.into();
        Self::with_id(format!("test:status:{title}"), title, items)
    }
}

impl AgentTuiStatusItem {
    /// 创建 runtime 状态条目。
    #[must_use]
    pub fn new(
        kind: impl Into<String>,
        summary: impl Into<String>,
        turn_id: Option<u64>,
        phase: Option<String>,
        tool_name: Option<String>,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            kind: kind.into(),
            summary: summary.into(),
            turn_id,
            phase: phase.as_deref().map(AgentTuiLifecyclePhase::from_raw),
            tool_name,
            duration_ms,
        }
    }

    /// 从生命周期 view 创建普通状态条目。
    #[must_use]
    pub fn from_lifecycle_view(view: AgentTuiLifecycleStatusView) -> Self {
        Self {
            kind: view.kind,
            summary: view.summary,
            turn_id: None,
            phase: Some(view.phase),
            tool_name: None,
            duration_ms: None,
        }
    }
}

impl AgentTuiLifecycleLine {
    /// 创建带稳定 id 的 runtime 生命周期行。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        title: impl Into<String>,
        items: Vec<AgentTuiLifecycleStatusView>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            items,
        }
    }

    /// 创建测试 fixture 生命周期行。
    #[cfg(test)]
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<AgentTuiLifecycleStatusView>) -> Self {
        let title = title.into();
        Self::with_id(format!("test:lifecycle-line:{title}"), title, items)
    }
}

impl AgentTuiReasoningLine {
    /// 创建 reasoning 行。
    #[must_use]
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

impl AgentTuiHistoryLine {
    /// 创建带稳定 id 的历史行。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        title: impl Into<String>,
        items: Vec<AgentTuiHistoryItem>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            items,
        }
    }

    /// 创建测试 fixture 历史行。
    #[cfg(test)]
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<AgentTuiHistoryItem>) -> Self {
        let title = title.into();
        Self::with_id(format!("history:test:{title}"), title, items)
    }
}

impl AgentTuiHistoryItem {
    /// 创建历史与上下文生命周期条目。
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        kind_label: Option<String>,
        status_label: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind_label,
            status_label,
        }
    }
}

impl AgentTuiToolLifecycleLine {
    /// 创建带稳定 part id 的工具生命周期行。
    #[must_use]
    pub fn with_id(input: AgentTuiToolLifecycleLineInput) -> Self {
        Self {
            id: input.id,
            tool_part_id: input.tool_part_id,
            provider_call_id: input.provider_call_id,
            tool_name: input.tool_name,
            kind: input.kind,
            status: input.status,
            summary: input.summary,
            arguments_preview: input.arguments_preview,
            output_preview: input.output_preview,
        }
    }
}

impl AgentTuiPlanLine {
    /// 创建带稳定 id 的计划行。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        title: impl Into<String>,
        explanation: Option<String>,
        steps: Vec<AgentTuiPlanStep>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            explanation: normalize_optional_text(explanation),
            steps,
        }
    }

    /// 创建测试 fixture 计划行。
    #[cfg(test)]
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        explanation: Option<String>,
        steps: Vec<AgentTuiPlanStep>,
    ) -> Self {
        let title = title.into();
        Self::with_id(format!("test:plan:{title}"), title, explanation, steps)
    }

    /// 计算计划步骤进度。
    #[must_use]
    pub fn progress(&self) -> Option<AgentTuiPlanProgress> {
        let total = self.steps.len();
        if total == 0 {
            return None;
        }
        let completed = self
            .steps
            .iter()
            .filter(|step| step.status == AgentTuiPlanStatus::Completed)
            .count();
        Some(AgentTuiPlanProgress { completed, total })
    }
}

impl AgentTuiPlanStep {
    /// 创建计划步骤。
    #[must_use]
    pub fn new(
        status: impl AsRef<str>,
        summary: impl Into<String>,
        cycle: Option<u32>,
        diagnostic_count: usize,
        failing_step_count: usize,
    ) -> Self {
        Self {
            id: None,
            status: AgentTuiPlanStatus::from_raw(status.as_ref()),
            summary: summary.into(),
            cycle,
            diagnostic_count,
            failing_step_count,
        }
    }

    /// 创建带稳定 ID 的计划步骤。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        status: impl AsRef<str>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: Some(id.into()),
            status: AgentTuiPlanStatus::from_raw(status.as_ref()),
            summary: summary.into(),
            cycle: None,
            diagnostic_count: 0,
            failing_step_count: 0,
        }
    }
}

impl AgentTuiReviewLine {
    /// 创建带稳定 id 的审阅行。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        status: impl AsRef<str>,
        subject: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status: AgentTuiReviewStatus::from_raw(status.as_ref()),
            subject: subject.into(),
            details,
        }
    }

    /// 创建测试 fixture 审阅行。
    #[cfg(test)]
    #[must_use]
    pub fn new(status: impl AsRef<str>, subject: impl Into<String>, details: Vec<String>) -> Self {
        let subject = subject.into();
        Self::with_id(format!("test:review:{subject}"), status, subject, details)
    }
}

impl AgentTuiLifecycleStatusView {
    /// 创建带稳定 id 的生命周期状态视图。
    #[must_use]
    pub fn with_id(input: AgentTuiLifecycleStatusViewInput) -> Self {
        Self {
            id: input.id,
            title: input.title,
            kind: input.kind,
            summary: input.summary,
            phase: AgentTuiLifecyclePhase::from_raw(&input.phase),
            tool_name: input.tool_name,
            duration_ms: input.duration_ms,
            attempt_index: input.attempt_index,
        }
    }

    /// 创建测试 fixture 生命周期状态视图。
    #[cfg(test)]
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        kind: impl Into<String>,
        summary: impl Into<String>,
        phase: impl AsRef<str>,
        tool_name: Option<String>,
        duration_ms: Option<u64>,
        attempt_index: Option<usize>,
    ) -> Self {
        let kind = kind.into();
        Self::with_id(AgentTuiLifecycleStatusViewInput {
            id: format!("test:lifecycle-event:{kind}"),
            title: title.into(),
            kind,
            summary: summary.into(),
            phase: phase.as_ref().to_owned(),
            tool_name,
            duration_ms,
            attempt_index,
        })
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(normalize_text)
}

fn normalize_text(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

impl AgentTuiPlanStatus {
    /// 从运行时或 JSON 边界的原始状态值归一为 TUI 计划状态。
    #[must_use]
    pub fn from_raw(status: &str) -> Self {
        match status {
            "completed" | "success" | "passed" => Self::Completed,
            "running" | "in_progress" | "active" => Self::Running,
            "failed" | "error" | "blocked" => Self::Failed,
            "skipped" | "cancelled" | "canceled" => Self::Skipped,
            _ => Self::Pending,
        }
    }
}

impl AgentTuiReviewStatus {
    /// 从运行时或 JSON 边界的原始状态值归一为 TUI 审阅状态。
    #[must_use]
    pub fn from_raw(status: &str) -> Self {
        match status {
            "approved" => Self::Approved,
            "denied" => Self::Denied,
            "pending" => Self::Pending,
            "running" | "in_progress" | "active" => Self::Running,
            "completed" | "success" | "passed" => Self::Completed,
            "failed" | "error" => Self::Failed,
            "blocked" => Self::Blocked,
            "stopped" | "interrupted" | "cancelled" | "canceled" => Self::Stopped,
            _ => Self::Unknown,
        }
    }
}

impl AgentTuiLifecyclePhase {
    /// 从运行时或 JSON 边界的原始阶段值归一为 TUI 生命周期阶段。
    #[must_use]
    pub fn from_raw(phase: &str) -> Self {
        match phase {
            "streaming" | "StreamingStarted" | "streaming_started" => Self::Streaming,
            "generation" | "Generation" => Self::Generation,
            "notice" | "Notice" => Self::Notice,
            "command" | "Command" => Self::Command,
            _ => Self::Unknown,
        }
    }
}
