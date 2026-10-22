//! 项目级代码符号记忆索引。

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use axon_domain::{AxonError, AxonResult};

#[path = "memory_code_symbol_extractor.rs"]
mod memory_code_symbol_extractor;

use self::memory_code_symbol_extractor::extract_symbols_for_path;
use crate::memory_code_symbol_support::validate_project_root;

pub use crate::memory_code_symbol_types::{
    MEMORY_CODE_SYMBOL_HASH_MAX_CHARS, MEMORY_CODE_SYMBOL_INDEXER_MAX_SOURCE_BYTES,
    MemoryCodeSymbolHit, MemoryCodeSymbolIncrementalIndexInput,
    MemoryCodeSymbolIncrementalIndexReport, MemoryCodeSymbolInvalidationInput,
    MemoryCodeSymbolInvalidationPlan, MemoryCodeSymbolQuery, MemoryCodeSymbolRecallReport,
    MemoryCodeSymbolRecord,
};

/// 代码符号记忆 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryCodeSymbolRuntime;

impl MemoryCodeSymbolRuntime {
    /// 在离线符号索引中执行 bounded ref-only 召回。
    pub fn recall(
        &self,
        records: &[MemoryCodeSymbolRecord],
        query: &MemoryCodeSymbolQuery,
    ) -> AxonResult<MemoryCodeSymbolRecallReport> {
        validate_query(query)?;
        for record in records {
            validate_record(record)?;
        }

        let query_terms = query_terms(&query.query_text);
        let mut hits = records
            .iter()
            .filter_map(|record| symbol_hit(record, &query_terms))
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score_basis_points
                .cmp(&left.score_basis_points)
                .then_with(|| left.symbol_ref.cmp(&right.symbol_ref))
        });
        let truncated = hits.len() > query.limit;
        hits.truncate(query.limit);

        Ok(MemoryCodeSymbolRecallReport {
            scanned_records: records.len(),
            hit_count: hits.len(),
            truncated,
            ref_only: true,
            remote_source_read_allowed: false,
            hits,
        })
    }

    /// 根据路径变更生成 ref-only 增量失效计划。
    pub fn plan_incremental_invalidation(
        &self,
        records: &[MemoryCodeSymbolRecord],
        input: &MemoryCodeSymbolInvalidationInput,
    ) -> AxonResult<MemoryCodeSymbolInvalidationPlan> {
        validate_invalidation_input(input)?;
        for record in records {
            validate_record(record)?;
        }

        let changed_paths = normalized_path_set(&input.changed_path_refs);
        let deleted_paths = normalized_path_set(&input.deleted_path_refs);
        let mut direct_stale = BTreeSet::new();
        for record in records.iter().filter(|record| {
            changed_paths.contains(record.path_ref.trim())
                || deleted_paths.contains(record.path_ref.trim())
        }) {
            direct_stale.insert(record.symbol_ref.clone());
        }

        let mut dependent_stale = BTreeSet::new();
        for record in records {
            if direct_stale.contains(&record.symbol_ref) {
                continue;
            }
            if record
                .dependency_refs
                .iter()
                .any(|dependency_ref| direct_stale.contains(dependency_ref))
            {
                dependent_stale.insert(record.symbol_ref.clone());
            }
        }

        let mut stale_symbol_refs = direct_stale.clone();
        stale_symbol_refs.extend(dependent_stale.iter().cloned());

        Ok(MemoryCodeSymbolInvalidationPlan {
            scanned_records: records.len(),
            reindex_path_refs: changed_paths.into_iter().collect(),
            deleted_path_refs: deleted_paths.into_iter().collect(),
            direct_stale_symbol_refs: direct_stale.into_iter().collect(),
            dependent_stale_symbol_refs: dependent_stale.into_iter().collect(),
            stale_symbol_refs: stale_symbol_refs.into_iter().collect(),
            ref_only: true,
            remote_source_read_allowed: false,
            planned_at_ms: input.now_ms,
        })
    }

    /// 执行本机增量索引：删除失效投影，并重建变更路径的 ref-only 符号记录。
    pub fn apply_incremental_index(
        &self,
        input: MemoryCodeSymbolIncrementalIndexInput,
    ) -> AxonResult<MemoryCodeSymbolIncrementalIndexReport> {
        validate_project_root(&input.project_root)?;
        let plan = self.plan_incremental_invalidation(
            &input.existing_records,
            &MemoryCodeSymbolInvalidationInput {
                changed_path_refs: input.changed_path_refs.clone(),
                deleted_path_refs: input.deleted_path_refs.clone(),
                now_ms: input.now_ms,
            },
        )?;
        let stale_symbols = normalized_ref_set(&plan.stale_symbol_refs);
        let deleted_paths = normalized_path_set(&plan.deleted_path_refs);
        let changed_paths = normalized_path_set(&plan.reindex_path_refs);
        let mut records = input
            .existing_records
            .into_iter()
            .filter(|record| {
                !stale_symbols.contains(record.symbol_ref.trim())
                    && !deleted_paths.contains(record.path_ref.trim())
                    && !changed_paths.contains(record.path_ref.trim())
            })
            .collect::<Vec<_>>();
        let removed_symbol_count = plan.stale_symbol_refs.len();
        let mut rebuilt_symbol_count = 0usize;
        for path_ref in &plan.reindex_path_refs {
            let rebuilt = rebuilt_records_for_path(&input.project_root, path_ref, input.now_ms)?;
            rebuilt_symbol_count = rebuilt_symbol_count.saturating_add(rebuilt.len());
            records.extend(rebuilt);
        }
        records.sort_by(|left, right| left.symbol_ref.cmp(&right.symbol_ref));
        records.dedup_by(|left, right| left.symbol_ref == right.symbol_ref);
        for record in &records {
            validate_record(record)?;
        }
        Ok(MemoryCodeSymbolIncrementalIndexReport {
            plan,
            records,
            rebuilt_path_count: changed_paths.len(),
            rebuilt_symbol_count,
            removed_symbol_count,
            ref_only: true,
            remote_source_read_allowed: false,
        })
    }
}

