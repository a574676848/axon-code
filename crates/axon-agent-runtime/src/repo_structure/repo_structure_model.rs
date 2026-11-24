//! Repo Structure Model：统一仓库骨架、符号来源与最小关系边。

use std::{collections::BTreeMap, path::PathBuf};

use axon_domain::{AxonError, AxonResult};

use crate::repo_structure_model_scan::{
    current_time_ms, file_stem_name, scan_workspace, source_tag, uri_to_path_ref,
    validate_workspace_root,
};
use crate::repo_structure_model_summary::{
    bounded_summary, primary_symbol_source, summary_source_refs,
};
pub use crate::repo_structure_model_types::{
    RepoEdge, RepoEdgeKind, RepoFile, RepoFileRole, RepoIndexSnapshot, RepoModule, RepoModuleKind,
    RepoSourceTag, RepoStructureQuery, RepoStructureQueryReport, RepoStructureRefreshInput,
    RepoStructureRefreshPlan, RepoStructureSourceKind, RepoSymbol, RepoSymbolKind, RepoWorkspace,
};
use crate::{
    LspWorkspaceSymbolReport, MemoryCodeSymbolInvalidationInput, MemoryCodeSymbolKind,
    MemoryCodeSymbolQuery, MemoryCodeSymbolRecord, MemoryCodeSymbolRuntime,
};

#[derive(Debug, Default, Clone, Copy)]
/// 仓库结构查询与刷新规划 runtime。
pub struct RepoStructureModelRuntime;

impl RepoStructureModelRuntime {
    /// 构建可跨 turn 复用的仓库基础快照。
    pub fn build_snapshot(
        &self,
        workspace_root: &str,
        max_files: usize,
    ) -> AxonResult<RepoIndexSnapshot> {
        validate_snapshot_input(workspace_root, max_files)?;
        self.build_snapshot_with_lsp_report(workspace_root, max_files, None)
    }

    /// 查询本地仓库结构；LSP 结果必须由显式调用方注入。
    pub fn query(&self, input: &RepoStructureQuery) -> AxonResult<RepoStructureQueryReport> {
        validate_query(input)?;
        let snapshot = self.build_snapshot(&input.workspace_root, input.max_files)?;
        self.query_snapshot(
            &snapshot,
            &input.query_text,
            input.symbol_limit,
            &input.memory_records,
        )
    }

    /// 使用已获得的 LSP workspace symbol 报告查询仓库结构。
    pub fn query_with_lsp_report(
        &self,
        input: &RepoStructureQuery,
        lsp_report: Option<&LspWorkspaceSymbolReport>,
    ) -> AxonResult<RepoStructureQueryReport> {
        validate_query(input)?;
        let snapshot = self.build_snapshot_with_lsp_report(
            &input.workspace_root,
            input.max_files,
            lsp_report,
        )?;
        self.query_snapshot(
            &snapshot,
            &input.query_text,
            input.symbol_limit,
            &input.memory_records,
        )
    }

    /// 基于预加载快照执行 prompt 相关查询，不再访问文件系统。
    pub fn query_snapshot(
        &self,
        base_snapshot: &RepoIndexSnapshot,
        query_text: &str,
        symbol_limit: usize,
        memory_records: &[MemoryCodeSymbolRecord],
    ) -> AxonResult<RepoStructureQueryReport> {
        validate_snapshot_query(query_text, symbol_limit)?;
        let mut snapshot = base_snapshot.clone();
        merge_symbols(
            &mut snapshot.symbols,
            memory_symbols(memory_records, &snapshot.modules, query_text, symbol_limit)?,
        );
        snapshot
            .symbols
            .sort_by(|left, right| left.symbol_ref.cmp(&right.symbol_ref));
        snapshot.edges = build_edges(&snapshot.files, &snapshot.symbols, memory_records);
        Ok(RepoStructureQueryReport {
            summary: bounded_summary(&snapshot, query_text),
            source_refs: summary_source_refs(&snapshot),
            snapshot,
            refresh_plan: None,
        })
    }

    fn build_snapshot_with_lsp_report(
        &self,
        workspace_root: &str,
        max_files: usize,
        lsp_report: Option<&LspWorkspaceSymbolReport>,
    ) -> AxonResult<RepoIndexSnapshot> {
        validate_snapshot_input(workspace_root, max_files)?;
        let scan = scan_workspace(&PathBuf::from(workspace_root), max_files)?;
        let mut symbols = collect_static_symbols(&scan.files, scan.refreshed_at_ms);
        merge_symbols(
            &mut symbols,
            lsp_symbols(lsp_report, &scan.modules, scan.refreshed_at_ms),
        );
        symbols.sort_by(|left, right| left.symbol_ref.cmp(&right.symbol_ref));
        let edges = build_edges(&scan.files, &symbols, &[]);
        Ok(RepoIndexSnapshot {
            workspace: scan.workspace,
            modules: scan.modules,
            files: scan.files,
            edges,
            refreshed_at_ms: scan.refreshed_at_ms,
            partial: scan.partial,
            symbols,
        })
    }

