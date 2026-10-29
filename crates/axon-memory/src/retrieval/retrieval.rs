//! Memory 混合检索与治理元数据重排。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult, MemoryId, MemoryState};

use crate::{MemoryOrchestrator, MemorySqliteRecord};

/// Hybrid 检索候选扩展倍数。
pub const MEMORY_HYBRID_CANDIDATE_MULTIPLIER: usize = 4;
/// 文本 rank 融合权重，单位 basis point。
pub const MEMORY_HYBRID_TEXT_WEIGHT_BASIS_POINTS: u32 = 4_500;
/// 向量 rank 融合权重，单位 basis point。
pub const MEMORY_HYBRID_VECTOR_WEIGHT_BASIS_POINTS: u32 = 4_500;
/// 置信度/重要度治理权重，单位 basis point。
pub const MEMORY_HYBRID_GOVERNANCE_WEIGHT_BASIS_POINTS: u32 = 1_000;
/// basis point 最大值。
const MAX_BASIS_POINTS: u32 = 10_000;

/// Memory 混合检索查询。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHybridQuery {
    /// 文本查询；用于 Tantivy BM25 与文本退化。
    pub query_text: String,
    /// 查询 embedding；未提供时只走文本召回。
    pub query_embedding: Option<Vec<f32>>,
    /// 返回数量上限。
    pub limit: usize,
    /// 当前时间，用于 TTL 过滤。
    pub now_ms: u64,
    /// 可选作用域过滤；提供时只召回该用户命名空间内的记录。
    pub scope: Option<MemoryHybridScope>,
}

/// Memory hybrid 检索作用域。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHybridScope {
    /// owner principal。
    pub user_ref: Option<String>,
    /// namespace 引用。
    pub namespace_ref: String,
}

/// Memory 混合检索命中。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHybridHit {
    /// SQLite 治理记录。
    pub record: MemorySqliteRecord,
    /// 融合后分数，单位 basis point。
    pub score_basis_points: u16,
    /// 文本 BM25 分数。
    pub text_score: Option<f32>,
    /// 向量距离，越小越相似。
    pub vector_distance: Option<f32>,
    /// 文本 rank，从 1 开始。
    pub text_rank: Option<usize>,
    /// 向量 rank，从 1 开始。
    pub vector_rank: Option<usize>,
    /// 召回解释。
    pub explain: MemoryHybridExplain,
}

/// Memory 混合召回解释。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHybridExplain {
    /// 文本分量分数。
    pub text_component_basis_points: u16,
    /// 向量分量分数。
    pub vector_component_basis_points: u16,
    /// 治理分量分数。
    pub governance_component_basis_points: u16,
    /// 融合权重摘要。
    pub weights: &'static str,
}

#[derive(Debug, Clone)]
struct Candidate {
    record: MemorySqliteRecord,
    text_score: Option<f32>,
    vector_distance: Option<f32>,
    text_rank: Option<usize>,
    vector_rank: Option<usize>,
}

struct MergeHitInput {
    record: MemorySqliteRecord,
    text_score: Option<f32>,
    vector_distance: Option<f32>,
    text_rank: Option<usize>,
    vector_rank: Option<usize>,
    now_ms: u64,
    scope: Option<MemoryHybridScope>,
}

impl MemoryOrchestrator {
    /// 执行文本 + 向量 + SQLite 治理元数据的混合检索。
    /// 只走文本与文本向量路的混合检索。
    pub fn hybrid_search(&self, query: &MemoryHybridQuery) -> AxonResult<Vec<MemoryHybridHit>> {
        self.hybrid_search_with_image(query, None)
    }

