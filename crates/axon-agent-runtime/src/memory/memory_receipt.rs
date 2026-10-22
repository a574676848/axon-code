//! Memory 回执：统一 memory recall 与 write 的可解释回执。
use axon_domain::{
    AxonResult, MemoryId, MemoryRecallDecision, MemoryRecallPath, MemorySourceOrigin,
    MemoryWriteDecision,
};

/// Memory 操作回执：无论 recall 还是 write，都产出统一的可审计回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryReceipt {
    /// 记忆 ID。
    pub memory_id: MemoryId,
    /// 操作为 recall 或 write。
    pub operation: MemoryReceiptOperation,
    /// 操作结果状态。
    pub status: MemoryReceiptStatus,
    /// 内容摘要短引用，不包含原文。
    pub content_ref: String,
    /// 来源类型。
    pub source_origin: Option<MemorySourceOrigin>,
    /// 可审计原因。
    pub reason: String,
    /// 是否允许远控读取完整内容。
    pub remote_full_content_allowed: bool,
}

/// Memory 回执操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryReceiptOperation {
    /// 召回。
    Recall,
    /// 写入。
    Write,
    /// 遗忘/tombstone。
    Forget,
    /// 隔离。
    Quarantine,
}

/// Memory 回执状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryReceiptStatus {
    /// 成功。
    Ok,
    /// 跳过（不满足准入条件）。
    Skipped,
    /// 隔离。
    Quarantined,
    /// 失败。
    Failed,
}

/// Memory 回执 Runtime：为 memory recall/write 生成统一的可解释回执。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryReceiptRuntime;

impl MemoryReceiptRuntime {
    /// 从召回决策生成回执。
    #[must_use]
    pub fn receipt_for_recall(
        memory_id: MemoryId,
        decision: &MemoryRecallDecision,
        path: MemoryRecallPath,
        content_ref: impl Into<String>,
        source_origin: Option<MemorySourceOrigin>,
    ) -> MemoryReceipt {
        let (status, reason) = match decision {
            MemoryRecallDecision::AllowContext => (
                MemoryReceiptStatus::Ok,
                format!("recall 允许；路径={path:?}"),
            ),
            MemoryRecallDecision::AuditOnly => (
                MemoryReceiptStatus::Skipped,
                "recall 仅允许审计查看，不进入模型上下文".to_owned(),
            ),
            MemoryRecallDecision::Block => {
                (MemoryReceiptStatus::Failed, "recall 被阻断".to_owned())
            }
        };

        MemoryReceipt {
            memory_id,
            operation: MemoryReceiptOperation::Recall,
            status,
            content_ref: content_ref.into(),
            source_origin,
            reason,
            remote_full_content_allowed: false,
        }
    }

    /// 从写入决策生成回执。
    #[must_use]
    pub fn receipt_for_write(
        memory_id: MemoryId,
        decision: MemoryWriteDecision,
        content_ref: impl Into<String>,
        source_origin: Option<MemorySourceOrigin>,
    ) -> MemoryReceipt {
        let (status, reason) = match decision {
            MemoryWriteDecision::WriteStrong => {
                (MemoryReceiptStatus::Ok, "写入 strong memory".to_owned())
            }
            MemoryWriteDecision::WriteWeak => {
                (MemoryReceiptStatus::Ok, "写入 weak memory".to_owned())
            }
            MemoryWriteDecision::Reject => (
                MemoryReceiptStatus::Skipped,
                "写入被准入策略拒绝".to_owned(),
            ),
            MemoryWriteDecision::Quarantine => (
                MemoryReceiptStatus::Quarantined,
                "隔离：疑似包含敏感原文".to_owned(),
            ),
            MemoryWriteDecision::ForgetTombstone => (
                MemoryReceiptStatus::Ok,
                "已生成 tombstone 遗忘记录".to_owned(),
            ),
            MemoryWriteDecision::NeedsUserReview => {
                (MemoryReceiptStatus::Skipped, "需要本机用户复核".to_owned())
            }
        };

        MemoryReceipt {
            memory_id,
            operation: MemoryReceiptOperation::Write,
            status,
            content_ref: content_ref.into(),
            source_origin,
            reason,
            remote_full_content_allowed: false,
        }
    }

    /// 从遗忘/tombstone 操作生成回执。
    #[must_use]
    pub fn receipt_for_forget(
        memory_id: MemoryId,
        source_origin: Option<MemorySourceOrigin>,
    ) -> MemoryReceipt {
        MemoryReceipt {
            memory_id,
            operation: MemoryReceiptOperation::Forget,
            status: MemoryReceiptStatus::Ok,
            content_ref: String::new(),
            source_origin,
            reason: "用户显式遗忘或治理删除".to_owned(),
            remote_full_content_allowed: false,
        }
    }

    /// 从隔离操作生成回执。
    #[must_use]
    pub fn receipt_for_quarantine(
        memory_id: MemoryId,
        content_ref: impl Into<String>,
        source_origin: Option<MemorySourceOrigin>,
    ) -> MemoryReceipt {
        MemoryReceipt {
            memory_id,
            operation: MemoryReceiptOperation::Quarantine,
            status: MemoryReceiptStatus::Quarantined,
            content_ref: content_ref.into(),
            source_origin,
            reason: "治理隔离：疑似包含不安全内容".to_owned(),
            remote_full_content_allowed: false,
        }
    }

    /// 验证回执字段完整性。
    pub fn validate(receipt: &MemoryReceipt) -> AxonResult<()> {
        if receipt.memory_id.0 == 0 {
            return Err(axon_domain::AxonError::runtime_failure(
                "MemoryReceipt memory_id 无效",
            ));
        }
        if receipt.reason.trim().is_empty() {
            return Err(axon_domain::AxonError::runtime_failure(
                "MemoryReceipt reason 不能为空",
            ));
        }
        if receipt.remote_full_content_allowed {
            return Err(axon_domain::AxonError::runtime_failure(
                "MemoryReceipt 不能开放远控全文读取",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "memory_receipt_tests.rs"]
mod tests;
