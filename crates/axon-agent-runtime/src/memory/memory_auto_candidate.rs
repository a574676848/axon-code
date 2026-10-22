//! Agent Loop 自动记忆候选抽取。

use axon_crypto::sha256_hex;
use axon_domain::{
    ApplyExecutionStatus, AxonResult, MemoryCandidate, MemoryCandidateId, RuntimeCell,
    RuntimeCellEvent, SessionId,
};

use crate::{
    DebuggingExperienceDecision, DebuggingExperienceRuntime, ExperienceDistillationDecision,
    ExperienceDistillationInput, ExperienceDistillationRuntime, ExperienceReusePolicy,
};

/// 自动记忆候选 evidence ref 前缀。
pub const MEMORY_AUTO_CANDIDATE_EVIDENCE_PREFIX: &str = "runtime-cell";
/// 自动记忆候选问题签名前缀。
pub const MEMORY_AUTO_CANDIDATE_SIGNATURE_PREFIX: &str = "sha256:";

/// Agent Loop 自动记忆候选抽取输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAutoCandidateInput<'a> {
    /// 已完成的 RuntimeCell。
    pub cell: &'a RuntimeCell,
    /// 所属 session。
    pub session_id: SessionId,
    /// 候选 ID。
    pub candidate_id: MemoryCandidateId,
    /// 是否运行于 subagent。
    pub is_subagent: bool,
    /// 是否由远控触发。
    pub remote_triggered: bool,
    /// 会话累计迭代数。
    pub total_iterations: u64,
    /// 当前任务开始时的迭代数。
    pub goal_start_iterations: u64,
    /// 外层验证证据摘要；不接受原始日志。
    pub verification_evidence: Option<String>,
}

/// Agent Loop 自动记忆候选抽取报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAutoCandidateReport {
    /// 抽取出的候选。
    pub candidate: Option<MemoryCandidate>,
    /// 跳过原因。
    pub skipped_reason: Option<MemoryAutoCandidateSkipReason>,
    /// 可审计 evidence ref。
    pub evidence_ref: Option<String>,
    /// 是否只允许本机写入。
    pub local_only: bool,
    /// 是否允许远控写入。
    pub remote_write_allowed: bool,
    /// 是否持久化 prompt 原文。
    pub prompt_text_persisted: bool,
    /// 是否持久化 raw payload。
    pub raw_payload_persisted: bool,
}

/// 自动候选抽取跳过原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAutoCandidateSkipReason {
    /// subagent 不自动写长期记忆。
    SubagentBlocked,
    /// 远控不能触发自动写入。
    RemoteTriggerBlocked,
    /// 缺少失败尝试摘要。
    MissingFailedAttempt,
    /// 缺少最终修复摘要。
    MissingFinalFix,
    /// 缺少验证证据。
    MissingVerification,
    /// 经验沉淀拒绝。
    ExperienceRejected,
}

/// Agent Loop 自动记忆候选抽取 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryAutoCandidateRuntime;