    /// 基于路径变更生成仓库结构增量刷新计划。
    pub fn plan_refresh(
        &self,
        input: &RepoStructureRefreshInput,
    ) -> AxonResult<RepoStructureRefreshPlan> {
        if input.changed_path_refs.is_empty() && input.deleted_path_refs.is_empty() {
            return Ok(RepoStructureRefreshPlan {
                mode: "full_scan".to_owned(),
                changed_path_refs: Vec::new(),
                deleted_path_refs: Vec::new(),
                invalidation: None,
                partial_stale: false,
            });
        }
        let invalidation = MemoryCodeSymbolRuntime.plan_incremental_invalidation(
            &input.memory_records,
            &MemoryCodeSymbolInvalidationInput {
                changed_path_refs: input.changed_path_refs.clone(),
                deleted_path_refs: input.deleted_path_refs.clone(),
                now_ms: input.now_ms,
            },
        )?;
        Ok(RepoStructureRefreshPlan {
            mode: "incremental".to_owned(),
            changed_path_refs: invalidation.reindex_path_refs.clone(),
            deleted_path_refs: invalidation.deleted_path_refs.clone(),
            partial_stale: !invalidation.stale_symbol_refs.is_empty(),
            invalidation: Some(invalidation),
        })
    }
}

fn collect_static_symbols(files: &[RepoFile], updated_at_ms: u64) -> Vec<RepoSymbol> {
    files
        .iter()
        .filter(|file| file.role == RepoFileRole::EntryPoint)
        .map(|file| RepoSymbol {
            symbol_ref: format!("symbol:{}:entrypoint", file.path_ref),
            symbol_name: file_stem_name(&file.path_ref),
            kind: RepoSymbolKind::EntryPoint,
            path_ref: file.path_ref.clone(),
            module_ref: Some(file.module_ref.clone()),
            updated_at_ms,
            ref_only: true,
            sources: vec![source_tag(
                RepoStructureSourceKind::StaticScan,
                "entrypoint_scan",
            )],
        })
        .collect()
}

fn lsp_symbols(
    report: Option<&LspWorkspaceSymbolReport>,
    modules: &[RepoModule],
    updated_at_ms: u64,
) -> Vec<RepoSymbol> {
    let Some(report) = report else {
        return Vec::new();
    };
    let mut symbols = Vec::new();
    for hit in &report.hits {
        let Some(items) = hit
            .response
            .get("result")
            .and_then(|value| value.as_array())
        else {
            continue;
        };
        for item in items {
            let Some(symbol_name) = item.get("name").and_then(|value| value.as_str()) else {
                continue;
            };
            let path_ref = item
                .get("location")
                .and_then(|value| value.get("uri"))
                .and_then(|value| value.as_str())
                .and_then(uri_to_path_ref)
                .unwrap_or_else(|| "unknown".to_owned());
            symbols.push(RepoSymbol {
                symbol_ref: format!("symbol:{path_ref}:{symbol_name}"),
                symbol_name: symbol_name.to_owned(),
                kind: lsp_symbol_kind(item.get("kind").and_then(serde_json::Value::as_u64)),
                path_ref: path_ref.clone(),
                module_ref: find_module_ref(modules, &path_ref),
                updated_at_ms,
                ref_only: true,
                sources: vec![source_tag(
                    RepoStructureSourceKind::Lsp,
                    &format!("workspace_symbol:{}", hit.server_id),
                )],
            });
        }
    }
    symbols
}

fn memory_symbols(
    records: &[MemoryCodeSymbolRecord],
    modules: &[RepoModule],
    query_text: &str,
    symbol_limit: usize,
) -> AxonResult<Vec<RepoSymbol>> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let recall = MemoryCodeSymbolRuntime.recall(
        records,
        &MemoryCodeSymbolQuery {
            query_text: query_text.to_owned(),
            limit: symbol_limit,
        },
    )?;
    Ok(recall
        .hits
        .into_iter()
        .map(|hit| RepoSymbol {
            symbol_ref: hit.symbol_ref,
            symbol_name: hit.symbol_name,
            kind: memory_symbol_kind(hit.kind),
            path_ref: hit.path_ref.clone(),
            module_ref: find_module_ref(modules, &hit.path_ref),
            updated_at_ms: current_time_ms(),
            ref_only: true,
            sources: vec![source_tag(
                RepoStructureSourceKind::MemoryIndex,
                "memory_code_symbol",
            )],
        })
        .collect())
}

