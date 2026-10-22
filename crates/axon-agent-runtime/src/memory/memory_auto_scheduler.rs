//! Agent Loop 自动记忆候选调度。

use std::path::Path;

use axon_crypto::sha256_hex;
use axon_domain::{AxonResult, MemoryCandidateId, MemoryId, MemoryKind, RuntimeCell, SessionId};
use axon_memory::MemoryOrchestrator;

use crate::{
    MemoryAutoCandidateInput, MemoryAutoCandidateReport, MemoryAutoCandidateRuntime,
    MemoryAutoCandidateSkipReason, MemoryWriteScope, MemoryWriteTriggerInput,
    MemoryWriteTriggerReport, MemoryWriteTriggerRuntime, SelfIterationHookKind, SelfIterationInput,
    SelfIterationPlan, SelfIterationRuntime, SelfIterationSubagentResult,
    SelfIterationSubagentStatus, SelfIterationWriteDecision, SemanticSalienceAssessment,
    SubagentDelegationExecutionRuntime, SubagentDelegationMergeReport, SubagentSessionHandle,
    TurnOutcomeEnvelope,
};

/// 自动记忆调度 source trace 前缀。
pub const MEMORY_AUTO_SCHEDULER_SOURCE_TRACE_PREFIX: &str = "memory-auto-scheduler";
/// 自动记忆调度已写入原因。
pub const MEMORY_AUTO_SCHEDULER_REASON_PERSISTED: &str = "auto candidate persisted";
/// 自动记忆调度无候选原因。
pub const MEMORY_AUTO_SCHEDULER_REASON_NO_CANDIDATE: &str = "auto candidate skipped";
/// 自动记忆调度未规划原因。
pub const MEMORY_AUTO_SCHEDULER_REASON_NOT_PLANNED: &str = "memory update hook not planned";

/// Agent Loop 自动记忆调度输入。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryAutoSchedulerInput<'a> {
    /// 已完成 RuntimeCell。
    pub cell: &'a RuntimeCell,
    /// 上游构建的有界任务结果事实包。
    pub turn_outcome: TurnOutcomeEnvelope,
    /// 上游模型输出并经严格校验的语义显著性评估。
    pub semantic_assessment: SemanticSalienceAssessment,
    /// 所属 session。
    pub session_id: SessionId,
    /// 父 session 字符串 ID。
    pub parent_session_id: String,
    /// 候选 ID。
    pub candidate_id: MemoryCandidateId,
    /// 目标 Memory ID。
    pub memory_id: MemoryId,
    /// 记忆作用域。
    pub scope: MemoryWriteScope,
    /// 记录时间。
    pub now_ms: u64,
    /// TTL 过期时间。
    pub ttl_expires_at_ms: Option<u64>,
    /// 重要度。
    pub importance_basis_points: u16,
    /// 可选 embedding。
    pub embedding: Option<Vec<f32>>,
    /// embedding 模型引用；提供 embedding 时必须同步提供。
    pub embedding_model_ref: Option<String>,
    /// 是否运行于 subagent。
    pub is_subagent: bool,
    /// 是否由远控触发。
    pub remote_triggered: bool,
    /// 会话累计迭代数。
    pub total_iterations: u64,
    /// 当前任务开始时的迭代数。
    pub goal_start_iterations: u64,
    /// 验证证据摘要。
    pub verification_evidence: Option<String>,
}

/// Agent Loop 自动记忆调度报告。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryAutoSchedulerReport {
    /// 自我迭代计划。
    pub plan: SelfIterationPlan,
    /// 自动记忆调度不直接启动真实子代理；真实执行由 CLI post-run 编排层承接。
    pub subagent_session: Option<SubagentSessionHandle>,
    /// 若已桥接 subagent delegation，则回汇统一 merge report。
    pub delegation_merge: Option<SubagentDelegationMergeReport>,
    /// 候选抽取报告。
    pub extraction: Option<MemoryAutoCandidateReport>,
    /// 写入报告。
    pub write: Option<MemoryWriteTriggerReport>,
    /// 回执记录。
    pub receipt: Option<crate::SelfIterationReceiptRecord>,
    /// 是否展示 memory updated 信息。
    pub memory_updated_info_visible: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否持久化 prompt 原文。
    pub prompt_text_persisted: bool,
    /// 是否持久化原始 transcript。
    pub raw_transcript_persisted: bool,
}

/// Agent Loop 自动记忆候选调度 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryAutoSchedulerRuntime;

impl MemoryAutoSchedulerRuntime {
    /// 从自我迭代计划调度自动候选抽取、写入和 receipt 审计。
    pub fn schedule(
        &self,
        orchestrator: &MemoryOrchestrator,
        receipt_store_path: impl AsRef<Path>,
        input: &MemoryAutoSchedulerInput<'_>,
    ) -> AxonResult<MemoryAutoSchedulerReport> {
        let plan = SelfIterationRuntime.plan_with_delegation(
            input.session_id,
            input.remote_triggered,
            &SelfIterationInput {
                is_subagent: input.is_subagent,
                turn_outcome: input.turn_outcome.clone(),
                semantic_assessment: Some(input.semantic_assessment.clone()),
                skill_execution: None,
                parent_total_cost_micros: 0,
                subagent_results: Vec::new(),
            },
        )?;
        if !has_memory_hook(&plan) {
            return Ok(report_without_receipt(plan));
        }

        let extraction = MemoryAutoCandidateRuntime.extract(&MemoryAutoCandidateInput {
            cell: input.cell,
            session_id: input.session_id,
            candidate_id: input.candidate_id,
            is_subagent: input.is_subagent,
            remote_triggered: input.remote_triggered,
            total_iterations: input.total_iterations,
            goal_start_iterations: input.goal_start_iterations,
            verification_evidence: input.verification_evidence.clone(),
        })?;
        let Some(candidate) = extraction.candidate.clone() else {
            let receipt = append_receipt(
                receipt_store_path,
                input,
                SelfIterationSubagentStatus::CompletedNoWrite,
                SelfIterationWriteDecision::SkippedNoChange,
                skip_reason_text(extraction.skipped_reason),
                None,
            )?;
            let delegation_merge = merge_memory_only_delegation(&plan)?;
            return Ok(report_with_receipt(
                plan,
                delegation_merge,
                extraction,
                None,
                receipt,
            ));
        };

        let summary_hash = Some(candidate_summary_hash(&candidate));
        let memory_kind = MemoryKind::from_candidate_kind(candidate.candidate_kind);
        let write = MemoryWriteTriggerRuntime.trigger(
            orchestrator,
            &MemoryWriteTriggerInput {
                candidate,
                memory_kind,
                memory_id: input.memory_id,
                supersedes_memory_ids: Vec::new(),
                scope: input.scope.clone(),
                now_ms: input.now_ms,
                ttl_expires_at_ms: input.ttl_expires_at_ms,
                importance_basis_points: input.importance_basis_points,
                embedding: input.embedding.clone(),
                embedding_model_ref: input.embedding_model_ref.clone(),
            },
        )?;
        let persisted = write.record.is_some();
        let receipt = append_receipt(
            receipt_store_path,
            input,
            if persisted {
                SelfIterationSubagentStatus::CompletedWithWrite
            } else {
                SelfIterationSubagentStatus::CompletedNoWrite
            },
            if persisted {
                SelfIterationWriteDecision::Persisted
            } else {
                SelfIterationWriteDecision::SkippedNoChange
            },
            if persisted {
                MEMORY_AUTO_SCHEDULER_REASON_PERSISTED
            } else {
                MEMORY_AUTO_SCHEDULER_REASON_NO_CANDIDATE
            },
            summary_hash,
        )?;

        let delegation_merge = merge_memory_delegation(&plan, persisted)?;
        Ok(report_with_receipt(
            plan,
            delegation_merge,
            extraction,
            Some(write),
            receipt,
        ))
    }
}

fn has_memory_hook(plan: &SelfIterationPlan) -> bool {
    plan.hooks
        .iter()
        .any(|hook| hook.kind == SelfIterationHookKind::MemoryUpdate)
}

fn report_without_receipt(plan: SelfIterationPlan) -> MemoryAutoSchedulerReport {
    MemoryAutoSchedulerReport {
        plan,
        subagent_session: None,
        delegation_merge: None,
        extraction: None,
        write: None,
        receipt: None,
        memory_updated_info_visible: false,
        remote_trigger_allowed: false,
        prompt_text_persisted: false,
        raw_transcript_persisted: false,
    }
}

fn report_with_receipt(
    plan: SelfIterationPlan,
    delegation_merge: Option<SubagentDelegationMergeReport>,
    extraction: MemoryAutoCandidateReport,
    write: Option<MemoryWriteTriggerReport>,
    receipt: crate::SelfIterationReceiptRecord,
) -> MemoryAutoSchedulerReport {
    MemoryAutoSchedulerReport {
        plan,
        subagent_session: None,
        delegation_merge,
        extraction: Some(extraction),
        write,
        memory_updated_info_visible: receipt.write_decision
            == SelfIterationWriteDecision::Persisted,
        receipt: Some(receipt),
        remote_trigger_allowed: false,
        prompt_text_persisted: false,
        raw_transcript_persisted: false,
    }
}

