use axon_domain::{MemoryId, MemoryRecallDecision, MemoryRecallPath, MemoryWriteDecision};

use crate::memory_receipt::{MemoryReceiptOperation, MemoryReceiptRuntime, MemoryReceiptStatus};

#[test]
fn recall_allow_context_generates_ok_receipt() {
    let receipt = MemoryReceiptRuntime::receipt_for_recall(
        MemoryId(1),
        &MemoryRecallDecision::AllowContext,
        MemoryRecallPath::Exact,
        "用户偏好摘要",
        None,
    );
    assert_eq!(receipt.memory_id, MemoryId(1));
    assert_eq!(receipt.status, MemoryReceiptStatus::Ok);
    assert!(matches!(receipt.operation, MemoryReceiptOperation::Recall));
    assert!(!receipt.reason.is_empty());
    assert!(!receipt.remote_full_content_allowed);
    MemoryReceiptRuntime::validate(&receipt).expect("回执验证应通过");
}

#[test]
fn recall_block_generates_failed_receipt() {
    let receipt = MemoryReceiptRuntime::receipt_for_recall(
        MemoryId(2),
        &MemoryRecallDecision::Block,
        MemoryRecallPath::Vector,
        "疑似不安全内容",
        None,
    );
    assert_eq!(receipt.status, MemoryReceiptStatus::Failed);
    assert!(matches!(receipt.operation, MemoryReceiptOperation::Recall));
}

#[test]
fn write_strong_ok_receipt() {
    let receipt = MemoryReceiptRuntime::receipt_for_write(
        MemoryId(3),
        MemoryWriteDecision::WriteStrong,
        "strong 记忆摘要",
        None,
    );
    assert_eq!(receipt.status, MemoryReceiptStatus::Ok);
    assert!(receipt.reason.contains("strong memory"));
}

#[test]
fn quarantine_generates_receipt() {
    let receipt =
        MemoryReceiptRuntime::receipt_for_quarantine(MemoryId(7), "被隔离的内容引用", None);
    assert_eq!(receipt.status, MemoryReceiptStatus::Quarantined);
    assert!(matches!(
        receipt.operation,
        MemoryReceiptOperation::Quarantine
    ));
}

#[test]
fn validate_rejects_invalid_receipt_fields() {
    let zero_id = crate::memory_receipt::MemoryReceipt {
        memory_id: MemoryId(0),
        operation: MemoryReceiptOperation::Recall,
        status: MemoryReceiptStatus::Ok,
        content_ref: String::new(),
        source_origin: None,
        reason: "test".to_owned(),
        remote_full_content_allowed: false,
    };
    assert!(MemoryReceiptRuntime::validate(&zero_id).is_err());

    let remote_full = crate::memory_receipt::MemoryReceipt {
        memory_id: MemoryId(1),
        operation: MemoryReceiptOperation::Recall,
        status: MemoryReceiptStatus::Ok,
        content_ref: String::new(),
        source_origin: None,
        reason: "test".to_owned(),
        remote_full_content_allowed: true,
    };
    assert!(MemoryReceiptRuntime::validate(&remote_full).is_err());
}
