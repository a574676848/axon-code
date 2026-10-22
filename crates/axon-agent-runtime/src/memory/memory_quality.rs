//! Memory 质量评估与离线守门。

use std::collections::{BTreeMap, BTreeSet};

use axon_domain::{AxonError, AxonResult, MemoryState};

use axon_memory::{
    MemoryOrchestrator, MemoryRuntimeQualityReport, MemoryRuntimeQualityStore, MemorySqliteRecord,
};

const MEMORY_DEFAULT_NAMESPACE_REF: &str = "memory-scope:local";

/// 最大污染率，单位 basis point。
pub const MEMORY_QUALITY_MAX_POLLUTION_BASIS_POINTS: u16 = 0;
/// 最大旧事实残留率，单位 basis point。
pub const MEMORY_QUALITY_MAX_STALE_FACT_BASIS_POINTS: u16 = 0;
/// 最大误召回风险率，单位 basis point。
pub const MEMORY_QUALITY_MAX_FALSE_RECALL_BASIS_POINTS: u16 = 500;
/// 最大活跃重复残留率，单位 basis point。
pub const MEMORY_QUALITY_MAX_DUPLICATE_ACTIVE_RESIDUAL_BASIS_POINTS: u16 = 0;
/// Golden set 最低通过率，单位 basis point。
pub const MEMORY_QUALITY_MIN_GOLDEN_PASS_BASIS_POINTS: u16 = 9_000;
/// Memory 质量评估通过原因。
pub const MEMORY_QUALITY_REASON_PASSED: &str = "memory quality gate passed";
/// Memory 质量评估失败原因。
pub const MEMORY_QUALITY_REASON_FAILED: &str = "memory quality gate failed";
/// Golden set 评估通过原因。
pub const MEMORY_QUALITY_GOLDEN_REASON_PASSED: &str = "memory golden set gate passed";
/// Golden set 评估失败原因。
pub const MEMORY_QUALITY_GOLDEN_REASON_FAILED: &str = "memory golden set gate failed";

/// Memory 质量评估输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQualityEvaluationInput<'a> {
    /// Memory 编排器。
    pub orchestrator: &'a MemoryOrchestrator,
    /// 当前时间。
    pub now_ms: u64,
}

/// Memory 质量评估报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQualityEvaluationReport {
    /// 总记录数。
    pub total_records: usize,
    /// 活跃记录数。
    pub active_records: usize,
    /// 污染记录数。
    pub pollution_records: usize,
    /// 旧事实残留记录数。
    pub stale_fact_residual_records: usize,
    /// 误召回风险记录数。
    pub false_recall_risk_records: usize,
    /// 活跃重复记录残留数。
    pub duplicate_active_residual_records: usize,
    /// 污染率，单位 basis point。
    pub pollution_basis_points: u16,
    /// 旧事实残留率，单位 basis point。
    pub stale_fact_basis_points: u16,
    /// 误召回风险率，单位 basis point。
    pub false_recall_basis_points: u16,
    /// 活跃重复残留率，单位 basis point。
    pub duplicate_active_residual_basis_points: u16,
    /// Provider cache、深反思产出与错误 Memory 拒绝指标。
    pub runtime: MemoryRuntimeQualityReport,
    /// 是否仍保持 ref-only。
    pub ref_only: bool,
    /// 是否通过质量门禁。
    pub quality_gate_passed: bool,
    /// 报告原因。
    pub reason: &'static str,
}

/// Memory golden set 单条样本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGoldenCase {
    /// 样本 ID。
    pub case_id: String,
    /// 查询文本。
    pub query_text: String,
    /// 期望命中的 content ref。
    pub expected_content_ref: String,
    /// 不允许命中的 content ref。
    pub forbidden_content_refs: Vec<String>,
}

/// Memory golden set 单条命中摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGoldenHit {
    /// 命中的 content ref。
    pub content_ref: String,
    /// 排名，从 1 开始。
    pub rank: usize,
    /// 分数，单位 basis point。
    pub score_basis_points: u16,
}