fn merge_memory_only_delegation(
    plan: &SelfIterationPlan,
) -> AxonResult<Option<SubagentDelegationMergeReport>> {
    let Some(delegation_plan) = plan.delegation_plan.as_ref() else {
        return Ok(None);
    };
    if delegation_plan.entries.is_empty() {
        return Ok(None);
    }
    SelfIterationRuntime
        .merge_delegation_results(
            plan,
            0,
            vec![SelfIterationSubagentResult {
                hook: SelfIterationHookKind::MemoryUpdate,
                label: "memory_update".to_owned(),
                cost_micros: 0,
                status: SelfIterationSubagentStatus::CompletedNoWrite,
                bounded_summary: MEMORY_AUTO_SCHEDULER_REASON_NO_CANDIDATE.to_owned(),
            }],
        )
        .map(Some)
}

fn merge_memory_delegation(
    plan: &SelfIterationPlan,
    persisted: bool,
) -> AxonResult<Option<SubagentDelegationMergeReport>> {
    let Some(delegation_plan) = plan.delegation_plan.as_ref() else {
        return Ok(None);
    };
    if delegation_plan.entries.is_empty() {
        return Ok(None);
    }
    let report =
        SubagentDelegationExecutionRuntime.execute(delegation_plan, 0, |entry, index| {
            Ok(crate::SubagentRunResult {
                dispatch_order: index,
                label: entry.label.clone(),
                status: if persisted {
                    crate::SubagentRunStatus::CompletedWithWrite
                } else {
                    crate::SubagentRunStatus::CompletedNoWrite
                },
                bounded_summary: if persisted {
                    MEMORY_AUTO_SCHEDULER_REASON_PERSISTED.to_owned()
                } else {
                    MEMORY_AUTO_SCHEDULER_REASON_NO_CANDIDATE.to_owned()
                },
                cost_micros: 0,
            })
        })?;
    Ok(Some(report))
}

fn append_receipt(
    path: impl AsRef<Path>,
    input: &MemoryAutoSchedulerInput<'_>,
    status: SelfIterationSubagentStatus,
    write_decision: SelfIterationWriteDecision,
    reason: &str,
    summary_hash: Option<String>,
) -> AxonResult<crate::SelfIterationReceiptRecord> {
    let receipt =
        crate::SelfIterationReceiptRecord::from_input(crate::SelfIterationReceiptInput {
            parent_session_id: input.parent_session_id.clone(),
            parent_cell_id: Some(input.cell.id.0),
            recorded_at_ms: input.now_ms,
            hook: SelfIterationHookKind::MemoryUpdate,
            source_trace: format!(
                "{}:{}:{}",
                MEMORY_AUTO_SCHEDULER_SOURCE_TRACE_PREFIX, input.session_id.0, input.cell.id.0
            ),
            status,
            cost_micros: 0,
            write_decision,
            reason: reason.to_owned(),
            summary_hash,
            skill_name: None,
            hook_iterations: input
                .total_iterations
                .saturating_sub(input.goal_start_iterations),
        })?;
    crate::SelfIterationReceiptStoreRuntime::append_record(path, &receipt)?;
    Ok(receipt)
}

fn candidate_summary_hash(candidate: &axon_domain::MemoryCandidate) -> String {
    let source_summary = candidate
        .source_trace
        .as_ref()
        .map_or("", |trace| trace.summary.as_str());
    sha256_hex(format!("{}:{source_summary}", candidate.content_ref).as_bytes())
}

const fn skip_reason_text(reason: Option<MemoryAutoCandidateSkipReason>) -> &'static str {
    match reason {
        Some(MemoryAutoCandidateSkipReason::SubagentBlocked) => "subagent blocked",
        Some(MemoryAutoCandidateSkipReason::RemoteTriggerBlocked) => "remote trigger blocked",
        Some(MemoryAutoCandidateSkipReason::MissingFailedAttempt) => "missing failed attempt",
        Some(MemoryAutoCandidateSkipReason::MissingFinalFix) => "missing final fix",
        Some(MemoryAutoCandidateSkipReason::MissingVerification) => "missing verification",
        Some(MemoryAutoCandidateSkipReason::ExperienceRejected) => "experience rejected",
        None => MEMORY_AUTO_SCHEDULER_REASON_NO_CANDIDATE,
    }
}

#[cfg(test)]
mod tests {
    use axon_domain::{
        ApplyExecutionStatus, CellId, CellResult, ExecutionRequestId, MemoryCandidateId, MemoryId,
        ModelProfileId, PaneId, PermissionSnapshotId, RuntimeCell, SessionId,
    };
    use axon_memory::{
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig, MemoryOrchestrator,
    };

