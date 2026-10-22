use std::path::PathBuf;

use axon_domain::ToolCallId;

use super::agent_run_tool_result_wire_test_fixtures::{FixtureToolReceipt, receipt, tool_call};
use crate::{
    AgentRunToolDecisionFactsInput, AgentRunToolDecisionInput, AgentRunToolOutputPolicy,
    AgentRunToolResultReceipt, AgentTurnToolDecision, ProviderToolCallExecutionStatus,
    agent_run_changed_file_paths_from_receipts, agent_run_provider_receipt_status_name,
    agent_run_receipts_have_post_apply_diagnostics_errors, agent_run_tool_decision,
    agent_run_tool_decision_from_facts, agent_run_tool_receipt_completed_from_facts,
    agent_run_tool_receipt_to_execution_outcome, agent_run_tool_receipt_to_provider_receipt,
    agent_run_tool_receipt_write_scan, agent_run_tool_receipt_write_scan_from_facts,
    agent_run_tool_receipts_to_provider_receipt_json, agent_run_tool_result_receipts_from_facts,
};

#[test]
fn tool_receipt_facts_are_projected_to_runtime_receipts() {
    let receipts = agent_run_tool_result_receipts_from_facts(&[FixtureToolReceipt {
        provider_call_id: "call-1".to_owned(),
        tool_name: "builtin.edit".to_owned(),
        status: "failed".to_owned(),
        summary: "patch denied".to_owned(),
        metadata: Some(serde_json::json!({"path": "src/lib.rs"})),
        output_ref: None,
        error_type: Some("blocked".to_owned()),
        remote_trigger_allowed: true,
        filesystem_write_performed: false,
    }]);

    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].provider_call_id, "call-1");
    assert_eq!(receipts[0].tool_name, "builtin.edit");
    assert_eq!(receipts[0].status, "failed");
    assert_eq!(
        receipts[0].metadata.as_ref().expect("metadata")["path"],
        "src/lib.rs"
    );
    assert_eq!(receipts[0].error_type.as_deref(), Some("blocked"));
    assert!(receipts[0].remote_trigger_allowed);
    assert!(!receipts[0].filesystem_write_performed);
}

#[test]
fn tool_decision_projects_continue_with_runtime_messages() {
    let decision = agent_run_tool_decision(AgentRunToolDecisionInput {
        tool_calls: &[tool_call("call-1")],
        receipts: vec![receipt("call-1")],
        reasoning_content: Some("reasoning"),
        output_policy: AgentRunToolOutputPolicy::default(),
    })
    .expect("tool decision 应可生成");

    let AgentTurnToolDecision::Continue {
        receipts,
        next_messages,
    } = decision
    else {
        panic!("应继续下一轮");
    };

    assert_eq!(receipts.len(), 1);
    assert_eq!(next_messages.len(), 2);
}

#[test]
fn tool_decision_from_facts_preserves_adapter_receipts() {
    let source = FixtureToolReceipt {
        provider_call_id: "call-1".to_owned(),
        tool_name: "builtin.search".to_owned(),
        status: "completed".to_owned(),
        summary: "ok".to_owned(),
        metadata: None,
        output_ref: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    };

    let decision = agent_run_tool_decision_from_facts(AgentRunToolDecisionFactsInput {
        tool_calls: &[tool_call("call-1")],
        receipts: vec![source],
        reasoning_content: None,
        output_policy: AgentRunToolOutputPolicy::default(),
    })
    .expect("tool decision 应可生成");

    let AgentTurnToolDecision::Continue {
        receipts,
        next_messages,
        ..
    } = decision
    else {
        panic!("应继续下一轮");
    };

    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].provider_call_id, "call-1");
    assert_eq!(next_messages.len(), 2);
}

#[test]
fn failed_runtime_tool_receipt_remains_a_continue_decision() {
    let mut failed = receipt("call-1");
    failed.tool_name = "builtin.shell_command".to_owned();
    failed.status = "failed".to_owned();
    failed.summary = "command failed".to_owned();
    failed.error_type = Some("execution_failed".to_owned());

    let decision = agent_run_tool_decision(AgentRunToolDecisionInput {
        tool_calls: &[tool_call("call-1")],
        receipts: vec![failed],
        reasoning_content: None,
        output_policy: AgentRunToolOutputPolicy::default(),
    })
    .expect("运行时失败回执应可结构化回传");

    assert!(matches!(decision, AgentTurnToolDecision::Continue { .. }));
}

#[test]
fn post_apply_diagnostics_and_changed_paths_are_extracted_from_receipts() {
    let mut source = receipt("call-1");
    source.filesystem_write_performed = true;
    source.metadata = Some(serde_json::json!({
        "path": "src/lib.rs",
        "postApplyDiagnosticsErrors": [{"message": "boom"}],
    }));
    let ignored = AgentRunToolResultReceipt {
        filesystem_write_performed: false,
        metadata: Some(serde_json::json!({
            "path": "src/ignored.rs",
            "postApplyDiagnosticsErrors": [{"message": "ignored"}],
        })),
        ..receipt("call-2")
    };

    assert!(agent_run_receipts_have_post_apply_diagnostics_errors(&[
        source.clone(),
        ignored.clone()
    ]));
    assert_eq!(
        agent_run_changed_file_paths_from_receipts(&[source, ignored]),
        vec![PathBuf::from("src/lib.rs")]
    );
}

