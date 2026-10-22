use axon_domain::{AxonError, RemoteAction, RemoteCapabilityPolicy, RemoteDenialReason, SessionId};

use super::{
    REMOTE_CONTROL_REASON_BUSY_QUEUED, REMOTE_CONTROL_REASON_CONTROL_BYPASS,
    REMOTE_CONTROL_REASON_DUPLICATE_DROPPED, REMOTE_CONTROL_REASON_IDLE_DISPATCH,
    RemoteControlCommand, RemoteControlDedupeStatus, RemoteControlDeliveryStatus,
    RemoteControlDispatchAction, RemoteControlInboxRuntime, RemoteControlMessageInput,
};

#[test]
fn remote_control_commands_bypass_busy_message_queue() {
    let plan = runtime().plan(&input("/status", true)).unwrap();

    assert_eq!(plan.action, RemoteControlDispatchAction::InlineControl);
    assert_eq!(plan.command, Some(RemoteControlCommand::Status));
    assert_eq!(plan.command_argument, None);
    assert!(
        plan.reasons
            .contains(&REMOTE_CONTROL_REASON_CONTROL_BYPASS.to_owned())
    );
}

#[test]
fn remote_control_stop_commands_are_thread_control() {
    for raw_text in ["/stop", "停止", "/cancel", "/interrupt"] {
        let plan = runtime().plan(&input(raw_text, true)).unwrap();

        assert_eq!(plan.action, RemoteControlDispatchAction::InlineControl);
        assert_eq!(plan.command, Some(RemoteControlCommand::Stop));
        assert!(plan.receipt.local_execution_required);
        assert!(!plan.receipt.queued);
    }
}

#[test]
fn remote_control_busy_plain_message_is_queued_not_interrupted() {
    let plan = runtime().plan(&input("继续说明当前状态", true)).unwrap();

    assert_eq!(plan.action, RemoteControlDispatchAction::QueueUserMessage);
    assert_eq!(plan.command, None);
    assert_eq!(plan.command_argument, None);
    assert_eq!(plan.receipt.status, RemoteControlDeliveryStatus::Queued);
    assert_eq!(plan.receipt.dedupe_status, RemoteControlDedupeStatus::New);
    assert!(plan.receipt.queued);
    assert!(plan.receipt.local_execution_required);
    assert!(
        plan.reasons
            .contains(&REMOTE_CONTROL_REASON_BUSY_QUEUED.to_owned())
    );
    assert_eq!(plan.bounded_preview, "继续说明当前状态");
}

#[test]
fn remote_control_idle_plain_message_returns_inline_delivery_receipt() {
    let plan = runtime().plan(&input("继续推进实现", false)).unwrap();

    assert_eq!(
        plan.action,
        RemoteControlDispatchAction::DispatchUserMessage
    );
    assert_eq!(plan.command, None);
    assert_eq!(plan.receipt.delivery_id, "delivery-1");
    assert_eq!(
        plan.receipt.status,
        RemoteControlDeliveryStatus::AcceptedInline
    );
    assert_eq!(plan.receipt.dedupe_status, RemoteControlDedupeStatus::New);
    assert!(!plan.receipt.queued);
    assert!(plan.receipt.local_execution_required);
    assert!(
        plan.receipt
            .reasons
            .contains(&REMOTE_CONTROL_REASON_IDLE_DISPATCH.to_owned())
    );
}

#[test]
fn unknown_control_command_is_dispatched_as_plain_message() {
    let plan = runtime().plan(&input("/archive req-19", true)).unwrap();

    assert_eq!(plan.action, RemoteControlDispatchAction::QueueUserMessage);
    assert_eq!(plan.command, None);
    assert_eq!(plan.command_argument, None);
    assert_eq!(plan.receipt.status, RemoteControlDeliveryStatus::Queued);
    assert!(plan.receipt.queued);
    assert!(plan.receipt.local_execution_required);
    assert!(
        plan.reasons
            .contains(&REMOTE_CONTROL_REASON_BUSY_QUEUED.to_owned())
    );
}

#[test]
fn remote_control_duplicate_delivery_is_dropped_idempotently() {
    let mut input = input("/archive req-1", true);
    input.seen_delivery_ids = vec!["delivery-1".to_owned()];

    let plan = runtime().plan(&input).unwrap();

    assert_eq!(plan.action, RemoteControlDispatchAction::DropDuplicate);
    assert_eq!(plan.command, None);
    assert_eq!(
        plan.receipt.status,
        RemoteControlDeliveryStatus::DuplicateDropped
    );
    assert_eq!(
        plan.receipt.dedupe_status,
        RemoteControlDedupeStatus::Duplicate
    );
    assert!(!plan.receipt.local_execution_required);
    assert_eq!(
        plan.receipt.reasons,
        vec![REMOTE_CONTROL_REASON_DUPLICATE_DROPPED.to_owned()],
    );
}

#[test]
fn remote_control_stop_command_respects_thread_control_policy_gate() {
    let policy = RemoteCapabilityPolicy {
        allow_bounded_summary: true,
        allow_attachment_upload: true,
        allow_output_ref_grant: true,
        allow_thread_control: false,
    };
    let runtime = RemoteControlInboxRuntime::new(policy);

    let denied = runtime.plan(&input("/stop", true));

    assert_eq!(
        denied,
        Err(AxonError::RemoteDenied {
            action: RemoteAction::ControlThread,
            reason: RemoteDenialReason::NotGrantedByPolicy,
        })
    );
}

fn runtime() -> RemoteControlInboxRuntime {
    RemoteControlInboxRuntime::new(RemoteCapabilityPolicy::default_sidecar())
}

fn input(raw_text: &str, session_busy: bool) -> RemoteControlMessageInput {
    RemoteControlMessageInput {
        delivery_id: "delivery-1".to_owned(),
        session_id: SessionId(7),
        raw_text: raw_text.to_owned(),
        session_busy,
        seen_delivery_ids: Vec::new(),
    }
}
