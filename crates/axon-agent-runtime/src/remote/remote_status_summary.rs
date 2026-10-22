//! 远控状态摘要投影。
//!
//! 该模块只聚合已有结构化投影字段，远控侧车不能借此读取完整上下文或完整回答。

use axon_domain::{
    AxonResult, RemoteAction, RemoteCapabilityPolicy, RemoteSuggestionSummary, RuntimeCellSnapshot,
    SessionId,
};

use crate::MemoryFooterProjection;

/// 远控状态摘要文本最大字符数。
pub const REMOTE_STATUS_SUMMARY_TEXT_MAX_CHARS: usize = 160;
/// 远控状态摘要已完成原因。
pub const REMOTE_STATUS_REASON_PROJECTED: &str = "remote_status_summary:projected";
/// 远控状态摘要禁止完整上下文原因。
pub const REMOTE_STATUS_REASON_FULL_CONTEXT_CLOSED: &str =
    "remote_status_summary:full_context_closed";

/// 远控状态摘要输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStatusSummaryInput {
    /// 目标 session。
    pub session_id: SessionId,
    /// 健康摘要。
    pub health_label: String,
    /// 当前 runtime cell 快照。
    pub runtime_cell: Option<RuntimeCellSnapshot>,
    /// Memory footer 投影。
    pub memory_footer: Option<MemoryFooterProjection>,
    /// 远控建议摘要。
    pub suggestion: Option<RemoteSuggestionSummary>,
}

/// 远控状态摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStatusSummary {
    /// 目标 session。
    pub session_id: SessionId,
    /// 健康摘要。
    pub health: RemoteHealthSummary,
    /// Cell 摘要。
    pub runtime_cell: Option<RemoteRuntimeCellSummary>,
    /// Memory 摘要。
    pub memory: Option<RemoteMemorySummary>,
    /// 建议摘要。
    pub suggestion: Option<RemoteSuggestionSummary>,
    /// 是否允许远控读取完整上下文。
    pub remote_full_context_allowed: bool,
    /// 是否发生截断。
    pub truncated: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 健康摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHealthSummary {
    /// 有界标签。
    pub label: String,
    /// 是否被截断。
    pub truncated: bool,
}

/// RuntimeCell 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRuntimeCellSummary {
    /// Cell ID。
    pub id: axon_domain::CellId,
    /// 状态文本。
    pub state: String,
    /// 输出排空状态文本。
    pub output_drain: String,
    /// 关闭状态文本。
    pub closure: String,
    /// 事件数量。
    pub event_count: usize,
}

/// Memory 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMemorySummary {
    /// strong 记忆数。
    pub strong_count: usize,
    /// weak 记忆数。
    pub weak_count: usize,
    /// 隔离记忆数。
    pub quarantined_count: usize,
    /// tombstone 数。
    pub tombstone_count: usize,
    /// 拒绝数。
    pub rejected_count: usize,
    /// 待本机复核数。
    pub pending_review_count: usize,
    /// footer 有界文案。
    pub footer_text: String,
    /// 是否被截断。
    pub truncated: bool,
}

/// 远控状态摘要 runtime。
#[derive(Debug, Clone)]
pub struct RemoteStatusSummaryRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteStatusSummaryRuntime {
    /// 创建 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 生成远控状态摘要。
    ///
    /// # Errors
    ///
    /// 当远控策略未允许 bounded summary 时返回错误。
    pub fn project(&self, input: &RemoteStatusSummaryInput) -> AxonResult<RemoteStatusSummary> {
        self.policy
            .ensure_allowed(RemoteAction::ReadBoundedSummary)?;

        let health = project_health(&input.health_label);
        let runtime_cell = input.runtime_cell.as_ref().map(project_runtime_cell);
        let memory = input.memory_footer.as_ref().map(project_memory);
        let suggestion = input.suggestion.clone();
        let truncated = health.truncated || memory.as_ref().is_some_and(|item| item.truncated);

        Ok(RemoteStatusSummary {
            session_id: input.session_id,
            health,
            runtime_cell,
            memory,
            suggestion,
            remote_full_context_allowed: false,
            truncated,
            reasons: vec![
                REMOTE_STATUS_REASON_PROJECTED.to_owned(),
                REMOTE_STATUS_REASON_FULL_CONTEXT_CLOSED.to_owned(),
            ],
        })
    }
}

fn project_health(label: &str) -> RemoteHealthSummary {
    let (label, truncated) = bounded_text(label);
    RemoteHealthSummary { label, truncated }
}

fn project_runtime_cell(snapshot: &RuntimeCellSnapshot) -> RemoteRuntimeCellSummary {
    RemoteRuntimeCellSummary {
        id: snapshot.id,
        state: format!("{:?}", snapshot.state),
        output_drain: format!("{:?}", snapshot.output_drain),
        closure: format!("{:?}", snapshot.closure),
        event_count: snapshot.event_count,
    }
}

fn project_memory(projection: &MemoryFooterProjection) -> RemoteMemorySummary {
    let (footer_text, truncated) = bounded_text(&projection.footer_text);
    RemoteMemorySummary {
        strong_count: projection.strong_count,
        weak_count: projection.weak_count,
        quarantined_count: projection.quarantined_count,
        tombstone_count: projection.tombstone_count,
        rejected_count: projection.rejected_count,
        pending_review_count: projection.pending_review_count,
        footer_text,
        truncated,
    }
}

fn bounded_text(value: &str) -> (String, bool) {
    let bounded = value
        .chars()
        .take(REMOTE_STATUS_SUMMARY_TEXT_MAX_CHARS)
        .collect::<String>();
    let truncated = value.chars().count() > REMOTE_STATUS_SUMMARY_TEXT_MAX_CHARS;
    (bounded, truncated)
}