/// Memory golden set 单条结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGoldenCaseResult {
    /// 样本 ID。
    pub case_id: String,
    /// 查询文本。
    pub query_text: String,
    /// 期望命中的 content ref。
    pub expected_content_ref: String,
    /// 是否命中期望。
    pub expected_hit: bool,
    /// 是否命中禁召回项。
    pub forbidden_hit: bool,
    /// 期望命中排名。
    pub expected_rank: Option<usize>,
    /// 首条命中 content ref。
    pub top_content_ref: Option<String>,
}

/// Memory golden set 评估输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGoldenSetEvaluationInput {
    /// 固定样本集合。
    pub cases: Vec<MemoryGoldenCase>,
    /// 每个 case 的实际召回命中。
    pub hits_by_case: Vec<MemoryGoldenCaseHits>,
}

/// Memory golden set case 命中集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGoldenCaseHits {
    /// 样本 ID。
    pub case_id: String,
    /// 实际召回命中。
    pub hits: Vec<MemoryGoldenHit>,
}

/// Memory golden set 评估报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGoldenSetEvaluationReport {
    /// 样本数。
    pub total_cases: usize,
    /// 期望命中数。
    pub expected_hits: usize,
    /// 禁召回命中数。
    pub forbidden_hits: usize,
    /// 缺失结果数。
    pub missing_cases: usize,
    /// 通过率，单位 basis point。
    pub pass_basis_points: u16,
    /// 防踩坑命中数。
    pub avoidance_hits: usize,
    /// 防踩坑命中率，单位 basis point。
    pub avoidance_basis_points: u16,
    /// 是否通过 golden set 门禁。
    pub golden_gate_passed: bool,
    /// 每个 case 的详细结果。
    pub case_results: Vec<MemoryGoldenCaseResult>,
    /// 报告原因。
    pub reason: &'static str,
}

/// Memory 质量评估 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryQualityEvaluationRuntime;

impl MemoryQualityEvaluationRuntime {
    /// 对折叠后的 Memory store 执行 O(n) 离线质量评估。
    pub fn evaluate(
        &self,
        input: &MemoryQualityEvaluationInput<'_>,
    ) -> AxonResult<MemoryQualityEvaluationReport> {
        if input.now_ms == 0 {
            return Err(AxonError::runtime_failure("Memory 质量评估输入无效"));
        }
        let records = input.orchestrator.sqlite_store()?.records()?;

        let active_records = records
            .iter()
            .filter(|record| record.archived_at_ms.is_none())
            .count();
        let pollution_records = records.iter().filter(|record| is_polluted(record)).count();
        let stale_fact_residual_records = stale_fact_residual_count(&records, input.now_ms);
        let false_recall_risk_records = records
            .iter()
            .filter(|record| is_false_recall_risk(record, input.now_ms))
            .count();
        let duplicate_active_residual_records = duplicate_active_residual_count(&records);
        let ref_only = records
            .iter()
            .all(|record| !record.prompt_text_persisted && !record.raw_payload_persisted);
        let pollution_basis_points = rate_basis_points(pollution_records, records.len());
        let stale_fact_basis_points = rate_basis_points(stale_fact_residual_records, records.len());
        let false_recall_basis_points = rate_basis_points(false_recall_risk_records, records.len());
        let duplicate_active_residual_basis_points =
            rate_basis_points(duplicate_active_residual_records, records.len());
        let runtime = MemoryRuntimeQualityStore::open(input.orchestrator.config())?.report()?;
        let quality_gate_passed = ref_only
            && pollution_basis_points == MEMORY_QUALITY_MAX_POLLUTION_BASIS_POINTS
            && stale_fact_basis_points == MEMORY_QUALITY_MAX_STALE_FACT_BASIS_POINTS
            && false_recall_basis_points <= MEMORY_QUALITY_MAX_FALSE_RECALL_BASIS_POINTS
            && duplicate_active_residual_basis_points
                == MEMORY_QUALITY_MAX_DUPLICATE_ACTIVE_RESIDUAL_BASIS_POINTS;

        Ok(MemoryQualityEvaluationReport {
            total_records: records.len(),
            active_records,
            pollution_records,
            stale_fact_residual_records,
            false_recall_risk_records,
            duplicate_active_residual_records,
            pollution_basis_points,
            stale_fact_basis_points,
            false_recall_basis_points,
            duplicate_active_residual_basis_points,
            runtime,
            ref_only,
            quality_gate_passed,
            reason: if quality_gate_passed {
                MEMORY_QUALITY_REASON_PASSED
            } else {
                MEMORY_QUALITY_REASON_FAILED
            },
        })
    }