impl MemoryAutoCandidateRuntime {
    /// 从已完成 RuntimeCell timeline 抽取高价值 debugging experience 候选。
    pub fn extract(
        &self,
        input: &MemoryAutoCandidateInput<'_>,
    ) -> AxonResult<MemoryAutoCandidateReport> {
        axon_domain::MemoryAdmissionGate::ensure_cell_extractable(input.cell)?;
        if input.is_subagent {
            return Ok(skipped(MemoryAutoCandidateSkipReason::SubagentBlocked));
        }
        if input.remote_triggered {
            return Ok(skipped(MemoryAutoCandidateSkipReason::RemoteTriggerBlocked));
        }
        let Some(failed_attempt) = latest_failed_attempt(input.cell) else {
            return Ok(skipped(MemoryAutoCandidateSkipReason::MissingFailedAttempt));
        };
        let Some(final_fix) = latest_successful_fix(input.cell) else {
            return Ok(skipped(MemoryAutoCandidateSkipReason::MissingFinalFix));
        };
        let verification = input
            .verification_evidence
            .as_deref()
            .map(str::trim)
            .map(str::to_owned)
            .filter(|value| !value.is_empty());
        let Some(verification) = verification else {
            return Ok(skipped(MemoryAutoCandidateSkipReason::MissingVerification));
        };
        let evidence_ref = evidence_ref(input.session_id, input.cell.id);
        let distilled = ExperienceDistillationRuntime.distill(ExperienceDistillationInput {
            problem_signature: problem_signature(&failed_attempt, &final_fix),
            environment: input.cell.cwd.trim().to_owned(),
            failed_attempts: vec![failed_attempt],
            final_fix,
            verification,
            reuse_policy: ExperienceReusePolicy::ProjectOnly,
        });
        let ExperienceDistillationDecision::Record(record) = distilled else {
            return Ok(skipped(MemoryAutoCandidateSkipReason::ExperienceRejected));
        };
        let debugging_input = ExperienceDistillationRuntime::debugging_input(
            &record,
            input.candidate_id,
            input.session_id,
            input.cell.id,
            evidence_ref.clone(),
        );
        let DebuggingExperienceDecision::Candidate(candidate) =
            DebuggingExperienceRuntime.candidate_from_closed_loop(debugging_input)
        else {
            return Ok(skipped(MemoryAutoCandidateSkipReason::ExperienceRejected));
        };

        Ok(MemoryAutoCandidateReport {
            candidate: Some(candidate),
            skipped_reason: None,
            evidence_ref: Some(evidence_ref),
            local_only: true,
            remote_write_allowed: false,
            prompt_text_persisted: false,
            raw_payload_persisted: false,
        })
    }
}

fn latest_failed_attempt(cell: &RuntimeCell) -> Option<String> {
    cell.events.iter().rev().find_map(|event| {
        if let RuntimeCellEvent::ApplyExecutionProjected {
            status,
            audit_summary,
            ..
        } = event
            && matches!(
                status,
                ApplyExecutionStatus::ClientWriteFailed | ApplyExecutionStatus::Blocked
            )
        {
            return Some(audit_summary.trim().to_owned());
        }
        None
    })
}

fn latest_successful_fix(cell: &RuntimeCell) -> Option<String> {
    cell.events.iter().rev().find_map(|event| {
        if let RuntimeCellEvent::ApplyExecutionProjected {
            status,
            audit_summary,
            path,
            ..
        } = event
            && *status == ApplyExecutionStatus::ClientWriteCompleted
        {
            let summary = audit_summary.trim();
            return Some(format!("{}: {}", path.trim(), summary));
        }
        None
    })
}

fn skipped(reason: MemoryAutoCandidateSkipReason) -> MemoryAutoCandidateReport {
    MemoryAutoCandidateReport {
        candidate: None,
        skipped_reason: Some(reason),
        evidence_ref: None,
        local_only: true,
        remote_write_allowed: false,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
    }
}

fn evidence_ref(session_id: SessionId, cell_id: axon_domain::CellId) -> String {
    format!(
        "{}:{}:{}",
        MEMORY_AUTO_CANDIDATE_EVIDENCE_PREFIX, session_id.0, cell_id.0
    )
}

