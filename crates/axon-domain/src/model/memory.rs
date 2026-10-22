//! 记忆准入领域合同。

use crate::{
    AxonError, AxonResult, CellClosureState, CellId, MemoryCandidateId, MemoryId, RuntimeCell,
    RuntimeCellState, SessionId,
};

/// strong memory 默认最低置信度，单位 basis point。
pub const MEMORY_STRONG_MIN_CONFIDENCE_BASIS_POINTS: u16 = 8_000;
/// weak memory 默认最低置信度，单位 basis point。
pub const MEMORY_WEAK_MIN_CONFIDENCE_BASIS_POINTS: u16 = 5_000;
/// 默认遗忘墓碑原因，不包含原文。
pub const MEMORY_DEFAULT_TOMBSTONE_REASON: &str = "用户显式遗忘或治理删除";

/// 记忆候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCandidate {
    /// 候选 ID。
    pub id: MemoryCandidateId,
    /// 来源追踪；缺失时不得写入 strong memory。
    pub source_trace: Option<MemorySourceTrace>,
    /// 候选类型。
    pub candidate_kind: MemoryCandidateKind,
    /// 内容引用，不保存原文。
    pub content_ref: String,
    /// 置信度，单位 basis point。
    pub confidence_basis_points: u16,
    /// 敏感等级。
    pub sensitivity: MemorySensitivity,
    /// 建议保留策略。
    pub proposed_retention: MemoryRetentionPolicy,
    /// 候选验证来源。
    pub verification: MemoryVerification,
}

/// 记忆来源追踪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySourceTrace {
    /// 来源类型。
    pub origin: MemorySourceOrigin,
    /// 关联 session。
    pub session_id: Option<SessionId>,
    /// 关联 RuntimeCell。
    pub cell_id: Option<CellId>,
    /// 来源摘要，不含 secret 原文。
    pub summary: String,
    /// 可审计证据引用。
    pub evidence_ref: Option<String>,
}

/// 记忆来源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySourceOrigin {
    /// 本机 runtime timeline。
    LocalTimeline,
    /// Agent 从用户语境提炼的画像或偏好。
    AgentUserContext,
    /// Agent 从完成任务中提炼的经验。
    AgentExperience,
    /// Agent 从当前语境确认的稳定知识。
    AgentKnowledge,
    /// Agent 从实践中提炼的可复用程序。
    AgentProcedure,
    /// 用户显式偏好。
    ExplicitUserPreference,
    /// debugging failure 与修复结果。
    DebuggingExperience,
    /// 远控上传摘要。
    RemoteUpload,
    /// command completion feedback。
    CommandCompletionFeedback,
    /// skill / playbook 执行反思。
    SkillReflection,
}

/// 记忆候选类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryCandidateKind {
    /// 用户画像。
    Profile,
    /// 用户画像或偏好。
    UserPreference,
    /// 通用经验。
    Experience,
    /// 稳定事实、实体或领域知识。
    Knowledge,
    /// 调试经验。
    DebuggingExperience,
    /// 工具输出摘要。
    ToolResultSummary,
    /// 远控上传摘要。
    RemoteUploadSummary,
    /// 补全反馈。
    CommandFeedback,
    /// 程序记忆：skill、playbook 或工具调用模板。
    ProceduralMemory,
}

/// memory_write 与 Memory SQLite 使用的稳定记忆类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// 用户画像。
    Profile,
    /// 用户偏好。
    Preference,
    /// 可复用经验。
    Experience,
    /// 稳定知识。
    Knowledge,
    /// 可执行程序或流程。
    Procedure,
}