    /// 对固定查询集合执行 golden set 质量评估。
    pub fn evaluate_golden_set(
        &self,
        input: &MemoryGoldenSetEvaluationInput,
    ) -> AxonResult<MemoryGoldenSetEvaluationReport> {
        validate_golden_set(input)?;
        let hits_by_case = input
            .hits_by_case
            .iter()
            .map(|case_hits| (case_hits.case_id.clone(), case_hits))
            .collect::<BTreeMap<_, _>>();
        let mut expected_hits = 0usize;
        let mut forbidden_hits = 0usize;
        let mut missing_cases = 0usize;
        let mut case_results = Vec::with_capacity(input.cases.len());

        for case in &input.cases {
            let Some(case_hits) = hits_by_case.get(&case.case_id) else {
                missing_cases = missing_cases.saturating_add(1);
                case_results.push(missing_case_result(case));
                continue;
            };
            let expected_rank = expected_rank(case, &case_hits.hits);
            let forbidden_hit = has_forbidden_hit(case, &case_hits.hits);
            if expected_rank.is_some() {
                expected_hits = expected_hits.saturating_add(1);
            }
            if forbidden_hit {
                forbidden_hits = forbidden_hits.saturating_add(1);
            }
            case_results.push(MemoryGoldenCaseResult {
                case_id: case.case_id.clone(),
                query_text: case.query_text.clone(),
                expected_content_ref: case.expected_content_ref.clone(),
                expected_hit: expected_rank.is_some(),
                forbidden_hit,
                expected_rank,
                top_content_ref: case_hits.hits.first().map(|hit| hit.content_ref.clone()),
            });
        }

        let pass_basis_points = rate_basis_points(
            expected_hits.saturating_sub(forbidden_hits.saturating_add(missing_cases)),
            input.cases.len(),
        );
        let avoidance_hits = input.cases.len().saturating_sub(forbidden_hits);
        let avoidance_basis_points = rate_basis_points(avoidance_hits, input.cases.len());
        let golden_gate_passed = forbidden_hits == 0
            && missing_cases == 0
            && pass_basis_points >= MEMORY_QUALITY_MIN_GOLDEN_PASS_BASIS_POINTS;

        Ok(MemoryGoldenSetEvaluationReport {
            total_cases: input.cases.len(),
            expected_hits,
            forbidden_hits,
            missing_cases,
            pass_basis_points,
            avoidance_hits,
            avoidance_basis_points,
            golden_gate_passed,
            case_results,
            reason: if golden_gate_passed {
                MEMORY_QUALITY_GOLDEN_REASON_PASSED
            } else {
                MEMORY_QUALITY_GOLDEN_REASON_FAILED
            },
        })
    }
}

fn validate_golden_set(input: &MemoryGoldenSetEvaluationInput) -> AxonResult<()> {
    if input.cases.is_empty() {
        return Err(AxonError::runtime_failure("Memory golden set 样本数量无效"));
    }
    let mut case_ids = BTreeSet::<&str>::new();
    for case in &input.cases {
        if case.case_id.trim().is_empty()
            || case.query_text.trim().is_empty()
            || case.expected_content_ref.trim().is_empty()
            || !case_ids.insert(case.case_id.as_str())
        {
            return Err(AxonError::runtime_failure("Memory golden set 样本无效"));
        }
    }
    for case_hits in &input.hits_by_case {
        if case_hits.case_id.trim().is_empty() {
            return Err(AxonError::runtime_failure("Memory golden set 命中结果无效"));
        }
        for hit in &case_hits.hits {
            if hit.content_ref.trim().is_empty() || hit.rank == 0 {
                return Err(AxonError::runtime_failure("Memory golden set 命中项无效"));
            }
        }
    }
    Ok(())
}

fn missing_case_result(case: &MemoryGoldenCase) -> MemoryGoldenCaseResult {
    MemoryGoldenCaseResult {
        case_id: case.case_id.clone(),
        query_text: case.query_text.clone(),
        expected_content_ref: case.expected_content_ref.clone(),
        expected_hit: false,
        forbidden_hit: false,
        expected_rank: None,
        top_content_ref: None,
    }
}