    /// 混合检索；额外给出图像查询向量时，按第二路向量再召回一次，同一条记录取更优名次合并。
    pub fn hybrid_search_with_image(
        &self,
        query: &MemoryHybridQuery,
        image_query_embedding: Option<&[f32]>,
    ) -> AxonResult<Vec<MemoryHybridHit>> {
        validate_query(query)?;
        if query.limit == 0 {
            return Ok(Vec::new());
        }

        let candidate_limit = query
            .limit
            .saturating_mul(MEMORY_HYBRID_CANDIDATE_MULTIPLIER)
            .max(query.limit);
        let store = self.sqlite_store()?;
        let index_candidate_limit = candidate_limit;
        let mut candidates = BTreeMap::<MemoryId, Candidate>::new();
        if let Ok(text_index) = self.text_index() {
            let text_hits = text_index
                .search(&query.query_text, index_candidate_limit)
                .unwrap_or_default();
            let text_memory_ids = text_hits
                .iter()
                .map(|hit| hit.memory_id)
                .collect::<Vec<_>>();
            let text_records = store.records_by_memory_ids(&text_memory_ids)?;
            let text_records = records_by_memory_id(text_records);

            let mut scoped_rank = 0;
            for (index, hit) in text_hits.iter().enumerate() {
                if let Some(record) = text_records.get(&hit.memory_id) {
                    let accepted = merge_hit(
                        &mut candidates,
                        MergeHitInput {
                            record: record.clone(),
                            text_score: Some(hit.score),
                            vector_distance: None,
                            text_rank: Some(if query.scope.is_some() {
                                scoped_rank + 1
                            } else {
                                index + 1
                            }),
                            vector_rank: None,
                            now_ms: query.now_ms,
                            scope: query.scope.clone(),
                        },
                    );
                    if accepted && query.scope.is_some() {
                        scoped_rank += 1;
                    }
                }
            }
        }

        if let Ok(Some(vector_index)) = self.vector_index() {
            // 文本向量与图像向量各走一路召回，命中同一记录时 merge_hit 取更优名次。
            for embedding in [query.query_embedding.as_deref(), image_query_embedding]
                .into_iter()
                .flatten()
            {
                let vector_hits = vector_index.search(embedding, index_candidate_limit)?;
                let vector_ids = vector_hits
                    .iter()
                    .map(|hit| hit.vector_id)
                    .collect::<Vec<_>>();
                let vector_records = store.records_by_vector_ids(&vector_ids)?;
                let vector_records = records_by_vector_id(vector_records);

                let mut scoped_rank = 0;
                for (index, hit) in vector_hits.iter().enumerate() {
                    if let Some(record) = vector_records.get(&hit.vector_id) {
                        let accepted = merge_hit(
                            &mut candidates,
                            MergeHitInput {
                                record: record.clone(),
                                text_score: None,
                                vector_distance: Some(hit.distance),
                                text_rank: None,
                                vector_rank: Some(if query.scope.is_some() {
                                    scoped_rank + 1
                                } else {
                                    index + 1
                                }),
                                now_ms: query.now_ms,
                                scope: query.scope.clone(),
                            },
                        );
                        if accepted && query.scope.is_some() {
                            scoped_rank += 1;
                        }
                    }
                }
            }
        }

        if let Some(scope) = query.scope.as_ref() {
            let scoped_records = match scope.user_ref.as_deref() {
                Some(user_ref) => store.recallable_records_by_scope(
                    user_ref,
                    &scope.namespace_ref,
                    query.now_ms,
                    index_candidate_limit,
                )?,
                None => store.recallable_records_by_namespace(
                    &scope.namespace_ref,
                    query.now_ms,
                    index_candidate_limit,
                )?,
            };
            for record in scoped_records {
                if candidates.contains_key(&record.record.id) {
                    continue;
                }
                let fallback_score = core_fallback_score_basis_points(&record, &query.query_text);
                let score = if fallback_score > 0 {
                    fallback_score
                } else {
                    governance_score_basis_points(&record)
                };
                merge_hit(
                    &mut candidates,
                    MergeHitInput {
                        record,
                        text_score: Some(f32::from(score)),
                        vector_distance: None,
                        text_rank: None,
                        vector_rank: None,
                        now_ms: query.now_ms,
                        scope: query.scope.clone(),
                    },
                );
            }
        }
        if candidates.is_empty() {
            let records = match query.scope.as_ref() {
                Some(scope) => match scope.user_ref.as_deref() {
                    Some(user_ref) => store.recallable_records_by_scope(
                        user_ref,
                        &scope.namespace_ref,
                        query.now_ms,
                        candidate_limit,
                    )?,
                    None => store.recallable_records_by_namespace(
                        &scope.namespace_ref,
                        query.now_ms,
                        candidate_limit,
                    )?,
                },
                None => store.recallable_records_limited(query.now_ms, candidate_limit)?,
            };
            for record in records {
                let fallback_score = core_fallback_score_basis_points(&record, &query.query_text);
                if fallback_score == 0 {
                    continue;
                }
                merge_hit(
                    &mut candidates,
                    MergeHitInput {
                        record,
                        text_score: Some(f32::from(fallback_score)),
                        vector_distance: None,
                        text_rank: None,
                        vector_rank: None,
                        now_ms: query.now_ms,
                        scope: query.scope.clone(),
                    },
                );
            }
        }

        let mut hits = candidates
            .into_values()
            .map(candidate_to_hit)
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score_basis_points
                .cmp(&left.score_basis_points)
                .then_with(|| left.record.record.id.0.cmp(&right.record.record.id.0))
        });
        hits.truncate(query.limit);
        Ok(hits)
    }
}

