//! `axon run` 进度展示模型。

use std::collections::BTreeMap;
use std::path::PathBuf;

use axon_domain::ToolExecutionPermission;
use axon_protocol::{diff::FileChange, wire::WireLifecycleStatus};

/// TUI 可展示的生命周期状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunLifecycleStatusView {
    /// 稳定 lifecycle part id。
    pub(crate) id: String,
    /// 状态标题。
    pub(crate) title: String,
    /// 稳定事件类别。
    pub(crate) kind: String,
    /// 用户可读摘要。
    pub(crate) summary: String,
    /// 生命周期阶段。
    pub(crate) phase: String,
    /// 生命周期状态。
    pub(crate) status: WireLifecycleStatus,
    /// 相关工具或模型名；ProviderRetry 使用模型名，TUI 负责隐藏供应商前缀。
    pub(crate) tool_name: Option<String>,
    /// provider transport 尝试序号。
    pub(crate) attempt_index: Option<usize>,
    /// 运行单元开始时间戳，毫秒。
    pub(crate) started_at_ms: Option<u64>,
    /// 真实运行耗时，毫秒。
    pub(crate) duration_ms: Option<u64>,
    /// 真实 token 消耗总量。
    pub(crate) total_tokens: Option<u64>,
    /// 缓存读取占完整输入的命中率，单位为 basis points。
    pub(crate) cache_hit_rate_basis_points: Option<u16>,
}

impl RunLifecycleStatusView {
    pub(crate) fn with_id(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: impl Into<String>,
        summary: impl Into<String>,
        phase: impl Into<String>,
        attempt_index: Option<usize>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind: kind.into(),
            summary: summary.into(),
            phase: phase.into(),
            status: WireLifecycleStatus::Running,
            tool_name: None,
            attempt_index,
            started_at_ms: None,
            duration_ms: None,
            total_tokens: None,
            cache_hit_rate_basis_points: None,
        }
    }

    pub(crate) fn new(
        title: impl Into<String>,
        kind: impl Into<String>,
        summary: impl Into<String>,
        phase: impl Into<String>,
        attempt_index: Option<usize>,
    ) -> Self {
        let kind = kind.into();
        let attempt_suffix =
            attempt_index.map_or_else(String::new, |attempt| format!(":{attempt}"));
        Self::with_id(
            format!("lifecycle:{kind}{attempt_suffix}"),
            title,
            kind,
            summary,
            phase,
            attempt_index,
        )
    }

    pub(crate) fn with_tool_name(mut self, tool_name: impl Into<String>) -> Self {
        let tool_name = tool_name.into();
        if !tool_name.trim().is_empty() {
            self.tool_name = Some(tool_name);
        }
        self
    }

    pub(crate) fn completed(mut self) -> Self {
        self.status = WireLifecycleStatus::Completed;
        self
    }

    pub(crate) fn failed(mut self) -> Self {
        self.status = WireLifecycleStatus::Failed;
        self
    }

    pub(crate) const fn with_started_at_ms(mut self, started_at_ms: u64) -> Self {
        self.started_at_ms = Some(started_at_ms);
        self
    }

    pub(crate) const fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub(crate) const fn with_total_tokens(mut self, total_tokens: u64) -> Self {
        self.total_tokens = Some(total_tokens);
        self
    }

    pub(crate) const fn with_cache_hit_rate_basis_points(mut self, value: u16) -> Self {
        self.cache_hit_rate_basis_points = Some(value);
        self
    }
}

/// TUI 可展示的工具完成视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunToolCompletionView {
    /// provider tool call id。
    pub(crate) provider_call_id: String,
    /// transcript part ID。
    pub(crate) part_id: String,
    /// 工具名称。
    pub(crate) tool_name: String,
    /// 工具类型。
    pub(crate) kind: &'static str,
    /// 执行状态。
    pub(crate) status: String,
    /// 工具结果摘要。
    pub(crate) summary: String,
    /// 工具结构化 metadata。
    pub(crate) metadata: Option<serde_json::Value>,
    /// 错误类型。
    pub(crate) error_type: Option<String>,
    /// 是否执行了文件系统写入。
    pub(crate) filesystem_write_performed: bool,
    /// 真实工具执行耗时，单位毫秒。
    pub(crate) duration_ms: Option<u64>,
}

/// TUI 可展示的工具执行视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunToolExecutionView {
    /// 审批请求 ID。
    pub(crate) request_id: u64,
    /// provider tool call id。
    pub(crate) provider_call_id: String,
    /// transcript part ID。
    pub(crate) part_id: String,
    /// 工具名称。
    pub(crate) tool_name: String,
    /// 工具类型。
    pub(crate) kind: &'static str,
    /// 工具参数预览。
    pub(crate) arguments_preview: String,
    /// 模型自述的调用目的概要；仅用于展示。
    pub(crate) intent: Option<String>,
    /// 本次调用的最终执行权限。
    pub(crate) execution_permission: ToolExecutionPermission,
}

/// TUI 可展示的工具 diff 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunToolDiffSnapshotView {
    /// provider tool call id。
    pub(crate) provider_call_id: String,
    /// 被该 diff 快照关联的 tool transcript part ID。
    pub(crate) tool_part_id: String,
    /// 文件变更列表。
    pub(crate) changes: BTreeMap<PathBuf, FileChange>,
}

/// TUI 可展示的工具 stdout/stderr 增量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunToolOutputChunk {
    /// provider tool call id。
    pub(crate) provider_call_id: String,
    /// transcript part ID。
    pub(crate) part_id: String,
    /// 工具名称。
    pub(crate) tool_name: String,
    /// 工具类型。
    pub(crate) kind: &'static str,
    /// stdout 或 stderr。
    pub(crate) stream: &'static str,
    /// 输出序号。
    pub(crate) sequence: u64,
    /// 输出文本。
    pub(crate) text: String,
}

/// TUI 可展示的工具进度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunToolProgressView {
    /// provider tool call id。
    pub(crate) provider_call_id: String,
    /// transcript part ID。
    pub(crate) part_id: String,
    /// 工具名称。
    pub(crate) tool_name: String,
    /// 工具类型。
    pub(crate) kind: &'static str,
    /// 稳定阶段。
    pub(crate) phase: String,
    /// 已完成项。
    pub(crate) items_completed: Option<u64>,
    /// 总项数。
    pub(crate) items_total: Option<u64>,
    /// 当前项。
    pub(crate) current_item: Option<String>,
    /// 工具执行耗时。
    pub(crate) elapsed_ms: Option<u64>,
    /// 简短说明。
    pub(crate) message: Option<String>,
}
