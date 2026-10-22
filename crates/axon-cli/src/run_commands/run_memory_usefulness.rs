use serde::{Deserialize, Serialize};

use axon_agent_runtime::agent_run_tool_receipt_completed_from_facts;
use axon_memory_read::{MemoryTurnContext, memory_turn_citation_report};

use super::{
    RunRequest,
    run_execution::{RunExecutionError, RunResponse, RunToolReceipt},
    run_memory_recall::RunMemoryRecallEvent,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunMemoryUsefulnessEvent {
    pub(super) hit_count: usize,
    pub(super) cited_hit_count: usize,
    pub(super) adopted_hit_count: usize,
    pub(super) cited_hit_refs: Vec<String>,
    pub(super) adopted_hit_refs: Vec<String>,
    pub(super) skill_reflection_hit_count: usize,
    pub(super) skill_reflection_success_count: usize,
    pub(super) procedural_review_hit_count: usize,
    pub(super) tool_call_count: usize,
    pub(super) filesystem_write_count: usize,
    pub(super) failed_tool_count: usize,
    pub(super) unused_hit_count: usize,
    pub(super) negative_feedback_hit_count: usize,
    pub(super) used_hit_basis_points: u16,
    pub(super) tool_reduction_proxy_basis_points: u16,
    pub(super) repeated_error_avoidance_basis_points: u16,
    pub(super) false_recall_risk_basis_points: u16,
    pub(super) downrank_recommended_hit_refs: Vec<String>,
    pub(super) needs_review_hit_refs: Vec<String>,
    pub(super) tool_guidance: Vec<String>,
    pub(super) remote_payload_allowed: bool,
    pub(super) prompt_text_persisted: bool,
    pub(super) raw_payload_persisted: bool,
}
pub(super) fn memory_usefulness_for_completed(
    request: &RunRequest,
    response: &RunResponse,
) -> Option<RunMemoryUsefulnessEvent> {
    let recall = request.memory_recall.as_ref()?;
    let adopted = adopted_hits(request, &response.content, &response.tool_receipts);
    let failed_tool_count = failed_tool_count(&response.tool_receipts);
    Some(memory_usefulness_event(
        request,
        recall,
        adopted,
        &response.tool_receipts,
        failed_tool_count,
        true,
    ))
}
pub(super) fn memory_usefulness_for_failed(
    request: &RunRequest,
    error: &RunExecutionError,
) -> Option<RunMemoryUsefulnessEvent> {
    let recall = request.memory_recall.as_ref()?;
    Some(memory_usefulness_event(
        request,
        recall,
        AdoptedHits::default(),
        &error.tool_receipts,
        failed_tool_count(&error.tool_receipts),
        false,
    ))
}
pub(super) fn memory_tool_guidance(request: &RunRequest) -> Vec<String> {
    request
        .memory_recall
        .as_ref()
        .map_or_else(Vec::new, |recall| recall.tool_guidance.clone())
}

fn memory_usefulness_event(
    request: &RunRequest,
    recall: &RunMemoryRecallEvent,
    adopted: AdoptedHits,
    tool_receipts: &[RunToolReceipt],
    failed_tool_count: usize,
    run_completed: bool,
) -> RunMemoryUsefulnessEvent {
    let cited_hit_count = adopted.cited_hit_refs.len();
    let adopted_hit_count = adopted.adopted_hit_refs.len();
    let skill_reflection_hit_count = recall.skill_reflection_hit_count;
    let skill_reflection_success_count =
        usize::from(run_completed && skill_reflection_hit_count > 0);
    let tool_call_count = tool_receipts.len();
    let filesystem_write_count = tool_receipts
        .iter()
        .filter(|receipt| receipt.filesystem_write_performed)
        .count();
    let unused_hit_count = recall.hit_count.saturating_sub(adopted_hit_count);
    let downrank_recommended_hit_refs =
        downrank_recommended_hit_refs(recall, cited_hit_count, failed_tool_count, run_completed);
    RunMemoryUsefulnessEvent {
        hit_count: recall.hit_count,
        cited_hit_count,
        adopted_hit_count,
        cited_hit_refs: adopted.cited_hit_refs,
        adopted_hit_refs: adopted.adopted_hit_refs,
        skill_reflection_hit_count,
        skill_reflection_success_count,
        procedural_review_hit_count: recall.procedural_review_hit_count,
        tool_call_count,
        filesystem_write_count,
        failed_tool_count,
        unused_hit_count,
        negative_feedback_hit_count: downrank_recommended_hit_refs.len(),
        used_hit_basis_points: basis_points(adopted_hit_count, recall.hit_count),
        tool_reduction_proxy_basis_points: tool_reduction_proxy(recall.hit_count, tool_call_count),
        repeated_error_avoidance_basis_points: repeated_error_avoidance(
            recall,
            failed_tool_count,
            run_completed,
        ),
        false_recall_risk_basis_points: false_recall_risk(
            recall.hit_count,
            cited_hit_count,
            failed_tool_count,
            run_completed,
        ),
        downrank_recommended_hit_refs,
        needs_review_hit_refs: needs_review_hit_refs(recall),
        tool_guidance: memory_tool_guidance(request),
        remote_payload_allowed: false,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
    }
}

fn downrank_recommended_hit_refs(
    recall: &RunMemoryRecallEvent,
    cited_hit_count: usize,
    failed_tool_count: usize,
    run_completed: bool,
) -> Vec<String> {
    let negative = !run_completed || failed_tool_count > 0 || cited_hit_count == 0;
    if negative {
        recall
            .trace
            .iter()
            .filter(|hit| !hit.tool_guidance_applied || cited_hit_count == 0)
            .map(|hit| hit.content_ref.clone())
            .collect()
    } else {
        Vec::new()
    }
}

fn needs_review_hit_refs(recall: &RunMemoryRecallEvent) -> Vec<String> {
    recall
        .trace
        .iter()
        .filter(|hit| hit.tool_guidance_applied && hit.score_basis_points < 6_000)
        .map(|hit| hit.content_ref.clone())
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AdoptedHits {
    cited_hit_refs: Vec<String>,
    adopted_hit_refs: Vec<String>,
}

fn adopted_hits(request: &RunRequest, content: &str, receipts: &[RunToolReceipt]) -> AdoptedHits {
    let mut haystack = content.to_owned();
    for receipt in receipts {
        haystack.push('\n');
        haystack.push_str(&receipt.summary);
    }
    let Some(recall) = request.memory_recall.as_ref() else {
        return AdoptedHits::default();
    };
    let turn_context = MemoryTurnContext {
        surface: "cli_run".to_owned(),
        recalled_hits: recall.memory_turn_hits.clone(),
        injected_hits: recall.memory_turn_hits.clone(),
    };
    let report = memory_turn_citation_report(&turn_context, content, &haystack);
    AdoptedHits {
        cited_hit_refs: report.cited_hit_refs,
        adopted_hit_refs: report.adopted_hit_refs,
    }
}

fn failed_tool_count(receipts: &[RunToolReceipt]) -> usize {
    receipts
        .iter()
        .filter(|receipt| !agent_run_tool_receipt_completed_from_facts(*receipt))
        .count()
}

fn repeated_error_avoidance(
    recall: &RunMemoryRecallEvent,
    failed_tool_count: usize,
    run_completed: bool,
) -> u16 {
    if !run_completed || recall.debugging_experience_hit_count == 0 {
        return 0;
    }
    if failed_tool_count == 0 {
        10_000
    } else {
        5_000
    }
}

fn false_recall_risk(
    hit_count: usize,
    cited_hit_count: usize,
    failed_tool_count: usize,
    run_completed: bool,
) -> u16 {
    if hit_count == 0 || cited_hit_count > 0 || (run_completed && failed_tool_count == 0) {
        0
    } else {
        5_000
    }
}

fn tool_reduction_proxy(hit_count: usize, tool_call_count: usize) -> u16 {
    if hit_count == 0 {
        return 0;
    }
    match tool_call_count {
        0 | 1 => 10_000,
        2..=3 => 7_500,
        4..=6 => 4_000,
        _ => 1_000,
    }
}

fn basis_points(numerator: usize, denominator: usize) -> u16 {
    if denominator == 0 {
        return 0;
    }
    let value = (numerator.saturating_mul(10_000) / denominator).min(10_000);
    u16::try_from(value).unwrap_or(10_000)
}
