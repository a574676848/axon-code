//! MemoryRuntime 治理投影。

use axon_crypto::sha256_hex;
use axon_domain::{
    AxonError, AxonResult, MemoryAdmissionDecision, MemoryAdmissionGate, MemoryAdmissionPolicy,
    MemoryAdmissionReason, MemoryCandidate, MemoryCandidateId, MemoryId, MemoryRecallDecision,
    MemoryRecallGate, MemoryRecallPath, MemoryRecord, MemoryRetentionPolicy, MemorySensitivity,
    MemorySourceOrigin, MemoryState, MemoryWriteDecision, RuntimeCell,
};

/// Memory footer hash 展示长度。
pub const MEMORY_FOOTER_HASH_DISPLAY_LEN: usize = 12;
/// Memory Inspector hash 展示长度。
pub const MEMORY_INSPECTOR_HASH_DISPLAY_LEN: usize = 16;

/// 记忆准入 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryAdmissionRuntime;

impl MemoryAdmissionRuntime {
    /// 确保 cell 允许抽取记忆候选。
    pub fn ensure_cell_extractable(cell: &RuntimeCell) -> AxonResult<()> {
        MemoryAdmissionGate::ensure_cell_extractable(cell)
    }

    /// 评估记忆候选写入决策。
    #[must_use]
    pub fn evaluate_candidate(candidate: &MemoryCandidate) -> MemoryAdmissionDecision {
        MemoryAdmissionGate::evaluate_candidate(candidate, MemoryAdmissionPolicy::default())
    }

    /// 根据准入决策形成治理状态记录。
    #[must_use]
    pub fn record_from_decision(
        id: MemoryId,
        candidate: &MemoryCandidate,
        decision: MemoryAdmissionDecision,
    ) -> Option<MemoryRecord> {
        MemoryAdmissionGate::record_from_decision(id, candidate, decision)
    }

    /// 评估记忆记录是否允许召回。
    #[must_use]
    pub fn evaluate_recall(record: &MemoryRecord, path: MemoryRecallPath) -> MemoryRecallDecision {
        MemoryRecallGate::evaluate_record(record, path)
    }
}

/// Memory footer 准入观测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFooterAdmissionObservation {
    /// 候选 ID。
    pub candidate_id: MemoryCandidateId,
    /// 写入决策。
    pub decision: MemoryWriteDecision,
    /// 决策原因。
    pub reason: MemoryAdmissionReason,
    /// 来源类型。
    pub source_origin: Option<MemorySourceOrigin>,
    /// 置信度，单位 basis point。
    pub confidence_basis_points: u16,
    /// 敏感等级。
    pub sensitivity: MemorySensitivity,
    /// 保留策略。
    pub retention: MemoryRetentionPolicy,
    /// 摘要 hash；不得传入原文。
    pub summary_hash: String,
}

/// Memory footer 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFooterInput {
    /// 已形成治理状态的记忆记录。
    pub records: Vec<MemoryRecord>,
    /// 本轮准入观测。
    pub admissions: Vec<MemoryFooterAdmissionObservation>,
}

/// Memory footer 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFooterProjection {
    /// strong 记忆数。
    pub strong_count: usize,
    /// weak 记忆数。
    pub weak_count: usize,
    /// 隔离记忆数。
    pub quarantined_count: usize,
    /// tombstone 数。
    pub tombstone_count: usize,
    /// 拒绝数。
    pub rejected_count: usize,
    /// 待本机复核数。
    pub pending_review_count: usize,
    /// 已截断的原因与 hash。
    pub reason_hashes: Vec<MemoryFooterReasonHash>,
    /// footer 文案，不包含原文。
    pub footer_text: String,
    /// 投影事件。
    pub events: Vec<MemoryFooterEvent>,
}

/// Memory Inspector 允许动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryInspectorAction {
    /// 打开本机审批视图。
    OpenLocalReview,
    /// 本机遗忘该记忆。
    Forget,
    /// 本机隔离该记忆。
    Quarantine,
    /// 本机提升为 strong memory。
    PromoteToStrong,
    /// 本机合并冲突记忆。
    Merge,
}

/// Memory Inspector 条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInspectorEntry {
    /// 记忆 ID。
    pub memory_id: MemoryId,
    /// 治理状态。
    pub state: MemoryState,
    /// 来源类型。
    pub source_origin: Option<MemorySourceOrigin>,
    /// 关联 session。
    pub session_id: Option<axon_domain::SessionId>,
    /// 关联 cell。
    pub cell_id: Option<axon_domain::CellId>,
    /// 可审计证据引用。
    pub evidence_ref: Option<String>,
    /// 置信度，单位 basis point。
    pub confidence_basis_points: u16,
    /// 敏感等级。
    pub sensitivity: MemorySensitivity,
    /// 保留策略。
    pub retention: MemoryRetentionPolicy,
    /// 健康原因。
    pub reason: MemoryAdmissionReason,
    /// 摘要 hash，不展示摘要原文。
    pub summary_hash: String,
    /// 允许动作。
    pub allowed_actions: Vec<MemoryInspectorAction>,
    /// 是否可见原文。
    pub raw_content_visible: bool,
    /// 远控是否允许执行动作。
    pub remote_action_allowed: bool,
}