impl MemoryKind {
    /// 返回持久化和 API 使用的稳定小写代码值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Preference => "preference",
            Self::Experience => "experience",
            Self::Knowledge => "knowledge",
            Self::Procedure => "procedure",
        }
    }

    /// 从候选类型映射到五值 Memory 类型。
    #[must_use]
    pub const fn from_candidate_kind(kind: MemoryCandidateKind) -> Self {
        match kind {
            MemoryCandidateKind::Profile => Self::Profile,
            MemoryCandidateKind::UserPreference => Self::Preference,
            MemoryCandidateKind::Experience | MemoryCandidateKind::DebuggingExperience => {
                Self::Experience
            }
            MemoryCandidateKind::Knowledge
            | MemoryCandidateKind::ToolResultSummary
            | MemoryCandidateKind::RemoteUploadSummary
            | MemoryCandidateKind::CommandFeedback => Self::Knowledge,
            MemoryCandidateKind::ProceduralMemory => Self::Procedure,
        }
    }

    /// 从稳定持久化代码解析类型。
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "profile" => Some(Self::Profile),
            "preference" => Some(Self::Preference),
            "experience" => Some(Self::Experience),
            "knowledge" => Some(Self::Knowledge),
            "procedure" => Some(Self::Procedure),
            _ => None,
        }
    }
}

/// 记忆敏感等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySensitivity {
    /// 公开或无敏感内容。
    Public,
    /// 项目内部信息。
    Internal,
    /// 个人偏好或画像。
    Personal,
    /// 接近 secret，默认隔离。
    SecretAdjacent,
    /// secret，默认拒绝。
    Secret,
}

/// 记忆保留策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRetentionPolicy {
    /// 仅当前 session。
    Session,
    /// 当前项目。
    Project,
    /// 长期保留。
    LongTerm,
}

/// 记忆候选验证来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryVerification {
    /// 未形成可信闭环证据。
    Unverified,
    /// 用户显式要求写入或本机人工复核通过。
    UserVerified,
    /// self-iteration 子代理基于本机工具、验证结果和闭环证据自动确认。
    AgentVerified,
}

/// 记忆写入决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryWriteDecision {
    /// 写入 strong memory。
    WriteStrong,
    /// 写入 weak memory。
    WriteWeak,
    /// 需要本机用户复核。
    NeedsUserReview,
    /// 进入隔离区。
    Quarantine,
    /// 拒绝写入。
    Reject,
    /// 写入遗忘 tombstone。
    ForgetTombstone,
}

/// 记忆准入原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAdmissionReason {
    /// source trace 缺失。
    MissingSourceTrace,
    /// source trace 摘要为空。
    EmptySourceSummary,
    /// content ref 为空。
    EmptyContentRef,
    /// 置信度低于 weak 阈值。
    ConfidenceTooLow,
    /// 需要本机复核后才能 strong write。
    LocalReviewRequired,
    /// 远控来源只能进入本机复核。
    RemoteSourceRequiresReview,
    /// secret-adjacent 内容进入隔离区。
    SecretAdjacentQuarantined,
    /// secret 内容被拒绝。
    SecretRejected,
    /// 允许 weak memory。
    WeakAllowed,
    /// 用户画像默认只能进入 weak memory。
    UserProfileDefaultsWeak,
    /// 允许 strong memory。
    StrongAllowed,
}

/// 记忆准入结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAdmissionDecision {
    /// 写入决策。
    pub decision: MemoryWriteDecision,
    /// 决策原因。
    pub reason: MemoryAdmissionReason,
    /// 是否允许形成 strong memory。
    pub strong_memory_allowed: bool,
}

/// 记忆治理状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryState {
    /// 弱记忆，只能作为低权重候选召回。
    Weak,
    /// 强记忆，可进入常规上下文召回。
    Strong,
    /// 隔离记忆，不得进入模型上下文。
    Quarantined,
    /// 遗忘墓碑，阻止旧摘要通过任意路径复活。
    Tombstone,
}

/// 记忆墓碑。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTombstone {
    /// 被遗忘的内容引用或摘要 hash。
    pub blocked_ref: String,
    /// 遗忘原因，不包含原文。
    pub reason: String,
}