fn problem_signature(failed_attempt: &str, final_fix: &str) -> String {
    format!(
        "{}{}",
        MEMORY_AUTO_CANDIDATE_SIGNATURE_PREFIX,
        sha256_hex(format!("{failed_attempt}:{final_fix}").as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use axon_domain::{
        CellId, CellResult, ExecutionRequestId, MemorySourceOrigin, ModelProfileId, PaneId,
        PermissionSnapshotId, RuntimeCell,
    };

    use super::*;

    #[test]
    fn auto_candidate_extracts_closed_loop_debugging_experience() {
        let mut cell = completed_cell();
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

        let report = MemoryAutoCandidateRuntime
            .extract(&MemoryAutoCandidateInput {
                cell: &cell,
                session_id: SessionId(7),
                candidate_id: MemoryCandidateId(70),
                is_subagent: false,
                remote_triggered: false,
                total_iterations: 1,
                goal_start_iterations: 0,
                verification_evidence: Some("cargo test --workspace 通过".to_owned()),
            })
            .unwrap();
        let candidate = report.candidate.unwrap();

        assert_eq!(
            candidate.source_trace.unwrap().origin,
            MemorySourceOrigin::DebuggingExperience
        );
        assert!(report.local_only);
        assert!(!report.remote_write_allowed);
        assert!(!report.prompt_text_persisted);
        assert!(!report.raw_payload_persisted);
    }

    #[test]
    fn auto_candidate_preserves_long_apply_summaries() {
        const SUMMARY_CHARS: usize = 500;

        let mut cell = completed_cell();
        let failed = "f".repeat(SUMMARY_CHARS);
        let fixed = "s".repeat(SUMMARY_CHARS);
        cell.project_apply_execution(
            "src/lib.rs",
            ExecutionRequestId(1),
            ApplyExecutionStatus::ClientWriteFailed,
            &failed,
            "apply://failed",
        )
        .unwrap();
        cell.project_apply_execution(
            "src/lib.rs",
            ExecutionRequestId(2),
            ApplyExecutionStatus::ClientWriteCompleted,
            &fixed,
            "apply://completed",
        )
        .unwrap();

        assert_eq!(
            latest_failed_attempt(&cell).as_deref(),
            Some(failed.as_str())
        );
        let expected_fix = format!("src/lib.rs: {fixed}");
        assert_eq!(
            latest_successful_fix(&cell).as_deref(),
            Some(expected_fix.as_str())
        );
    }

    #[test]
    fn auto_candidate_blocks_short_subagent_remote_or_unverified_runs() {
        let cell = closed_completed_cell_without_apply();
        let verified_cell = verified_completed_cell();
        let base = MemoryAutoCandidateInput {
            cell: &cell,
            session_id: SessionId(7),
            candidate_id: MemoryCandidateId(70),
            is_subagent: false,
            remote_triggered: false,
            total_iterations: 1,
            goal_start_iterations: 0,
            verification_evidence: Some("cargo test --workspace 通过".to_owned()),
        };

        assert_eq!(
            MemoryAutoCandidateRuntime
                .extract(&MemoryAutoCandidateInput {
                    is_subagent: true,
                    ..base.clone()
                })
                .unwrap()
                .skipped_reason,
            Some(MemoryAutoCandidateSkipReason::SubagentBlocked)
        );
        assert_eq!(
            MemoryAutoCandidateRuntime
                .extract(&MemoryAutoCandidateInput {
                    remote_triggered: true,
                    ..base.clone()
                })
                .unwrap()
                .skipped_reason,
            Some(MemoryAutoCandidateSkipReason::RemoteTriggerBlocked)
        );
        assert_eq!(
            MemoryAutoCandidateRuntime
                .extract(&MemoryAutoCandidateInput {
                    cell: &verified_cell,
                    verification_evidence: None,
                    ..base
                })
                .unwrap()
                .skipped_reason,
            Some(MemoryAutoCandidateSkipReason::MissingVerification)
        );
    }

    fn completed_cell() -> RuntimeCell {
        let cwd = std::env::current_dir()
            .unwrap_or_else(|_| axon_test_support::test_temp_root())
            .to_string_lossy()
            .into_owned();
        let mut cell = RuntimeCell::new(
            CellId(9),
            PaneId(1),
            &cwd,
            ModelProfileId(1),
            PermissionSnapshotId(1),
        );
        cell.start().unwrap();
        cell
    }

    fn closed_completed_cell_without_apply() -> RuntimeCell {
        let mut cell = completed_cell();
        cell.close(None).unwrap();
        cell.complete(CellResult {
            summary: "任务完成".to_owned(),
        })
        .unwrap();
        cell
    }

    fn verified_completed_cell() -> RuntimeCell {
        let mut cell = completed_cell();
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
}