/// Memory Inspector 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInspectorInput {
    /// 已形成治理状态的记忆记录。
    pub records: Vec<MemoryRecord>,
    /// 本轮准入观测。
    pub admissions: Vec<MemoryFooterAdmissionObservation>,
}

/// Memory Inspector 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInspectorProjection {
    /// Inspector 条目。
    pub entries: Vec<MemoryInspectorEntry>,
    /// 投影事件。
    pub events: Vec<MemoryInspectorEvent>,
}

/// Memory Inspector 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryInspectorEvent {
    /// Inspector 已刷新。
    InspectorProjected {
        /// 条目数量。
        entry_count: usize,
    },
}

/// Memory footer reason/hash 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFooterReasonHash {
    /// 决策原因。
    pub reason: MemoryAdmissionReason,
    /// 摘要 hash。
    pub summary_hash: String,
}

/// Memory footer 投影事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryFooterEvent {
    /// Memory Health 已刷新。
    MemoryHealthChanged {
        /// strong 记忆数。
        strong_count: usize,
        /// weak 记忆数。
        weak_count: usize,
        /// 隔离记忆数。
        quarantined_count: usize,
        /// 拒绝数。
        rejected_count: usize,
    },
}

/// Memory footer runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryFooterRuntime;

impl MemoryFooterRuntime {
    /// 生成 Memory footer 投影。
    pub fn project(&self, input: &MemoryFooterInput) -> AxonResult<MemoryFooterProjection> {
        validate_observations(&input.admissions)?;

        let counts = memory_counts(&input.records, &input.admissions);
        let reason_hashes = input
            .admissions
            .iter()
            .map(|observation| MemoryFooterReasonHash {
                reason: observation.reason,
                summary_hash: trim_hash(&observation.summary_hash),
            })
            .collect::<Vec<_>>();
        let footer_text = memory_footer_text(&counts, &reason_hashes);
        let events = vec![MemoryFooterEvent::MemoryHealthChanged {
            strong_count: counts.strong,
            weak_count: counts.weak,
            quarantined_count: counts.quarantined,
            rejected_count: counts.rejected,
        }];

        Ok(MemoryFooterProjection {
            strong_count: counts.strong,
            weak_count: counts.weak,
            quarantined_count: counts.quarantined,
            tombstone_count: counts.tombstones,
            rejected_count: counts.rejected,
            pending_review_count: counts.pending_review,
            reason_hashes,
            footer_text,
            events,
        })
    }

    /// 从候选生成不含原文的准入观测。
    #[must_use]
    pub fn admission_observation(
        candidate: &MemoryCandidate,
        decision: MemoryAdmissionDecision,
    ) -> MemoryFooterAdmissionObservation {
        let source_origin = candidate.source_trace.as_ref().map(|trace| trace.origin);
        let summary_hash = candidate.source_trace.as_ref().map_or_else(
            || stable_hash(&candidate.content_ref),
            |trace| stable_hash(&trace.summary),
        );

        MemoryFooterAdmissionObservation {
            candidate_id: candidate.id,
            decision: decision.decision,
            reason: decision.reason,
            source_origin,
            confidence_basis_points: candidate.confidence_basis_points,
            sensitivity: candidate.sensitivity,
            retention: candidate.proposed_retention,
            summary_hash,
        }
    }
}

/// Memory Inspector runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryInspectorRuntime;

