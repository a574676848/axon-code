use axon_agent_runtime::{ExecutionEvidenceKind, ExecutionEvidenceStatus, ProviderToolCall};

use super::{build_execution_evidence, selected_skill_execution};
use crate::run_commands::run_tool_loop::RunToolReceipt;

#[test]
fn filesystem_write_requires_later_validation_receipt() {
    let receipts = vec![
        receipt("call-edit", "builtin.edit", true, "completed"),
        receipt("call-test", "builtin.shell_command", false, "completed"),
    ];
    let evidence = build_execution_evidence(&receipts);

    assert_eq!(
        evidence.records[0].kind,
        ExecutionEvidenceKind::ArtifactWrite
    );
    assert_eq!(
        evidence.records[0].status,
        ExecutionEvidenceStatus::Verified
    );
    assert_eq!(evidence.records[0].verification_refs, vec!["receipt:2"]);
}

#[test]
fn failed_validation_marks_artifact_write_contradicted() {
    let receipts = vec![
        receipt("call-edit", "builtin.edit", true, "completed"),
        receipt("call-test", "builtin.shell_command", false, "failed"),
    ];
    let evidence = build_execution_evidence(&receipts);

    assert_eq!(
        evidence.records[0].status,
        ExecutionEvidenceStatus::Contradicted
    );
    assert!(evidence.records[0].verification_refs.is_empty());
}

#[test]
fn unvalidated_artifact_write_is_executed_but_not_verified() {
    let receipts = vec![receipt("call-edit", "builtin.edit", true, "completed")];
    let evidence = build_execution_evidence(&receipts);

    assert_eq!(
        evidence.records[0].status,
        ExecutionEvidenceStatus::Executed
    );
    assert!(!evidence.has_verified_execution());
}

#[test]
fn selected_skill_requires_matching_invocation_and_later_execution() {
    let calls = vec![ProviderToolCall::new(
        "call-skill",
        "builtin.invoke_skill",
        r#"{"skill_name":"release-notes"}"#,
    )];
    let receipts = vec![
        receipt("call-skill", "builtin.invoke_skill", false, "completed"),
        receipt("call-test", "builtin.shell_command", false, "completed"),
    ];
    let evidence = build_execution_evidence(&receipts);
    let selected = selected_skill_execution("release-notes", &calls, &receipts, &evidence);

    assert!(selected.execution_verified);
    assert_eq!(selected.evidence_refs, vec!["receipt:1", "receipt:2"]);
}

#[test]
fn selected_skill_without_goal_execution_is_not_verified() {
    let calls = vec![ProviderToolCall::new(
        "call-skill",
        "builtin.invoke_skill",
        r#"{"skill_name":"release-notes"}"#,
    )];
    let receipts = vec![receipt(
        "call-skill",
        "builtin.invoke_skill",
        false,
        "completed",
    )];
    let evidence = build_execution_evidence(&receipts);
    let selected = selected_skill_execution("release-notes", &calls, &receipts, &evidence);

    assert!(!selected.execution_verified);
}

fn receipt(id: &str, tool_name: &str, wrote: bool, status: &str) -> RunToolReceipt {
    RunToolReceipt {
        tool_call_id: 1,
        part_id: format!("part:{id}"),
        provider_call_id: id.to_owned(),
        tool_name: tool_name.to_owned(),
        status: status.to_owned(),
        summary: "fixture".to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: wrote,
        output_ref: None,
        duration_ms: None,
    }
}