/// 记忆记录，领域层只保存引用与治理元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecord {
    /// 记忆 ID。
    pub id: MemoryId,
    /// 来源候选 ID。
    pub candidate_id: MemoryCandidateId,
    /// 治理状态。
    pub state: MemoryState,
    /// 内容引用；quarantine/tombstone 不暴露原文。
    pub content_ref: Option<String>,
    /// 来源追踪。
    pub source_trace: Option<MemorySourceTrace>,
    /// 墓碑信息。
    pub tombstone: Option<MemoryTombstone>,
}

/// 记忆召回路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRecallPath {
    /// 精确 ID 或引用召回。
    Exact,
    /// 向量召回。
    Vector,
    /// 冷存储召回。
    Cold,
}

/// 记忆召回裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRecallDecision {
    /// 允许进入模型上下文。
    AllowContext,
    /// 仅允许审计查看，不进入模型上下文。
    AuditOnly,
    /// 阻断召回。
    Block,
}

/// 记忆准入策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAdmissionPolicy {
    /// strong memory 最低置信度。
    pub strong_min_confidence_basis_points: u16,
    /// weak memory 最低置信度。
    pub weak_min_confidence_basis_points: u16,
}

impl Default for MemoryAdmissionPolicy {
    fn default() -> Self {
        Self {
            strong_min_confidence_basis_points: MEMORY_STRONG_MIN_CONFIDENCE_BASIS_POINTS,
            weak_min_confidence_basis_points: MEMORY_WEAK_MIN_CONFIDENCE_BASIS_POINTS,
        }
    }
}

/// 记忆准入门禁。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryAdmissionGate;

impl MemoryAdmissionGate {
    /// 校验 cell 是否允许进入记忆候选抽取。
    ///
    /// # Errors
    ///
    /// 当 cell 尚未关闭或不是 `Completed` 终态时返回错误。
    pub fn ensure_cell_extractable(cell: &RuntimeCell) -> AxonResult<()> {
        if !matches!(cell.closure, CellClosureState::Closed { .. }) {
            return Err(AxonError::cell_invalid_transition(
                "RuntimeCell 未关闭，不能抽取记忆候选",
            ));
        }
        if !matches!(cell.state, RuntimeCellState::Completed(_)) {
            return Err(AxonError::cell_invalid_transition(
                "只有 Completed RuntimeCell 能抽取记忆候选",
            ));
        }
        Ok(())
    }

    /// 评估记忆候选写入决策。
    #[must_use]
    pub fn evaluate_candidate(
        candidate: &MemoryCandidate,
        policy: MemoryAdmissionPolicy,
    ) -> MemoryAdmissionDecision {
        if candidate.content_ref.trim().is_empty() {
            return memory_decision(
                MemoryWriteDecision::Reject,
                MemoryAdmissionReason::EmptyContentRef,
            );
        }

        let Some(source_trace) = candidate.source_trace.as_ref() else {
            return memory_decision(
                MemoryWriteDecision::Reject,
                MemoryAdmissionReason::MissingSourceTrace,
            );
        };
        if source_trace.summary.trim().is_empty() {
            return memory_decision(
                MemoryWriteDecision::Reject,
                MemoryAdmissionReason::EmptySourceSummary,
            );
        }

        match candidate.sensitivity {
            MemorySensitivity::Secret => {
                return memory_decision(
                    MemoryWriteDecision::Reject,
                    MemoryAdmissionReason::SecretRejected,
                );
            }
            MemorySensitivity::SecretAdjacent => {
                return memory_decision(
                    MemoryWriteDecision::Quarantine,
                    MemoryAdmissionReason::SecretAdjacentQuarantined,
                );
            }
            MemorySensitivity::Public
            | MemorySensitivity::Internal
            | MemorySensitivity::Personal => {}
        }

        if source_trace.origin == MemorySourceOrigin::RemoteUpload {
            return memory_decision(
                MemoryWriteDecision::NeedsUserReview,
                MemoryAdmissionReason::RemoteSourceRequiresReview,
            );
        }

        if candidate.confidence_basis_points < policy.weak_min_confidence_basis_points {
            return memory_decision(
                MemoryWriteDecision::Reject,
                MemoryAdmissionReason::ConfidenceTooLow,
            );
        }

        if candidate.confidence_basis_points < policy.strong_min_confidence_basis_points {
            return memory_decision(
                MemoryWriteDecision::WriteWeak,
                MemoryAdmissionReason::WeakAllowed,
            );
        }

        if user_profile_requires_weak(candidate) {
            return memory_decision(
                MemoryWriteDecision::WriteWeak,
                MemoryAdmissionReason::UserProfileDefaultsWeak,
            );
        }

        if candidate.verification == MemoryVerification::Unverified {
            return memory_decision(
                MemoryWriteDecision::NeedsUserReview,
                MemoryAdmissionReason::LocalReviewRequired,
            );
        }

        MemoryAdmissionDecision {
            decision: MemoryWriteDecision::WriteStrong,
            reason: MemoryAdmissionReason::StrongAllowed,
            strong_memory_allowed: true,
        }
    }

