//! Agent Pane apply 回执历史投影。

use axon_domain::ApplyExecutionStatus;
use serde::{Deserialize, Serialize};

use crate::AgentApplyExecutionProjection;

/// Agent Pane apply 历史 schema version。
pub const AGENT_PANE_APPLY_HISTORY_SCHEMA_VERSION: u32 = 1;

/// Agent Pane apply 回执历史记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPaneApplyHistoryRecord {
    /// schema version。
    pub schema_version: u32,
    /// pane ID。
    pub pane_id: u64,
    /// RuntimeCell ID。
    pub cell_id: u64,
    /// 关联的 Axon run session ID；旧记录可为空。
    #[serde(default)]
    pub session_id: Option<String>,
    /// 目标路径。
    pub path: String,
    /// 执行关联请求 ID。
    pub execution_request_id: u64,
    /// 执行状态。
    pub status: String,
    /// 展示摘要。
    pub summary: String,
    /// 来源引用。
    pub source_ref: String,
    /// materialize audit 是否存在。
    pub materialize_audit_present: bool,
    /// materialize 后 hash。
    pub post_hash: Option<String>,
    /// materialize 前 hash。
    #[serde(default)]
    pub pre_hash: Option<String>,
    /// 本机回滚备份文件路径。
    #[serde(default)]
    pub backup_path: Option<String>,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否允许远控读取完整内容。
    pub remote_full_content_allowed: bool,
}

impl AgentPaneApplyHistoryRecord {
    /// 从 apply 回执投影和 materialize 审计构造历史记录。
    #[must_use]
    pub fn from_projection(
        pane_id: u64,
        session_id: Option<&str>,
        projection: &AgentApplyExecutionProjection,
        post_hash: Option<String>,
        recorded_at_ms: u64,
        pre_hash: Option<String>,
        backup_path: Option<String>,
    ) -> Self {
        Self {
            schema_version: AGENT_PANE_APPLY_HISTORY_SCHEMA_VERSION,
            pane_id,
            cell_id: projection.cell_id,
            session_id: session_id.map(ToOwned::to_owned),
            path: projection.path.clone(),
            execution_request_id: projection.execution_request_id.0,
            status: apply_status_code(projection.status).to_owned(),
            summary: projection.summary.clone(),
            source_ref: projection.source_ref.clone(),
            materialize_audit_present: post_hash.is_some(),
            post_hash,
            pre_hash,
            backup_path,
            recorded_at_ms,
            remote_trigger_allowed: false,
            remote_full_content_allowed: false,
        }
    }
}

fn apply_status_code(status: ApplyExecutionStatus) -> &'static str {
    match status {
        ApplyExecutionStatus::WriteRequestReady => "writeRequestReady",
        ApplyExecutionStatus::ClientWriteCompleted => "clientWriteCompleted",
        ApplyExecutionStatus::ClientWriteFailed => "clientWriteFailed",
        ApplyExecutionStatus::Blocked => "blocked",
    }
}