fn merge_symbols(target: &mut Vec<RepoSymbol>, incoming: Vec<RepoSymbol>) {
    let mut index = target
        .iter()
        .enumerate()
        .map(|(position, symbol)| {
            (
                (
                    symbol.path_ref.clone(),
                    symbol.symbol_name.clone(),
                    symbol.kind,
                ),
                position,
            )
        })
        .collect::<BTreeMap<_, _>>();
    for symbol in incoming {
        let key = (
            symbol.path_ref.clone(),
            symbol.symbol_name.clone(),
            symbol.kind,
        );
        if let Some(position) = index.get(&key).copied() {
            let existing = &mut target[position];
            for source in symbol.sources {
                if !existing.sources.contains(&source) {
                    existing.sources.push(source);
                }
            }
            continue;
        }
        index.insert(key, target.len());
        target.push(symbol);
    }
}

fn build_edges(
    files: &[RepoFile],
    symbols: &[RepoSymbol],
    memory_records: &[MemoryCodeSymbolRecord],
) -> Vec<RepoEdge> {
    let mut edges = Vec::new();
    for file in files {
        edges.push(RepoEdge {
            from_ref: file.module_ref.clone(),
            to_ref: file.file_ref.clone(),
            kind: RepoEdgeKind::Contains,
            source: RepoStructureSourceKind::StaticScan,
        });
        if file.role == RepoFileRole::EntryPoint {
            edges.push(RepoEdge {
                from_ref: file.file_ref.clone(),
                to_ref: file.module_ref.clone(),
                kind: RepoEdgeKind::EntrypointOf,
                source: RepoStructureSourceKind::StaticScan,
            });
        }
    }
    for symbol in symbols {
        edges.push(RepoEdge {
            from_ref: format!("file:{}", symbol.path_ref),
            to_ref: symbol.symbol_ref.clone(),
            kind: RepoEdgeKind::Declares,
            source: primary_symbol_source(symbol),
        });
    }
    for record in memory_records {
        for dependency_ref in &record.dependency_refs {
            edges.push(RepoEdge {
                from_ref: record.symbol_ref.clone(),
                to_ref: dependency_ref.clone(),
                kind: RepoEdgeKind::DependsOn,
                source: RepoStructureSourceKind::MemoryIndex,
            });
        }
    }
    edges
}

fn validate_query(input: &RepoStructureQuery) -> AxonResult<()> {
    validate_snapshot_input(&input.workspace_root, input.max_files)?;
    validate_snapshot_query(&input.query_text, input.symbol_limit)
}

fn validate_snapshot_input(workspace_root: &str, max_files: usize) -> AxonResult<()> {
    validate_workspace_root(&PathBuf::from(workspace_root))?;
    if max_files == 0 {
        return Err(AxonError::runtime_failure("仓库结构扫描文件数必须大于 0"));
    }
    Ok(())
}

fn validate_snapshot_query(query_text: &str, symbol_limit: usize) -> AxonResult<()> {
    if query_text.trim().is_empty() {
        return Err(AxonError::runtime_failure("仓库结构查询不能为空"));
    }
    if symbol_limit == 0 {
        return Err(AxonError::runtime_failure("仓库结构符号数量必须大于 0"));
    }
    Ok(())
}

fn lsp_symbol_kind(kind: Option<u64>) -> RepoSymbolKind {
    match kind {
        Some(5 | 6 | 12) => RepoSymbolKind::Function,
        Some(2 | 3 | 10 | 23) => RepoSymbolKind::Module,
        Some(7 | 8 | 9 | 11 | 13 | 22 | 26) => RepoSymbolKind::Type,
        Some(24) => RepoSymbolKind::EntryPoint,
        _ => RepoSymbolKind::Unknown,
    }
}

fn memory_symbol_kind(kind: MemoryCodeSymbolKind) -> RepoSymbolKind {
    match kind {
        MemoryCodeSymbolKind::Function => RepoSymbolKind::Function,
        MemoryCodeSymbolKind::Type => RepoSymbolKind::Type,
        MemoryCodeSymbolKind::Module => RepoSymbolKind::Module,
        MemoryCodeSymbolKind::EntryPoint => RepoSymbolKind::EntryPoint,
    }
}

fn find_module_ref(modules: &[RepoModule], path_ref: &str) -> Option<String> {
    modules
        .iter()
        .filter(|module| module.path_ref != ".")
        .filter(|module| path_ref.starts_with(&module.path_ref))
        .max_by_key(|module| module.path_ref.len())
        .map(|module| module.module_ref.clone())
        .or_else(|| Some("repo:workspace".to_owned()))
}
