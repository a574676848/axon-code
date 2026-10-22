use axon_domain::{
    AttachmentId, AttachmentKind, AttachmentPromptPolicy, AttachmentRef, AttachmentReviewState,
    AttachmentSensitivity, AttachmentSource, CellId, ExecutionRequestId, OutputRefId, RemoteAction,
    RemoteCapabilityPolicy, RemoteDenialReason, SessionId,
};

use super::{
    REMOTE_ARTIFACT_REASON_DIRECT_FILE_READ_CLOSED, REMOTE_ARTIFACT_REASON_PROJECTED,
    REMOTE_ARTIFACT_REASON_RAW_CONTENT_CLOSED, RemoteArtifactKind, RemoteArtifactReference,
    RemoteArtifactViewBlockReason, RemoteArtifactViewDecision, RemoteArtifactViewRequest,
    RemoteArtifactViewRuntime, RemoteOutputRefGrant,
};

#[test]
fn remote_artifact_view_projects_output_and_screenshot_without_raw_access() {
    let runtime = RemoteArtifactViewRuntime::new(RemoteCapabilityPolicy::default_sidecar());

    let output = runtime.view(&RemoteArtifactViewRequest {
        session_id: session(),
        cell_id: cell(),
        kind: RemoteArtifactKind::CommandOutput,
        reference: RemoteArtifactReference::OutputGrant(output_grant()),
        requested_max_bytes: 128,
        request_raw_content: false,
        now_ms: 1_000,
    });
    let screenshot = runtime.view(&RemoteArtifactViewRequest {
        session_id: session(),
        cell_id: cell(),
        kind: RemoteArtifactKind::Screenshot,
        reference: RemoteArtifactReference::ScreenshotAttachment(screenshot_ref()),
        requested_max_bytes: 512,
        request_raw_content: false,
        now_ms: 1_000,
    });

    assert_closed(output, RemoteArtifactKind::CommandOutput);
    assert_closed(screenshot, RemoteArtifactKind::Screenshot);
}

#[test]
fn remote_artifact_view_fails_closed_for_raw_mismatch_expiry_and_policy() {
    let runtime = RemoteArtifactViewRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let mut raw_request = output_request();
    raw_request.request_raw_content = true;
    let mut kind_mismatch = output_request();
    kind_mismatch.kind = RemoteArtifactKind::Screenshot;
    let mut expired = output_request();
    expired.now_ms = 3_001;
    let denied_policy = RemoteArtifactViewRuntime::new(RemoteCapabilityPolicy {
        allow_bounded_summary: true,
        allow_attachment_upload: true,
        allow_output_ref_grant: false,
        allow_thread_control: true,
    });

    assert_eq!(
        runtime.view(&raw_request),
        RemoteArtifactViewDecision::Block {
            reason: RemoteArtifactViewBlockReason::RawContentDenied,
        }
    );
    assert_eq!(
        runtime.view(&kind_mismatch),
        RemoteArtifactViewDecision::Block {
            reason: RemoteArtifactViewBlockReason::KindMismatch,
        }
    );
    assert_eq!(
        runtime.view(&expired),
        RemoteArtifactViewDecision::Block {
            reason: RemoteArtifactViewBlockReason::GrantExpired,
        }
    );
    assert_eq!(
        denied_policy.view(&output_request()),
        RemoteArtifactViewDecision::Block {
            reason: RemoteArtifactViewBlockReason::PolicyDenied,
        }
    );
    assert_eq!(
        denied_policy
            .view(&output_request())
            .remote_denial_action_hint(),
        Some((
            RemoteAction::ReadOutputRef,
            RemoteDenialReason::NotGrantedByPolicy
        ))
    );
}

fn assert_closed(decision: RemoteArtifactViewDecision, expected_kind: RemoteArtifactKind) {
    let RemoteArtifactViewDecision::Allow { view } = decision else {
        panic!("远控产物查看应返回 bounded view");
    };

    assert_eq!(view.kind, expected_kind);
    assert!(!view.permissions.raw_content_allowed);
    assert!(!view.permissions.session_transcript_allowed);
    assert!(!view.permissions.direct_file_read_allowed);
    assert!(!view.permissions.raw_image_bytes_allowed);
    assert!(
        view.reasons
            .contains(&REMOTE_ARTIFACT_REASON_PROJECTED.to_owned())
    );
    assert!(
        view.reasons
            .contains(&REMOTE_ARTIFACT_REASON_RAW_CONTENT_CLOSED.to_owned())
    );
    assert!(
        view.reasons
            .contains(&REMOTE_ARTIFACT_REASON_DIRECT_FILE_READ_CLOSED.to_owned())
    );
}

trait RemoteArtifactViewDecisionHint {
    fn remote_denial_action_hint(&self) -> Option<(RemoteAction, RemoteDenialReason)>;
}

impl RemoteArtifactViewDecisionHint for RemoteArtifactViewDecision {
    fn remote_denial_action_hint(&self) -> Option<(RemoteAction, RemoteDenialReason)> {
        match self {
            RemoteArtifactViewDecision::Block {
                reason: RemoteArtifactViewBlockReason::PolicyDenied,
            } => Some((
                RemoteAction::ReadOutputRef,
                RemoteDenialReason::NotGrantedByPolicy,
            )),
            _ => None,
        }
    }
}

fn output_request() -> RemoteArtifactViewRequest {
    RemoteArtifactViewRequest {
        session_id: session(),
        cell_id: cell(),
        kind: RemoteArtifactKind::CommandOutput,
        reference: RemoteArtifactReference::OutputGrant(output_grant()),
        requested_max_bytes: 128,
        request_raw_content: false,
        now_ms: 1_000,
    }
}

fn output_grant() -> RemoteOutputRefGrant {
    RemoteOutputRefGrant {
        output_ref_id: OutputRefId(9),
        session_id: session(),
        cell_id: cell(),
        execution_request_id: ExecutionRequestId(80),
        sha256: "b".repeat(64),
        source_trace: "tool://shell/cargo-test".to_owned(),
        max_bytes: 512,
        expires_at_ms: 3_000,
        bounded_preview: "test result preview".to_owned(),
        session_transcript_allowed: false,
        reasons: vec!["fixture".to_owned()],
    }
}

fn screenshot_ref() -> AttachmentRef {
    AttachmentRef {
        id: AttachmentId(12),
        source: AttachmentSource::Screenshot,
        kind: AttachmentKind::Image,
        mime_type: "image/png".to_owned(),
        byte_len: 4_096,
        sha256: "d".repeat(64),
        preview: None,
        source_trace: "screenshot://window/7/session/7/pane/3".to_owned(),
        sensitivity: AttachmentSensitivity::Normal,
        review_state: AttachmentReviewState::Ready,
        prompt_policy: AttachmentPromptPolicy::RefOnly,
    }
}

const fn session() -> SessionId {
    SessionId(7)
}

const fn cell() -> CellId {
    CellId(17)
}
