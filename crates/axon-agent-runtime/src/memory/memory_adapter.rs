//! Memory adapter 可替换边界。

use axon_domain::{
    AxonResult, CellId, MemoryId, MemoryRecallDecision, MemoryRecallGate, MemoryRecallPath,
    MemoryRecord, MemorySourceOrigin, MemorySourceTrace, SessionId,
};

/// Memory adapter 能力声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAdapterCapabilities {
    /// 是否支持 exact recall。
    pub exact_recall: bool,
    /// 是否支持 vector recall。
    pub vector_recall: bool,
    /// 是否支持 cold recall。
    pub cold_recall: bool,
    /// 是否绑定了具体向量库。
    pub concrete_vector_store_bound: bool,
}

impl MemoryAdapterCapabilities {
    /// M1 默认能力：冻结边界，不绑定具体向量库。
    #[must_use]
    pub const fn m1_unbound() -> Self {
        Self {
            exact_recall: true,
            vector_recall: true,
            cold_recall: true,
            concrete_vector_store_bound: false,
        }
    }
}

/// Memory adapter 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAdapterKind {
    /// 仅用于本机确定性合同验证。
    DeterministicLocal,
    /// 外部实现，M1 不冻结物理 schema。
    External,
}

/// Memory recall 查询。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecallQuery {
    /// 召回路径。
    pub path: MemoryRecallPath,
    /// 最大返回数量。
    pub limit: usize,
    /// 查询证据 hash；不得包含原文。
    pub evidence_hash: String,
}

/// Memory adapter 原始命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAdapterHit {
    /// 命中的记忆记录。
    pub record: MemoryRecord,
    /// adapter 层分数，单位 basis point。
    pub score_basis_points: u16,
    /// 命中证据 hash；不得包含原文。
    pub matched_hash: String,
}

/// Memory recall 命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecallHit {
    /// 命中的记忆 ID。
    pub memory_id: MemoryId,
    /// 召回裁决。
    pub decision: MemoryRecallDecision,
    /// adapter 层分数，单位 basis point。
    pub score_basis_points: u16,
    /// 命中证据 hash。
    pub matched_hash: String,
    /// 来源追踪投影，不包含原始摘要。
    pub source_trace: Option<MemoryRecallSourceTrace>,
    /// 召回适用范围。
    pub applicability_scope: MemoryRecallApplicabilityScope,
}

/// Memory recall 来源追踪投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecallSourceTrace {
    /// 来源类型。
    pub origin: MemorySourceOrigin,
    /// 关联 session。
    pub session_id: Option<SessionId>,
    /// 关联 RuntimeCell。
    pub cell_id: Option<CellId>,
    /// 是否存在可审计证据引用。
    pub evidence_ref_present: bool,
}

/// Memory recall 适用范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRecallApplicabilityScope {
    /// 仅当前 session。
    Session,
    /// 当前项目。
    Project,
    /// 长期偏好或画像。
    LongTerm,
    /// 缺少 source trace，不能声明适用范围。
    Unspecified,
}

/// Memory recall 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecallResult {
    /// 允许进入上下文的命中。
    pub allowed: Vec<MemoryRecallHit>,
    /// 被治理门禁阻断或仅审计的命中。
    pub rejected: Vec<MemoryRecallHit>,
    /// 是否因为上限发生截断。
    pub truncated: bool,
}

/// Memory store adapter。
pub trait MemoryStoreAdapter {
    /// adapter 类型。
    fn kind(&self) -> MemoryAdapterKind;
    /// adapter 能力。
    fn capabilities(&self) -> MemoryAdapterCapabilities;
    /// 写入或替换治理记录。
    fn upsert(&mut self, record: MemoryRecord) -> AxonResult<()>;
    /// 召回候选；返回值仍必须经过 runtime recall gate。
    fn recall_candidates(&self, query: &MemoryRecallQuery) -> AxonResult<Vec<MemoryAdapterHit>>;
}

/// Memory adapter runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryAdapterRuntime;

impl MemoryAdapterRuntime {
    /// 校验 adapter 未在 M1 绑定具体向量库。
    #[must_use]
    pub fn accepts_m1_adapter(capabilities: MemoryAdapterCapabilities) -> bool {
        !capabilities.concrete_vector_store_bound
    }

