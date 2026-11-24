//! Work Ledger Result 领域模型。

use super::{ExecutionId, GoalId, ResultId};
use serde::{Deserialize, Serialize};

/// Goal 结果直接通知的 Unicode 字符上限；超出部分通过结果文档交付。
pub const GOAL_RESULT_SUMMARY_MAX_CHARS: usize = 400;

/// Result 错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultErrorKind {
    /// Provider 或协议错误。
    Provider,
    /// 权限或安全错误。
    Permission,
    /// 外部副作用结果未知。
    Uncertain,
    /// 运行时错误。
    Runtime,
}

/// Goal Execution 的持久结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultRecord {
    /// Result 标识。
    pub result_id: ResultId,
    /// 所属 Goal。
    pub goal_id: GoalId,
    /// 产生结果的 Execution。
    pub execution_id: ExecutionId,
    /// 是否成功。
    pub succeeded: bool,
    /// 脱敏摘要。
    pub summary: String,
    /// 工具/副作用回执引用。
    pub receipt_refs: Vec<String>,
    /// 产物引用。
    pub artifact_refs: Vec<String>,
    /// 失败分类。
    pub error_kind: Option<ResultErrorKind>,
    /// 产生时间。
    pub created_at_ms: u64,
}

impl ResultRecord {
    /// 创建成功结果。
    pub fn success(
        result_id: ResultId,
        goal_id: GoalId,
        execution_id: ExecutionId,
        summary: impl Into<String>,
        now_ms: u64,
    ) -> Self {
        Self {
            result_id,
            goal_id,
            execution_id,
            succeeded: true,
            summary: summary.into(),
            receipt_refs: Vec::new(),
            artifact_refs: Vec::new(),
            error_kind: None,
            created_at_ms: now_ms,
        }
    }

    /// 创建失败结果。
    pub fn failure(
        result_id: ResultId,
        goal_id: GoalId,
        execution_id: ExecutionId,
        summary: impl Into<String>,
        error_kind: ResultErrorKind,
        now_ms: u64,
    ) -> Self {
        Self {
            result_id,
            goal_id,
            execution_id,
            succeeded: false,
            summary: summary.into(),
            receipt_refs: Vec::new(),
            artifact_refs: Vec::new(),
            error_kind: Some(error_kind),
            created_at_ms: now_ms,
        }
    }
}