fn merge_hit(candidates: &mut BTreeMap<MemoryId, Candidate>, input: MergeHitInput) -> bool {
    let MergeHitInput {
        record,
        text_score,
        vector_distance,
        text_rank,
        vector_rank,
        now_ms,
        scope,
    } = input;
    if !record_is_recallable(&record, now_ms) || !record_matches_scope(&record, scope.as_ref()) {
        return false;
    }
    let memory_id = record.record.id;
    let candidate = candidates.entry(memory_id).or_insert(Candidate {
        record,
        text_score: None,
        vector_distance: None,
        text_rank: None,
        vector_rank: None,
    });
    if let Some(score) = text_score {
        candidate.text_score = Some(score);
    }
    if let Some(distance) = vector_distance {
        candidate.vector_distance = Some(match candidate.vector_distance {
            Some(existing) => existing.min(distance),
            None => distance,
        });
    }
    if let Some(rank) = text_rank {
        candidate.text_rank = Some(rank);
    }
    if let Some(rank) = vector_rank {
        candidate.vector_rank = Some(match candidate.vector_rank {
            Some(existing) => existing.min(rank),
            None => rank,
        });
    }
    true
}

fn record_matches_scope(record: &MemorySqliteRecord, scope: Option<&MemoryHybridScope>) -> bool {
    scope.is_none_or(|scope| {
        record.user_ref.as_deref() == scope.user_ref.as_deref()
            && record.namespace_ref.as_deref() == Some(scope.namespace_ref.as_str())
    })
}

fn candidate_to_hit(candidate: Candidate) -> MemoryHybridHit {
    let explain = explain_candidate(&candidate);
    let score_basis_points = explain
        .text_component_basis_points
        .saturating_add(explain.vector_component_basis_points)
        .saturating_add(explain.governance_component_basis_points)
        .min(basis_points_to_u16(MAX_BASIS_POINTS));
    MemoryHybridHit {
        record: candidate.record,
        score_basis_points,
        text_score: candidate.text_score,
        vector_distance: candidate.vector_distance,
        text_rank: candidate.text_rank,
        vector_rank: candidate.vector_rank,
        explain,
    }
}

fn explain_candidate(candidate: &Candidate) -> MemoryHybridExplain {
    if candidate.text_rank.is_none()
        && candidate.vector_rank.is_none()
        && let Some(score) = candidate.text_score
    {
        // 未进入任何索引 rank 的候选只携带兜底分：要么来自治理元数据，
        // 要么来自文本索引缺失时的关键词退化匹配。这类分数按治理权重折算，
        // 不得全额计入 text 分量，否则治理分高的记录会压过 rank=1 的强相关命中。
        return MemoryHybridExplain {
            text_component_basis_points: 0,
            vector_component_basis_points: 0,
            governance_component_basis_points: basis_points_to_u16(weighted_component(
                u32::from(f32_to_basis_points(score)),
                MEMORY_HYBRID_GOVERNANCE_WEIGHT_BASIS_POINTS,
            )),
            weights: "core_fallback",
        };
    }
    let text = candidate.text_rank.map(rank_score).unwrap_or_default();
    let vector = candidate.vector_rank.map(rank_score).unwrap_or_default();
    let governance = u32::midpoint(
        u32::from(candidate.record.confidence_basis_points),
        u32::from(candidate.record.importance_basis_points),
    );
    MemoryHybridExplain {
        text_component_basis_points: basis_points_to_u16(weighted_component(
            text,
            MEMORY_HYBRID_TEXT_WEIGHT_BASIS_POINTS,
        )),
        vector_component_basis_points: basis_points_to_u16(weighted_component(
            vector,
            MEMORY_HYBRID_VECTOR_WEIGHT_BASIS_POINTS,
        )),
        governance_component_basis_points: basis_points_to_u16(weighted_component(
            governance,
            MEMORY_HYBRID_GOVERNANCE_WEIGHT_BASIS_POINTS,
        )),
        weights: "text=4500,vector=4500,governance=1000",
    }
}