    /// 执行召回并先应用治理门禁，再返回上下文命中。
    pub fn recall(
        &self,
        adapter: &dyn MemoryStoreAdapter,
        query: &MemoryRecallQuery,
    ) -> AxonResult<MemoryRecallResult> {
        let capabilities = adapter.capabilities();
        if !Self::accepts_m1_adapter(capabilities) {
            return Ok(MemoryRecallResult {
                allowed: Vec::new(),
                rejected: Vec::new(),
                truncated: false,
            });
        }

        let mut candidates = adapter.recall_candidates(query)?;
        let truncated = candidates.len() > query.limit;
        candidates.truncate(query.limit);

        let mut result = MemoryRecallResult {
            allowed: Vec::new(),
            rejected: Vec::new(),
            truncated,
        };
        for candidate in candidates {
            let decision = MemoryRecallGate::evaluate_record(&candidate.record, query.path);
            let hit = MemoryRecallHit {
                memory_id: candidate.record.id,
                decision,
                score_basis_points: candidate.score_basis_points,
                matched_hash: candidate.matched_hash,
                source_trace: candidate
                    .record
                    .source_trace
                    .as_ref()
                    .map(memory_recall_source_trace),
                applicability_scope: memory_recall_applicability_scope(&candidate.record),
            };
            match decision {
                MemoryRecallDecision::AllowContext => result.allowed.push(hit),
                MemoryRecallDecision::AuditOnly | MemoryRecallDecision::Block => {
                    result.rejected.push(hit);
                }
            }
        }
        Ok(result)
    }
}

fn memory_recall_source_trace(trace: &MemorySourceTrace) -> MemoryRecallSourceTrace {
    MemoryRecallSourceTrace {
        origin: trace.origin,
        session_id: trace.session_id,
        cell_id: trace.cell_id,
        evidence_ref_present: trace.evidence_ref.is_some(),
    }
}

fn memory_recall_applicability_scope(record: &MemoryRecord) -> MemoryRecallApplicabilityScope {
    let Some(trace) = record.source_trace.as_ref() else {
        return MemoryRecallApplicabilityScope::Unspecified;
    };
    match trace.origin {
        MemorySourceOrigin::AgentUserContext | MemorySourceOrigin::ExplicitUserPreference => {
            MemoryRecallApplicabilityScope::LongTerm
        }
        MemorySourceOrigin::AgentExperience
        | MemorySourceOrigin::AgentKnowledge
        | MemorySourceOrigin::DebuggingExperience => MemoryRecallApplicabilityScope::Project,
        MemorySourceOrigin::AgentProcedure | MemorySourceOrigin::SkillReflection => {
            MemoryRecallApplicabilityScope::Project
        }
        MemorySourceOrigin::LocalTimeline
        | MemorySourceOrigin::RemoteUpload
        | MemorySourceOrigin::CommandCompletionFeedback => MemoryRecallApplicabilityScope::Session,
    }
}

#[cfg(test)]
mod tests {
    use axon_domain::{MemoryCandidateId, MemoryState};

    use super::*;

    struct TestAdapter {
        hits: Vec<MemoryAdapterHit>,
    }

    impl MemoryStoreAdapter for TestAdapter {
        fn kind(&self) -> MemoryAdapterKind {
            MemoryAdapterKind::DeterministicLocal
        }

        fn capabilities(&self) -> MemoryAdapterCapabilities {
            MemoryAdapterCapabilities::m1_unbound()
        }

        fn upsert(&mut self, _record: MemoryRecord) -> AxonResult<()> {
            Ok(())
        }

        fn recall_candidates(
            &self,
            _query: &MemoryRecallQuery,
        ) -> AxonResult<Vec<MemoryAdapterHit>> {
            Ok(self.hits.clone())
        }
    }

    #[test]
    fn recall_honors_explicit_limit_above_legacy_adapter_cap() {
        const REQUESTED_LIMIT: usize = 24;
        let adapter = TestAdapter {
            hits: (1..=REQUESTED_LIMIT)
                .map(|id| MemoryAdapterHit {
                    record: MemoryRecord {
                        id: MemoryId(id as u64),
                        candidate_id: MemoryCandidateId(id as u64),
                        state: MemoryState::Strong,
                        content_ref: Some(format!("memory-ref:{id}")),
                        source_trace: None,
                        tombstone: None,
                    },
                    score_basis_points: 9_000,
                    matched_hash: format!("hash:{id}"),
                })
                .collect(),
        };
        let result = MemoryAdapterRuntime
            .recall(
                &adapter,
                &MemoryRecallQuery {
                    path: MemoryRecallPath::Exact,
                    limit: REQUESTED_LIMIT,
                    evidence_hash: "query-hash".to_owned(),
                },
            )
            .unwrap();

        assert_eq!(result.allowed.len(), REQUESTED_LIMIT);
        assert!(!result.truncated);
    }
}