impl MemoryInspectorRuntime {
    /// 生成 Memory Inspector 最小投影。
    pub fn project(&self, input: &MemoryInspectorInput) -> AxonResult<MemoryInspectorProjection> {
        validate_observations(&input.admissions)?;

        let entries = input
            .records
            .iter()
            .map(|record| {
                let admission = input
                    .admissions
                    .iter()
                    .find(|item| item.candidate_id == record.candidate_id);
                inspector_entry(record, admission)
            })
            .collect::<Vec<_>>();
        let events = vec![MemoryInspectorEvent::InspectorProjected {
            entry_count: entries.len(),
        }];

        Ok(MemoryInspectorProjection { entries, events })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct MemoryFooterCounts {
    strong: usize,
    weak: usize,
    quarantined: usize,
    tombstones: usize,
    rejected: usize,
    pending_review: usize,
}

fn memory_counts(
    records: &[MemoryRecord],
    admissions: &[MemoryFooterAdmissionObservation],
) -> MemoryFooterCounts {
    let mut counts = MemoryFooterCounts::default();
    for record in records {
        match record.state {
            MemoryState::Strong => counts.strong += 1,
            MemoryState::Weak => counts.weak += 1,
            MemoryState::Quarantined => counts.quarantined += 1,
            MemoryState::Tombstone => counts.tombstones += 1,
        }
    }
    for admission in admissions {
        match admission.decision {
            MemoryWriteDecision::Reject => counts.rejected += 1,
            MemoryWriteDecision::NeedsUserReview => counts.pending_review += 1,
            MemoryWriteDecision::Quarantine
            | MemoryWriteDecision::WriteStrong
            | MemoryWriteDecision::WriteWeak
            | MemoryWriteDecision::ForgetTombstone => {}
        }
    }
    counts
}

fn validate_observations(admissions: &[MemoryFooterAdmissionObservation]) -> AxonResult<()> {
    for admission in admissions {
        if admission.summary_hash.trim().is_empty() {
            return Err(AxonError::runtime_failure("memory footer hash 不能为空"));
        }
    }
    Ok(())
}

fn inspector_entry(
    record: &MemoryRecord,
    admission: Option<&MemoryFooterAdmissionObservation>,
) -> MemoryInspectorEntry {
    let source = record.source_trace.as_ref();
    let summary_hash = source.map_or_else(
        || {
            record
                .content_ref
                .as_ref()
                .map_or_else(String::new, |value| stable_hash(value))
        },
        |trace| stable_hash(&trace.summary),
    );
    let reason = admission.map_or_else(|| memory_state_reason(record.state), |item| item.reason);

    MemoryInspectorEntry {
        memory_id: record.id,
        state: record.state,
        source_origin: source.map(|trace| trace.origin),
        session_id: source.and_then(|trace| trace.session_id),
        cell_id: source.and_then(|trace| trace.cell_id),
        evidence_ref: source.and_then(|trace| trace.evidence_ref.clone()),
        confidence_basis_points: admission.map_or(0, |item| item.confidence_basis_points),
        sensitivity: admission.map_or(MemorySensitivity::Internal, |item| item.sensitivity),
        retention: admission.map_or(MemoryRetentionPolicy::Project, |item| item.retention),
        reason,
        summary_hash: trim_inspector_hash(&summary_hash),
        allowed_actions: inspector_actions(record.state),
        raw_content_visible: false,
        remote_action_allowed: false,
    }
}

fn memory_state_reason(state: MemoryState) -> MemoryAdmissionReason {
    match state {
        MemoryState::Weak => MemoryAdmissionReason::WeakAllowed,
        MemoryState::Quarantined => MemoryAdmissionReason::SecretAdjacentQuarantined,
        MemoryState::Strong | MemoryState::Tombstone => MemoryAdmissionReason::StrongAllowed,
    }
}

fn inspector_actions(state: MemoryState) -> Vec<MemoryInspectorAction> {
    match state {
        MemoryState::Strong => vec![MemoryInspectorAction::Forget, MemoryInspectorAction::Merge],
        MemoryState::Weak => vec![
            MemoryInspectorAction::OpenLocalReview,
            MemoryInspectorAction::PromoteToStrong,
            MemoryInspectorAction::Forget,
        ],
        MemoryState::Quarantined => vec![
            MemoryInspectorAction::OpenLocalReview,
            MemoryInspectorAction::Forget,
        ],
        MemoryState::Tombstone => Vec::new(),
    }
}

fn memory_footer_text(
    counts: &MemoryFooterCounts,
    reason_hashes: &[MemoryFooterReasonHash],
) -> String {
    if reason_hashes.is_empty() {
        return String::new();
    }
    let strong_writes = reason_hashes
        .iter()
        .filter(|item| item.reason == MemoryAdmissionReason::StrongAllowed)
        .count();
    let quarantined = counts.quarantined.saturating_add(counts.rejected);
    if quarantined > 0 {
        return format!("memory 隔离 {quarantined} 条");
    }
    if strong_writes > 0 {
        return format!("memory 更新 {strong_writes} 条");
    }
    "memory 已观察".to_owned()
}

fn stable_hash(value: &str) -> String {
    sha256_hex(value.as_bytes())
}

fn trim_hash(value: &str) -> String {
    value.chars().take(MEMORY_FOOTER_HASH_DISPLAY_LEN).collect()
}

fn trim_inspector_hash(value: &str) -> String {
    value
        .chars()
        .take(MEMORY_INSPECTOR_HASH_DISPLAY_LEN)
        .collect()
}