fn symbol_hit(
    record: &MemoryCodeSymbolRecord,
    query_terms: &[String],
) -> Option<MemoryCodeSymbolHit> {
    let score_basis_points = symbol_score_basis_points(record, query_terms);
    (score_basis_points > 0).then(|| MemoryCodeSymbolHit {
        symbol_ref: record.symbol_ref.clone(),
        symbol_name: record.symbol_name.clone(),
        kind: record.kind,
        path_ref: record.path_ref.clone(),
        source_summary_hash: record.source_summary_hash.clone(),
        reference_refs: record.reference_refs.clone(),
        score_basis_points,
        ref_only: true,
    })
}

fn symbol_score_basis_points(record: &MemoryCodeSymbolRecord, query_terms: &[String]) -> u16 {
    let symbol_name = record.symbol_name.to_ascii_lowercase();
    let symbol_ref = record.symbol_ref.to_ascii_lowercase();
    let path_ref = record.path_ref.to_ascii_lowercase();
    let dependencies = record
        .dependency_refs
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let references = record
        .reference_refs
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();

    let mut score = 0u16;
    for term in query_terms {
        if symbol_name.contains(term) {
            score = score.saturating_add(4_000);
        }
        if symbol_ref.contains(term) {
            score = score.saturating_add(2_500);
        }
        if path_ref.contains(term) {
            score = score.saturating_add(1_500);
        }
        if dependencies.iter().any(|value| value.contains(term)) {
            score = score.saturating_add(1_000);
        }
        if references.iter().any(|value| value.contains(term)) {
            score = score.saturating_add(1_000);
        }
    }
    score.min(10_000)
}