fn rank_score(rank: usize) -> u32 {
    if rank == 0 {
        return 0;
    }
    let rank = u32::try_from(rank).unwrap_or(u32::MAX);
    MAX_BASIS_POINTS / rank
}

fn weighted_component(score: u32, weight_basis_points: u32) -> u32 {
    score.saturating_mul(weight_basis_points) / MAX_BASIS_POINTS
}

fn record_is_recallable(record: &MemorySqliteRecord, now_ms: u64) -> bool {
    matches!(record.record.state, MemoryState::Weak | MemoryState::Strong)
        && record.archived_at_ms.is_none()
        && record
            .ttl_expires_at_ms
            .is_none_or(|expires_at_ms| expires_at_ms > now_ms)
        && record
            .record
            .content_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn core_fallback_score_basis_points(record: &MemorySqliteRecord, query_text: &str) -> u16 {
    let terms = query_terms(query_text);
    if terms.is_empty() {
        return governance_score_basis_points(record);
    }
    let matched_terms = terms
        .iter()
        .filter(|term| record_ref_fields(record).any(|field| field.contains(term.as_str())))
        .count();
    if matched_terms == 0 {
        return 0;
    }
    let matched_terms = u32::try_from(matched_terms).unwrap_or(u32::MAX);
    let term_count = u32::try_from(terms.len()).unwrap_or(u32::MAX);
    let match_score = matched_terms
        .saturating_mul(MAX_BASIS_POINTS)
        .checked_div(term_count)
        .unwrap_or_default();
    let governance = u32::from(governance_score_basis_points(record));
    basis_points_to_u16((match_score.saturating_mul(9) + governance) / 10)
}

fn governance_score_basis_points(record: &MemorySqliteRecord) -> u16 {
    basis_points_to_u16(
        u32::midpoint(
            u32::from(record.confidence_basis_points),
            u32::from(record.importance_basis_points),
        )
        .min(MAX_BASIS_POINTS),
    )
}

fn basis_points_to_u16(score: u32) -> u16 {
    u16::try_from(score.min(MAX_BASIS_POINTS)).unwrap_or(u16::MAX)
}

fn f32_to_basis_points(score: f32) -> u16 {
    if !score.is_finite() {
        return 0;
    }
    score.clamp(0.0, f32::from(basis_points_to_u16(MAX_BASIS_POINTS))) as u16
}

fn query_terms(query_text: &str) -> Vec<String> {
    query_text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|term| {
            let term = term.trim();
            (term.chars().count() >= 2).then(|| term.to_lowercase())
        })
        .collect()
}

fn record_ref_fields(record: &MemorySqliteRecord) -> impl Iterator<Item = String> + '_ {
    [
        record.record.content_ref.as_deref(),
        record
            .record
            .source_trace
            .as_ref()
            .and_then(|trace| trace.evidence_ref.as_deref()),
        record
            .record
            .source_trace
            .as_ref()
            .map(|trace| trace.summary.as_str()),
        record
            .record
            .tombstone
            .as_ref()
            .map(|tombstone| tombstone.blocked_ref.as_str()),
        record
            .record
            .tombstone
            .as_ref()
            .map(|tombstone| tombstone.reason.as_str()),
    ]
    .into_iter()
    .flatten()
    .map(str::to_lowercase)
}

fn records_by_memory_id(
    records: Vec<MemorySqliteRecord>,
) -> BTreeMap<MemoryId, MemorySqliteRecord> {
    records
        .into_iter()
        .map(|record| (record.record.id, record))
        .collect()
}

fn records_by_vector_id(records: Vec<MemorySqliteRecord>) -> BTreeMap<i64, MemorySqliteRecord> {
    records
        .into_iter()
        .filter_map(|record| record.vector_id.map(|vector_id| (vector_id, record)))
        .collect()
}

fn validate_query(query: &MemoryHybridQuery) -> AxonResult<()> {
    if query.query_text.trim().is_empty() && query.query_embedding.is_none() {
        return Err(AxonError::invalid_input(
            "Memory hybrid 查询必须提供文本或 embedding",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "retrieval_tests.rs"]
mod tests;
