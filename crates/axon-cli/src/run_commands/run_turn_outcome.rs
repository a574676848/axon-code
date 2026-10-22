//! Post-run 反思使用的有界事实包构造器。

use axon_agent_runtime::{
    ExecutionEvidenceIndex, ExecutionEvidenceKind, ExecutionEvidenceRecord,
    ExecutionEvidenceStatus, ProviderToolCall, SelectedSkillExecution, TurnFinalStatus,
    TurnOutcomeEnvelope, agent_run_tool_receipt_completed_from_facts,
};

use crate::stable_ids::stable_numeric_id;

use super::{RunRequest, run_execution::RunResponse, run_tool_loop::RunToolReceipt};

const TURN_OUTCOME_TEXT_MAX_CHARS: usize = 2_000;
const INVOKE_SKILL_TOOL_NAME: &str = "builtin.invoke_skill";
const MEMORY_SEARCH_TOOL_NAME: &str = "builtin.memory_search";
const MEMORY_WRITE_TOOL_NAME: &str = "builtin.memory_write";
const VALIDATION_TOOL_NAMES: &[&str] = &[
    "builtin.read_file",
    "builtin.search",
    "builtin.glob",
    "builtin.shell_command",
];

pub(super) fn build_turn_outcome(
    request: &RunRequest,
    response: &RunResponse,
) -> TurnOutcomeEnvelope {
    let execution_evidence = build_execution_evidence(&response.tool_receipts);
    let selected_skill = request.selected_skill_name.as_ref().map(|skill_name| {
        selected_skill_execution(
            skill_name,
            &response.tool_calls,
            &response.tool_receipts,
            &execution_evidence,
        )
    });
    TurnOutcomeEnvelope {
        conversation_id: request.session_id.clone(),
        turn_id: response.assistant_part_id.clone(),
        agent_path: request.current_agent_path.to_string(),
        user_intent_summary: bounded_text(&request.prompt),
        assistant_outcome_summary: bounded_text(&response.content),
        user_message_ref: format!(
            "message:user:{}",
            stable_numeric_id(&format!("{}:{}", request.session_id, request.prompt))
        ),
        execution_evidence,
        selected_skill,
        final_status: TurnFinalStatus::Completed,
    }
}

pub(super) fn build_execution_evidence(receipts: &[RunToolReceipt]) -> ExecutionEvidenceIndex {
    let mut records = receipts
        .iter()
        .enumerate()
        .map(|(index, receipt)| evidence_record(index, receipt))
        .collect::<Vec<_>>();
    for index in 0..records.len() {
        if records[index].kind != ExecutionEvidenceKind::ArtifactWrite
            || !records[index].runtime_verified
        {
            continue;
        }
        let verification_refs = records
            .iter()
            .enumerate()
            .filter(|(validation_index, record)| {
                *validation_index > index
                    && record.kind == ExecutionEvidenceKind::Validation
                    && record.status == ExecutionEvidenceStatus::Verified
            })
            .map(|(_, record)| record.evidence_ref.clone())
            .collect::<Vec<_>>();
        let has_failed_validation = records
            .iter()
            .enumerate()
            .any(|(validation_index, record)| {
                validation_index > index
                    && record.kind == ExecutionEvidenceKind::Validation
                    && record.status == ExecutionEvidenceStatus::Attempted
            });
        records[index].status = if verification_refs.is_empty() && has_failed_validation {
            ExecutionEvidenceStatus::Contradicted
        } else if verification_refs.is_empty() {
            ExecutionEvidenceStatus::Executed
        } else {
            ExecutionEvidenceStatus::Verified
        };
        records[index].verification_refs = verification_refs;
    }
    ExecutionEvidenceIndex { records }
}

fn evidence_record(index: usize, receipt: &RunToolReceipt) -> ExecutionEvidenceRecord {
    let completed = agent_run_tool_receipt_completed_from_facts(receipt);
    let kind = evidence_kind(receipt);
    ExecutionEvidenceRecord {
        evidence_ref: receipt_ref(index),
        tool_name: receipt.tool_name.clone(),
        status: if !completed {
            ExecutionEvidenceStatus::Attempted
        } else if kind == ExecutionEvidenceKind::Validation {
            ExecutionEvidenceStatus::Verified
        } else {
            ExecutionEvidenceStatus::Executed
        },
        kind,
        runtime_verified: completed,
        verification_refs: Vec::new(),
    }
}

fn evidence_kind(receipt: &RunToolReceipt) -> ExecutionEvidenceKind {
    if receipt.tool_name == INVOKE_SKILL_TOOL_NAME {
        ExecutionEvidenceKind::SkillInvocation
    } else if receipt.filesystem_write_performed {
        ExecutionEvidenceKind::ArtifactWrite
    } else if VALIDATION_TOOL_NAMES.contains(&receipt.tool_name.as_str()) {
        ExecutionEvidenceKind::Validation
    } else {
        ExecutionEvidenceKind::ToolExecution
    }
}

fn selected_skill_execution(
    selected_skill_name: &str,
    calls: &[ProviderToolCall],
    receipts: &[RunToolReceipt],
    evidence: &ExecutionEvidenceIndex,
) -> SelectedSkillExecution {
    let invocation_index = receipts.iter().enumerate().find_map(|(index, receipt)| {
        (receipt.tool_name == INVOKE_SKILL_TOOL_NAME
            && agent_run_tool_receipt_completed_from_facts(receipt)
            && call_invokes_skill(calls, &receipt.provider_call_id, selected_skill_name))
        .then_some(index)
    });
    let execution_refs = invocation_index.map_or_else(Vec::new, |invocation_index| {
        evidence
            .records
            .iter()
            .enumerate()
            .filter(|(index, record)| {
                *index > invocation_index
                    && record.runtime_verified
                    && is_goal_execution_evidence(record)
                    && record.status == ExecutionEvidenceStatus::Verified
            })
            .map(|(_, record)| record.evidence_ref.clone())
            .collect::<Vec<_>>()
    });
    let execution_verified = invocation_index.is_some() && !execution_refs.is_empty();
    let mut evidence_refs = invocation_index
        .map(receipt_ref)
        .into_iter()
        .collect::<Vec<_>>();
    evidence_refs.extend(execution_refs);
    SelectedSkillExecution {
        skill_name: selected_skill_name.to_owned(),
        execution_verified,
        evidence_refs,
    }
}

fn call_invokes_skill(calls: &[ProviderToolCall], call_id: &str, skill_name: &str) -> bool {
    calls.iter().any(|call| {
        call.id == call_id
            && call.dispatch_name() == INVOKE_SKILL_TOOL_NAME
            && call
                .input
                .as_json()
                .and_then(|value| value.get("skill_name"))
                .and_then(serde_json::Value::as_str)
                == Some(skill_name)
    })
}

fn is_goal_execution_evidence(record: &ExecutionEvidenceRecord) -> bool {
    record.kind != ExecutionEvidenceKind::SkillInvocation
        && !matches!(
            record.tool_name.as_str(),
            MEMORY_SEARCH_TOOL_NAME | MEMORY_WRITE_TOOL_NAME
        )
}

fn receipt_ref(index: usize) -> String {
    format!("receipt:{}", index.saturating_add(1))
}

fn bounded_text(value: &str) -> String {
    value.chars().take(TURN_OUTCOME_TEXT_MAX_CHARS).collect()
}

#[cfg(test)]
#[path = "run_turn_outcome_tests.rs"]
mod tests;