#[test]
fn write_scan_from_facts_projects_changed_files() {
    let source = FixtureToolReceipt {
        provider_call_id: "call-1".to_owned(),
        tool_name: "builtin.edit".to_owned(),
        status: "completed".to_owned(),
        summary: "ok".to_owned(),
        metadata: Some(serde_json::json!({
            "affectedPaths": ["src/main.rs", "src/lib.rs"],
        })),
        output_ref: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: true,
    };

    let scan = agent_run_tool_receipt_write_scan_from_facts(&[source]);

    assert!(scan.has_filesystem_write);
    assert_eq!(
        scan.changed_files,
        vec![PathBuf::from("src/lib.rs"), PathBuf::from("src/main.rs")]
    );
}

#[test]
fn write_scan_projects_last_write_and_changed_files() {
    let read = receipt("call-1");
    let mut first_write = receipt("call-2");
    first_write.filesystem_write_performed = true;
    first_write.metadata = Some(serde_json::json!({"file_path": "src/lib.rs"}));
    let mut last_write = receipt("call-3");
    last_write.filesystem_write_performed = true;
    last_write.metadata = Some(serde_json::json!({
        "path": "src/main.rs",
        "postApplyDiagnosticsErrors": [{"message": "boom"}],
        "affectedPaths": ["src/main.rs", "src/lib.rs"],
    }));

    let scan = agent_run_tool_receipt_write_scan(&[read, first_write, last_write]);

    assert!(scan.has_filesystem_write);
    assert_eq!(scan.last_write_receipt_index, Some(2));
    assert!(scan.post_apply_diagnostics_errors_detected);
    assert_eq!(
        scan.changed_files,
        vec![PathBuf::from("src/lib.rs"), PathBuf::from("src/main.rs"),]
    );
}

#[test]
fn write_scan_reports_no_write_for_empty_or_read_only_receipts() {
    let scan = agent_run_tool_receipt_write_scan(&[receipt("call-1")]);

    assert!(!scan.has_filesystem_write);
    assert_eq!(scan.last_write_receipt_index, None);
    assert!(!scan.post_apply_diagnostics_errors_detected);
    assert!(scan.changed_files.is_empty());
}

#[test]
fn tool_receipt_projects_to_provider_receipt_status() {
    let mut source = receipt("call-1");
    source.status = "blocked".to_owned();
    source.summary = "denied".to_owned();
    source.error_type = Some("blocked".to_owned());
    source.remote_trigger_allowed = true;
    source.filesystem_write_performed = true;

    let mapped = agent_run_tool_receipt_to_provider_receipt(2, &source);

    assert_eq!(mapped.dispatch_order, 2);
    assert_eq!(mapped.tool_call_id.0, 3);
    assert_eq!(mapped.execution_request_id.0, 3);
    assert_eq!(mapped.status, ProviderToolCallExecutionStatus::Failed);
    assert_eq!(mapped.bounded_summary, "denied");
    assert_eq!(mapped.error_type.as_deref(), Some("blocked"));
    assert!(mapped.remote_trigger_allowed);
    assert!(mapped.filesystem_write_performed);
    assert_eq!(agent_run_provider_receipt_status_name(&mapped), "blocked");
}

#[test]
fn tool_receipts_project_to_provider_receipt_json() {
    let mut source = receipt("call-1");
    source.status = "blocked".to_owned();
    source.summary = "denied".to_owned();
    source.metadata = Some(serde_json::json!({"path": "src/lib.rs"}));
    source.error_type = Some("blocked".to_owned());
    source.remote_trigger_allowed = true;
    source.filesystem_write_performed = true;

    let receipts = agent_run_tool_receipts_to_provider_receipt_json(&[source]);

    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].dispatch_order, 0);
    assert_eq!(receipts[0].tool_call_id, 1);
    assert_eq!(receipts[0].execution_request_id, 1);
    assert_eq!(receipts[0].status, "blocked");
    assert_eq!(receipts[0].bounded_summary, "denied");
    assert_eq!(
        receipts[0].metadata.as_ref().expect("metadata")["path"],
        "src/lib.rs"
    );
    assert_eq!(receipts[0].error_type.as_deref(), Some("blocked"));
    assert!(receipts[0].remote_trigger_allowed);
    assert!(receipts[0].filesystem_write_performed);
}

#[test]
fn tool_receipt_completed_from_facts_uses_runtime_status_contract() {
    fn fixture_receipt(status: &str) -> FixtureToolReceipt {
        FixtureToolReceipt {
            provider_call_id: "call-1".to_owned(),
            tool_name: "builtin.search".to_owned(),
            status: status.to_owned(),
            summary: "ok".to_owned(),
            metadata: None,
            output_ref: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
        }
    }

    let completed = fixture_receipt("completed");
    let failed = fixture_receipt("failed");
    let runtime_completed = receipt("call-2");

    assert!(agent_run_tool_receipt_completed_from_facts(&completed));
    assert!(!agent_run_tool_receipt_completed_from_facts(&failed));
    assert!(agent_run_tool_receipt_completed_from_facts(
        &runtime_completed
    ));
}

#[test]
fn tool_receipt_projects_to_execution_outcome() {
    let mut source = receipt("call-1");
    source.status = "failed".to_owned();
    source.summary = "boom".to_owned();
    source.error_type = Some("execution_failed".to_owned());

    let outcome = agent_run_tool_receipt_to_execution_outcome(ToolCallId(7), &source, 42);

    assert_eq!(outcome.tool_call_id, ToolCallId(7));
    assert_eq!(outcome.status, ProviderToolCallExecutionStatus::Failed);
    assert_eq!(outcome.bounded_summary, "boom");
    assert_eq!(outcome.error_type.as_deref(), Some("execution_failed"));
    assert_eq!(outcome.completed_at_ms, 42);
}