fn expected_rank(case: &MemoryGoldenCase, hits: &[MemoryGoldenHit]) -> Option<usize> {
    hits.iter()
        .find(|hit| hit.content_ref == case.expected_content_ref)
        .map(|hit| hit.rank)
}

fn has_forbidden_hit(case: &MemoryGoldenCase, hits: &[MemoryGoldenHit]) -> bool {
    hits.iter().any(|hit| {
        case.forbidden_content_refs
            .iter()
            .any(|forbidden| forbidden == &hit.content_ref)
    })
}

fn is_polluted(record: &MemorySqliteRecord) -> bool {
    record.prompt_text_persisted
        || record.raw_payload_persisted
        || record.remote_write_allowed
        || (record.archived_at_ms.is_none()
            && matches!(
                record.record.state,
                MemoryState::Quarantined | MemoryState::Tombstone
            ))
}

fn stale_fact_residual_count(records: &[MemorySqliteRecord], now_ms: u64) -> usize {
    let expired_active = records
        .iter()
        .filter(|record| {
            record.archived_at_ms.is_none()
                && record
                    .ttl_expires_at_ms
                    .is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
        })
        .count();
    expired_active.saturating_add(conflicting_active_residual_count(records))
}

fn conflicting_active_residual_count(records: &[MemorySqliteRecord]) -> usize {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for record in records.iter().filter(|record| is_active_recallable(record)) {
        let Some(key) = scoped_evidence_key(record) else {
            continue;
        };
        let Some(content_ref) = record.record.content_ref.clone() else {
            continue;
        };
        grouped.entry(key).or_default().push(content_ref);
    }
    grouped
        .into_values()
        .filter(|content_refs| has_conflicting_content(content_refs))
        .map(|content_refs| content_refs.len().saturating_sub(1))
        .sum()
}

fn duplicate_active_residual_count(records: &[MemorySqliteRecord]) -> usize {
    let mut grouped = BTreeMap::<String, usize>::new();
    for record in records.iter().filter(|record| is_active_recallable(record)) {
        let Some(content_ref) = record.record.content_ref.clone() else {
            continue;
        };
        *grouped.entry(content_ref).or_default() += 1;
    }
    grouped
        .into_values()
        .filter(|count| *count > 1)
        .map(|count| count.saturating_sub(1))
        .sum()
}

fn is_false_recall_risk(record: &MemorySqliteRecord, now_ms: u64) -> bool {
    let source = record.record.source_trace.as_ref();
    is_active_recallable(record)
        && record
            .ttl_expires_at_ms
            .is_none_or(|expires_at_ms| expires_at_ms > now_ms)
        && source
            .and_then(|trace| trace.evidence_ref.as_ref())
            .is_none_or(|value| value.trim().is_empty())
        && source.is_none_or(|trace| trace.summary.trim().is_empty())
}

fn is_active_recallable(record: &MemorySqliteRecord) -> bool {
    record.archived_at_ms.is_none()
        && matches!(record.record.state, MemoryState::Weak | MemoryState::Strong)
        && record
            .record
            .content_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn scoped_evidence_key(record: &MemorySqliteRecord) -> Option<String> {
    let source = record.record.source_trace.as_ref()?;
    let evidence_key = source
        .evidence_ref
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| (!source.summary.trim().is_empty()).then(|| source.summary.clone()))?;
    Some(format!(
        "{}:{}:{}:{:?}:{}:{}:{evidence_key}",
        record
            .namespace_ref
            .as_deref()
            .unwrap_or(MEMORY_DEFAULT_NAMESPACE_REF),
        record.repo_ref.as_deref().unwrap_or("repo-scope:any"),
        record.user_ref.as_deref().unwrap_or("user-scope:any"),
        source.origin,
        source.session_id.map_or(0, |id| id.0),
        source.cell_id.map_or(0, |id| id.0)
    ))
}

fn has_conflicting_content(content_refs: &[String]) -> bool {
    content_refs
        .first()
        .is_some_and(|first| content_refs.iter().any(|value| value != first))
}

fn rate_basis_points(count: usize, total: usize) -> u16 {
    if total == 0 {
        return 0;
    }
    let rate = count.saturating_mul(10_000) / total;
    u16::try_from(rate).unwrap_or(u16::MAX)
}