    use super::*;
    use crate::{
        ExecutionEvidenceIndex, ExecutionEvidenceKind, ExecutionEvidenceRecord,
        ExecutionEvidenceStatus, SEMANTIC_SALIENCE_SCHEMA_VERSION, SemanticMemoryCandidateType,
        SemanticSalienceSignals, TurnFinalStatus,
    };

    #[test]
    fn scheduler_keeps_memory_delegation_audit_without_fake_subagent_session() {
        let temp = axon_test_support::test_temp_dir();
        let orchestrator = memory_orchestrator(temp.path());
        let receipt_path = temp.path().join("receipts.jsonl");
        let cell = verified_completed_cell();

        let report = MemoryAutoSchedulerRuntime
            .schedule(
                &orchestrator,
                &receipt_path,
                &MemoryAutoSchedulerInput {
                    cell: &cell,
                    turn_outcome: verified_turn_outcome(),
                    semantic_assessment: verified_semantic_assessment(),
                    session_id: SessionId(7),
                    parent_session_id: "parent-session".to_owned(),
                    candidate_id: MemoryCandidateId(70),
                    memory_id: MemoryId(700),
                    scope: MemoryWriteScope {
                        namespace_ref: "memory-scope:test".to_owned(),
                        repo_ref: Some("memory-repo:test".to_owned()),
                        user_ref: None,
                    },
                    now_ms: 1_000,
                    ttl_expires_at_ms: None,
                    importance_basis_points: 5_000,
                    embedding: None,
                    embedding_model_ref: None,
                    is_subagent: false,
                    remote_triggered: false,
                    total_iterations: 1,
                    goal_start_iterations: 0,
                    verification_evidence: Some("cargo test -p axon-agent-runtime 通过".to_owned()),
                },
            )
            .unwrap();

        assert!(report.plan.delegation_plan.is_some());
        assert!(report.delegation_merge.is_some());
        assert!(report.receipt.is_some());
        assert!(report.subagent_session.is_none());
    }

    fn memory_orchestrator(root: &std::path::Path) -> MemoryOrchestrator {
        MemoryOrchestrator::init(
            MemoryConfig::from_config_dir(root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
                .unwrap(),
        )
        .unwrap()
    }

    fn verified_completed_cell() -> RuntimeCell {
        let mut cell = RuntimeCell::new(
            CellId(9),
            PaneId(1),
            "E:\\zbg\\axon-code-subagent-control",
            ModelProfileId(1),
            PermissionSnapshotId(1),
        );
        cell.start().unwrap();
        cell.project_apply_execution(
            "src/lib.rs",
            ExecutionRequestId(1),
            ApplyExecutionStatus::ClientWriteFailed,
            "第一次写入未通过测试",
            "apply://failed",
        )
        .unwrap();
        cell.project_apply_execution(
            "src/lib.rs",
            ExecutionRequestId(2),
            ApplyExecutionStatus::ClientWriteCompleted,
            "补齐边界后测试通过",
            "apply://completed",
        )
        .unwrap();
        cell.close(None).unwrap();
        cell.complete(CellResult {
            summary: "任务完成".to_owned(),
        })
        .unwrap();
        cell
    }

    fn verified_turn_outcome() -> TurnOutcomeEnvelope {
        TurnOutcomeEnvelope {
            conversation_id: "parent-session".to_owned(),
            turn_id: "turn-1".to_owned(),
            agent_path: "/root".to_owned(),
            user_intent_summary: "修复并验证问题".to_owned(),
            assistant_outcome_summary: "测试通过".to_owned(),
            user_message_ref: "message:user:1".to_owned(),
            execution_evidence: ExecutionEvidenceIndex {
                records: vec![ExecutionEvidenceRecord {
                    evidence_ref: "receipt:test".to_owned(),
                    tool_name: "builtin.shell_command".to_owned(),
                    status: ExecutionEvidenceStatus::Verified,
                    kind: ExecutionEvidenceKind::Validation,
                    runtime_verified: true,
                    verification_refs: Vec::new(),
                }],
            },
            selected_skill: None,
            final_status: TurnFinalStatus::Completed,
        }
    }

    fn verified_semantic_assessment() -> SemanticSalienceAssessment {
        SemanticSalienceAssessment {
            schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
            should_reflect: true,
            candidate_types: vec![SemanticMemoryCandidateType::Experience],
            signals: SemanticSalienceSignals {
                future_utility: 9_000,
                verification_strength: 9_000,
                scope_confidence: 9_000,
                ..SemanticSalienceSignals::default()
            },
            evidence_refs: vec!["receipt:test".to_owned()],
            reason: "已验证的执行经验".to_owned(),
        }
    }
}
