use axon_memory::{
    MemoryCitation, MemoryCitationParseReport, MemoryCitationParser, MemoryHybridHit,
    MemoryUsageEvent,
};

use crate::usage::cited_usage_events;

/// 单轮模型调用中的 Memory hit 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTurnHit {
    /// Memory ID。
    pub memory_id: u64,
    /// 模型可见的内容引用。
    pub content_ref: String,
}

/// 单轮模型调用的 Memory 读闭环上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTurnContext {
    /// 调用入口。
    pub surface: String,
    /// 被 recall 命中的 Memory。
    pub recalled_hits: Vec<MemoryTurnHit>,
    /// 已实际注入模型上下文的 Memory。
    pub injected_hits: Vec<MemoryTurnHit>,
}

impl MemoryTurnContext {
    /// 从已注入的 hybrid hits 构造单轮上下文。
    #[must_use]
    pub fn from_injected_hits(surface: impl Into<String>, hits: &[MemoryHybridHit]) -> Self {
        let turn_hits = memory_turn_hits(hits);
        Self {
            surface: surface.into(),
            recalled_hits: turn_hits.clone(),
            injected_hits: turn_hits,
        }
    }

    /// 允许模型 citation 的 Memory ID。
    #[must_use]
    pub fn allowed_citation_ids(&self) -> Vec<u64> {
        self.injected_hits.iter().map(|hit| hit.memory_id).collect()
    }

    /// 当前 turn 的 Memory ID 和内容引用映射。
    #[must_use]
    pub fn hit_ref_pairs(&self) -> Vec<(u64, String)> {
        self.injected_hits
            .iter()
            .map(|hit| (hit.memory_id, hit.content_ref.clone()))
            .collect()
    }
}

/// 模型输出 citation 的采纳报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryTurnCitationReport {
    /// 通过严格 citation 采纳的 Memory 内容引用。
    pub cited_hit_refs: Vec<String>,
    /// 通过 citation 与输出/receipt 扫描合并后的采纳引用。
    pub adopted_hit_refs: Vec<String>,
    /// 通过严格 citation 采纳的 Memory ID。
    pub cited_memory_ids: Vec<u64>,
    /// citation 解析问题数量。
    pub issue_count: usize,
}

/// 解析模型输出中的 Memory citation。
#[must_use]
pub fn parse_memory_citations(text: &str) -> MemoryCitationParseReport {
    MemoryCitationParser::parse(text)
}

/// 解析本轮模型输出中的有效 Memory citation。
#[must_use]
pub fn memory_turn_citation_report(
    context: &MemoryTurnContext,
    output_text: &str,
    scanned_text: &str,
) -> MemoryTurnCitationReport {
    let mut report = parse_memory_citations(output_text);
    retain_allowed_citations(&mut report.citations, &context.allowed_citation_ids());
    let mut cited_memory_ids = report
        .citations
        .iter()
        .map(|citation| citation.memory_id.0)
        .collect::<Vec<_>>();
    cited_memory_ids.sort_unstable();
    cited_memory_ids.dedup();

    let pairs = context.hit_ref_pairs();
    let mut cited_hit_refs = report
        .citations
        .iter()
        .filter_map(|citation| {
            pairs
                .iter()
                .find(|(memory_id, _)| *memory_id == citation.memory_id.0)
                .map(|(_, hit_ref)| hit_ref.clone())
        })
        .collect::<Vec<_>>();
    cited_hit_refs.sort();
    cited_hit_refs.dedup();

    let mut adopted_hit_refs = cited_hit_refs.clone();
    adopted_hit_refs.extend(
        pairs
            .iter()
            .filter(|(_, hit_ref)| scanned_text.contains(hit_ref.as_str()))
            .map(|(_, hit_ref)| hit_ref.clone()),
    );
    adopted_hit_refs.sort();
    adopted_hit_refs.dedup();

    MemoryTurnCitationReport {
        cited_hit_refs,
        adopted_hit_refs,
        cited_memory_ids,
        issue_count: report.issues.len(),
    }
}

/// 生成本轮模型输出中有效 citation 的 usage 事件。
#[must_use]
pub fn memory_turn_cited_usage_events(
    context: &MemoryTurnContext,
    output_text: &str,
    now_ms: u64,
) -> Vec<MemoryUsageEvent> {
    let mut report = parse_memory_citations(output_text);
    retain_allowed_citations(&mut report.citations, &context.allowed_citation_ids());
    cited_usage_events(&report.citations, now_ms)
}

fn memory_turn_hits(hits: &[MemoryHybridHit]) -> Vec<MemoryTurnHit> {
    hits.iter()
        .filter_map(|hit| {
            let content_ref = hit.record.record.content_ref.as_deref()?.trim();
            (!content_ref.is_empty()).then(|| MemoryTurnHit {
                memory_id: hit.record.record.id.0,
                content_ref: content_ref.to_owned(),
            })
        })
        .collect()
}

fn retain_allowed_citations(citations: &mut Vec<MemoryCitation>, ids: &[u64]) {
    let allowed_ids = ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    citations.retain(|citation| allowed_ids.contains(&citation.memory_id.0));
}
