//! Debugging experience 记忆候选构造。

use axon_domain::{
    CellId, MemoryCandidate, MemoryCandidateId, MemoryCandidateKind, MemoryRetentionPolicy,
    MemorySensitivity, MemorySourceOrigin, MemorySourceTrace, MemoryVerification, SessionId,
};

const DEBUG_EXPERIENCE_CONFIDENCE_BASIS_POINTS: u16 = 9_000;
const DEBUG_EXPERIENCE_CONTENT_PREFIX: &str = "memory-ref:debugging-experience";
const PRIVATE_LOG_MIN_NON_EMPTY_LINES: usize = 4;
const SENSITIVE_ASSIGNMENT_MARKERS: &[&str] = &[
    "api_key=",
    "api_key:",
    "api key=",
    "api key:",
    "authorization:",
    "bearer ",
    "password=",
    "password:",
    "secret=",
    "secret:",
    "token=",
    "token:",
];
const SECRET_LITERAL_MARKERS: &[&str] = &["-----begin ", "private key", "sk-"];
const PRIVATE_LOG_MARKERS: &[&str] = &[
    "error", "trace", "debug", "info", "warn", "[error]", "[debug]",
];

/// 经验记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceRecord {
    /// 问题签名 hash。
    pub problem_signature: String,
    /// 环境摘要。
    pub environment: String,
    /// 失败尝试摘要。
    pub failed_attempts: Vec<String>,
    /// 最终修复摘要。
    pub final_fix: String,
    /// 验证证据。
    pub verification: String,
    /// 复用策略。
    pub reuse_policy: ExperienceReusePolicy,
}

/// 经验复用策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperienceReusePolicy {
    /// 仅当前项目复用。
    ProjectOnly,
    /// 同类环境可复用。
    SimilarEnvironment,
}

/// 经验沉淀输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceDistillationInput {
    /// 问题签名 hash。
    pub problem_signature: String,
    /// 环境摘要。
    pub environment: String,
    /// 失败尝试摘要。
    pub failed_attempts: Vec<String>,
    /// 最终修复摘要。
    pub final_fix: String,
    /// 验证证据。
    pub verification: String,
    /// 复用策略。
    pub reuse_policy: ExperienceReusePolicy,
}

/// 经验沉淀决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperienceDistillationDecision {
    /// 已形成经验记录。
    Record(Box<ExperienceRecord>),
    /// 缺少闭环证据，拒绝沉淀经验。
    Rejected(ExperienceRejectReason),
}

/// 经验沉淀拒绝原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperienceRejectReason {
    /// 缺少问题签名。
    MissingProblemSignature,
    /// 缺少环境摘要。
    MissingEnvironment,
    /// 缺少失败尝试。
    MissingFailedAttempt,
    /// 缺少最终修复。
    MissingFinalFix,
    /// 缺少验证证据。
    MissingVerification,
    /// 摘要包含 secret 或 token。
    SensitiveContentDetected,
    /// 摘要疑似包含完整私有日志。
    PrivateLogDetected,
}

/// Experience distillation runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ExperienceDistillationRuntime;

impl ExperienceDistillationRuntime {
    /// 只有失败、修复、验证闭环完整时才沉淀经验。
    #[must_use]
    pub fn distill(&self, input: ExperienceDistillationInput) -> ExperienceDistillationDecision {
        if input.problem_signature.trim().is_empty() {
            return ExperienceDistillationDecision::Rejected(
                ExperienceRejectReason::MissingProblemSignature,
            );
        }
        if input.environment.trim().is_empty() {
            return ExperienceDistillationDecision::Rejected(
                ExperienceRejectReason::MissingEnvironment,
            );
        }
        if input
            .failed_attempts
            .iter()
            .all(|attempt| attempt.trim().is_empty())
        {
            return ExperienceDistillationDecision::Rejected(
                ExperienceRejectReason::MissingFailedAttempt,
            );
        }
        if input.final_fix.trim().is_empty() {
            return ExperienceDistillationDecision::Rejected(
                ExperienceRejectReason::MissingFinalFix,
            );
        }
        if input.verification.trim().is_empty() {
            return ExperienceDistillationDecision::Rejected(
                ExperienceRejectReason::MissingVerification,
            );
        }
        if input_has_sensitive_content(&input) {
            return ExperienceDistillationDecision::Rejected(
                ExperienceRejectReason::SensitiveContentDetected,
            );
        }
        if input_has_private_log(&input) {
            return ExperienceDistillationDecision::Rejected(
                ExperienceRejectReason::PrivateLogDetected,
            );
        }

        ExperienceDistillationDecision::Record(Box::new(ExperienceRecord {
            problem_signature: input.problem_signature,
            environment: input.environment,
            failed_attempts: input
                .failed_attempts
                .into_iter()
                .filter(|attempt| !attempt.trim().is_empty())
                .collect(),
            final_fix: input.final_fix,
            verification: input.verification,
            reuse_policy: input.reuse_policy,
        }))
    }

    /// 将经验记录转换为 debugging experience 输入。
    #[must_use]
    pub fn debugging_input(
        record: &ExperienceRecord,
        candidate_id: MemoryCandidateId,
        session_id: SessionId,
        cell_id: CellId,
        evidence_ref: impl Into<String>,
    ) -> DebuggingExperienceInput {
        DebuggingExperienceInput {
            candidate_id,
            session_id,
            cell_id,
            failure_diagnostic: record.failed_attempts.join("；"),
            fix_summary: record.final_fix.clone(),
            verification_evidence: record.verification.clone(),
            evidence_ref: evidence_ref.into(),
            content_ref: debugging_experience_content_ref(session_id, cell_id),
        }
    }
}