fn query_terms(query_text: &str) -> Vec<String> {
    query_text
        .split(|value: char| !value.is_alphanumeric() && value != '_' && value != '-')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn validate_query(query: &MemoryCodeSymbolQuery) -> AxonResult<()> {
    if query.query_text.trim().is_empty() {
        return Err(AxonError::runtime_failure("代码符号记忆查询不能为空"));
    }
    if query.limit == 0 {
        return Err(AxonError::runtime_failure("代码符号记忆 limit 无效"));
    }
    Ok(())
}

fn validate_invalidation_input(input: &MemoryCodeSymbolInvalidationInput) -> AxonResult<()> {
    if input.now_ms == 0 {
        return Err(AxonError::runtime_failure("代码符号失效计划输入无效"));
    }
    let path_count = input
        .changed_path_refs
        .len()
        .saturating_add(input.deleted_path_refs.len());
    if path_count == 0 {
        return Err(AxonError::runtime_failure("代码符号失效路径不能为空"));
    }
    for path_ref in input
        .changed_path_refs
        .iter()
        .chain(input.deleted_path_refs.iter())
    {
        validate_non_empty(path_ref, "代码符号失效路径引用无效")?;
    }
    Ok(())
}

fn normalized_path_set(path_refs: &[String]) -> BTreeSet<String> {
    path_refs
        .iter()
        .map(|path_ref| path_ref.trim())
        .filter(|path_ref| !path_ref.is_empty())
        .map(str::to_owned)
        .collect()
}

fn normalized_ref_set(refs: &[String]) -> BTreeSet<String> {
    refs.iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn rebuilt_records_for_path(
    project_root: &Path,
    path_ref: &str,
    now_ms: u64,
) -> AxonResult<Vec<MemoryCodeSymbolRecord>> {
    let path = resolve_project_path(project_root, path_ref)?;
    let metadata =
        fs::metadata(&path).map_err(|_| AxonError::runtime_failure("代码符号路径不可读"))?;
    let source_len = usize::try_from(metadata.len())
        .map_err(|_| AxonError::runtime_failure("代码符号源码文件超出本机索引上限"))?;
    if !metadata.is_file() || source_len > MEMORY_CODE_SYMBOL_INDEXER_MAX_SOURCE_BYTES {
        return Err(AxonError::runtime_failure(
            "代码符号源码文件超出本机索引上限",
        ));
    }
    let source = fs::read_to_string(&path)
        .map_err(|_| AxonError::runtime_failure("代码符号源码文件不是 UTF-8"))?;
    Ok(extract_symbols_for_path(path_ref, &source, now_ms))
}

fn resolve_project_path(project_root: &Path, path_ref: &str) -> AxonResult<PathBuf> {
    let trimmed = path_ref.trim();
    validate_non_empty(trimmed, "代码符号路径引用无效")?;
    let relative = Path::new(trimmed);
    if relative.is_absolute()
        || trimmed.contains("..")
        || trimmed.contains('\\')
        || trimmed.starts_with('/')
    {
        return Err(AxonError::runtime_failure(
            "代码符号路径引用必须是项目内相对路径",
        ));
    }
    let root = project_root
        .canonicalize()
        .map_err(|_| AxonError::runtime_failure("代码符号项目根目录不可读"))?;
    let path = root.join(relative);
    let canonical = path
        .canonicalize()
        .map_err(|_| AxonError::runtime_failure("代码符号路径不可读"))?;
    if !canonical.starts_with(&root) {
        return Err(AxonError::runtime_failure("代码符号路径越过项目根目录"));
    }
    Ok(canonical)
}

fn validate_record(record: &MemoryCodeSymbolRecord) -> AxonResult<()> {
    validate_non_empty(&record.symbol_ref, "代码符号 ref 无效")?;
    validate_non_empty(&record.symbol_name, "代码符号名称无效")?;
    validate_non_empty(&record.path_ref, "代码符号路径引用无效")?;
    validate_bounded(
        &record.source_summary_hash,
        MEMORY_CODE_SYMBOL_HASH_MAX_CHARS,
        "代码符号摘要 hash 无效",
    )?;
    if record.updated_at_ms == 0 {
        return Err(AxonError::runtime_failure("代码符号记录治理元数据无效"));
    }
    for dependency_ref in &record.dependency_refs {
        validate_non_empty(dependency_ref, "代码符号依赖 ref 无效")?;
    }
    for reference_ref in &record.reference_refs {
        validate_non_empty(reference_ref, "代码符号引用 ref 无效")?;
    }
    Ok(())
}

fn validate_non_empty(value: &str, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

fn validate_bounded(value: &str, max_chars: usize, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

#[cfg(test)]
#[path = "memory_code_symbol_tests.rs"]
mod tests;