    /// 根据准入决策形成记忆记录。
    #[must_use]
    pub fn record_from_decision(
        id: MemoryId,
        candidate: &MemoryCandidate,
        decision: MemoryAdmissionDecision,
    ) -> Option<MemoryRecord> {
        match decision.decision {
            MemoryWriteDecision::WriteStrong => Some(memory_record(
                id,
                candidate,
                MemoryState::Strong,
                true,
                None,
            )),
            MemoryWriteDecision::WriteWeak => {
                Some(memory_record(id, candidate, MemoryState::Weak, true, None))
            }
            MemoryWriteDecision::Quarantine => Some(memory_record(
                id,
                candidate,
                MemoryState::Quarantined,
                false,
                None,
            )),
            MemoryWriteDecision::ForgetTombstone => Some(memory_record(
                id,
                candidate,
                MemoryState::Tombstone,
                false,
                Some(MemoryTombstone {
                    blocked_ref: candidate.content_ref.clone(),
                    reason: MEMORY_DEFAULT_TOMBSTONE_REASON.to_owned(),
                }),
            )),
            MemoryWriteDecision::NeedsUserReview | MemoryWriteDecision::Reject => None,
        }
    }
}

fn user_profile_requires_weak(candidate: &MemoryCandidate) -> bool {
    matches!(
        candidate.candidate_kind,
        MemoryCandidateKind::Profile | MemoryCandidateKind::UserPreference
    ) && candidate
        .source_trace
        .as_ref()
        .is_none_or(|trace| trace.origin != MemorySourceOrigin::ExplicitUserPreference)
}

/// 记忆召回门禁。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryRecallGate;

impl MemoryRecallGate {
    /// 根据状态与召回路径裁决是否可进入模型上下文。
    #[must_use]
    pub fn evaluate_record(record: &MemoryRecord, path: MemoryRecallPath) -> MemoryRecallDecision {
        match record.state {
            MemoryState::Strong | MemoryState::Weak => {
                if record.content_ref.is_some() {
                    MemoryRecallDecision::AllowContext
                } else {
                    MemoryRecallDecision::Block
                }
            }
            MemoryState::Quarantined => match path {
                MemoryRecallPath::Exact => MemoryRecallDecision::AuditOnly,
                MemoryRecallPath::Vector | MemoryRecallPath::Cold => MemoryRecallDecision::Block,
            },
            MemoryState::Tombstone => MemoryRecallDecision::Block,
        }
    }
}

fn memory_record(
    id: MemoryId,
    candidate: &MemoryCandidate,
    state: MemoryState,
    expose_content_ref: bool,
    tombstone: Option<MemoryTombstone>,
) -> MemoryRecord {
    MemoryRecord {
        id,
        candidate_id: candidate.id,
        state,
        content_ref: expose_content_ref.then(|| candidate.content_ref.clone()),
        source_trace: candidate.source_trace.clone(),
        tombstone,
    }
}

fn memory_decision(
    decision: MemoryWriteDecision,
    reason: MemoryAdmissionReason,
) -> MemoryAdmissionDecision {
    MemoryAdmissionDecision {
        decision,
        reason,
        strong_memory_allowed: false,
    }
}