fn input_has_sensitive_content(input: &ExperienceDistillationInput) -> bool {
    let fields = [
        input.problem_signature.as_str(),
        input.environment.as_str(),
        input.final_fix.as_str(),
        input.verification.as_str(),
    ];
    fields
        .into_iter()
        .chain(input.failed_attempts.iter().map(String::as_str))
        .any(summary_contains_sensitive_content)
}

pub(crate) fn summary_contains_sensitive_content(summary: &str) -> bool {
    let normalized = summary.to_ascii_lowercase();
    SENSITIVE_ASSIGNMENT_MARKERS
        .iter()
        .chain(SECRET_LITERAL_MARKERS.iter())
        .any(|marker| normalized.contains(marker))
}

fn input_has_private_log(input: &ExperienceDistillationInput) -> bool {
    let fields = [
        input.problem_signature.as_str(),
        input.environment.as_str(),
        input.final_fix.as_str(),
        input.verification.as_str(),
    ];
    fields
        .into_iter()
        .chain(input.failed_attempts.iter().map(String::as_str))
        .any(summary_looks_like_private_log)
}

pub(crate) fn summary_looks_like_private_log(summary: &str) -> bool {
    let non_empty_lines = summary
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    non_empty_lines >= PRIVATE_LOG_MIN_NON_EMPTY_LINES && summary_has_log_marker(summary)
}

fn summary_has_log_marker(summary: &str) -> bool {
    let normalized = summary.to_ascii_lowercase();
    PRIVATE_LOG_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

/// Debugging experience 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebuggingExperienceInput {
    /// 候选 ID。
    pub candidate_id: MemoryCandidateId,
    /// 所属 session。
    pub session_id: SessionId,
    /// 所属 RuntimeCell。
    pub cell_id: CellId,
    /// 失败诊断摘要。
    pub failure_diagnostic: String,
    /// 修复结果摘要。
    pub fix_summary: String,
    /// 验证证据摘要。
    pub verification_evidence: String,
    /// 可审计证据引用。
    pub evidence_ref: String,
    /// 内容引用。
    pub content_ref: String,
}

/// Debugging experience 构造结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebuggingExperienceDecision {
    /// 已形成记忆候选。
    Candidate(MemoryCandidate),
    /// 缺少闭环证据，拒绝形成候选。
    Rejected(DebuggingExperienceRejectReason),
}

/// Debugging experience 拒绝原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebuggingExperienceRejectReason {
    /// 缺少失败诊断。
    MissingFailureDiagnostic,
    /// 缺少修复结果。
    MissingFixSummary,
    /// 缺少验证证据。
    MissingVerificationEvidence,
    /// 缺少证据引用。
    MissingEvidenceRef,
    /// 缺少内容引用。
    MissingContentRef,
}

/// Debugging experience runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct DebuggingExperienceRuntime;

impl DebuggingExperienceRuntime {
    /// 构造 debugging experience 记忆候选。
    #[must_use]
    pub fn candidate_from_closed_loop(
        &self,
        input: DebuggingExperienceInput,
    ) -> DebuggingExperienceDecision {
        if input.failure_diagnostic.trim().is_empty() {
            return DebuggingExperienceDecision::Rejected(
                DebuggingExperienceRejectReason::MissingFailureDiagnostic,
            );
        }
        if input.fix_summary.trim().is_empty() {
            return DebuggingExperienceDecision::Rejected(
                DebuggingExperienceRejectReason::MissingFixSummary,
            );
        }
        if input.verification_evidence.trim().is_empty() {
            return DebuggingExperienceDecision::Rejected(
                DebuggingExperienceRejectReason::MissingVerificationEvidence,
            );
        }
        if input.evidence_ref.trim().is_empty() {
            return DebuggingExperienceDecision::Rejected(
                DebuggingExperienceRejectReason::MissingEvidenceRef,
            );
        }
        if input.content_ref.trim().is_empty() {
            return DebuggingExperienceDecision::Rejected(
                DebuggingExperienceRejectReason::MissingContentRef,
            );
        }

        let summary = debugging_summary(&input);
        DebuggingExperienceDecision::Candidate(MemoryCandidate {
            id: input.candidate_id,
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::DebuggingExperience,
                session_id: Some(input.session_id),
                cell_id: Some(input.cell_id),
                summary,
                evidence_ref: Some(input.evidence_ref),
            }),
            candidate_kind: MemoryCandidateKind::DebuggingExperience,
            content_ref: input.content_ref,
            confidence_basis_points: DEBUG_EXPERIENCE_CONFIDENCE_BASIS_POINTS,
            sensitivity: MemorySensitivity::Internal,
            proposed_retention: MemoryRetentionPolicy::Project,
            verification: MemoryVerification::AgentVerified,
        })
    }
}

fn debugging_summary(input: &DebuggingExperienceInput) -> String {
    format!(
        "失败诊断: {}; 修复结果: {}; 验证证据: {}",
        input.failure_diagnostic.trim(),
        input.fix_summary.trim(),
        input.verification_evidence.trim()
    )
}

/// 构造 debugging experience 内容引用。
#[must_use]
pub fn debugging_experience_content_ref(session_id: SessionId, cell_id: CellId) -> String {
    format!(
        "{}:{}:{}",
        DEBUG_EXPERIENCE_CONTENT_PREFIX, session_id.0, cell_id.0
    )
}
