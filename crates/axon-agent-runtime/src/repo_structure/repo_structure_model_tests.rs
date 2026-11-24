use serde_json::json;

use crate::{
    LspWorkspaceCoverage, LspWorkspaceSymbolHit, LspWorkspaceSymbolReport, MemoryCodeSymbolKind,
    MemoryCodeSymbolRecord, RepoEdgeKind, RepoStructureModelRuntime, RepoStructureQuery,
    RepoStructureRefreshInput,
};

#[test]
fn preloaded_repo_snapshot_is_queried_without_rescanning_workspace() {
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-repo-prewarm-{}-{}",
        std::process::id(),
        crate::repo_structure_model_scan::current_time_ms()
    ));
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='prewarm'\n").unwrap();
    std::fs::write(workspace.join("src/main.rs"), "fn main() {}\n").unwrap();
    let runtime = RepoStructureModelRuntime;
    let snapshot = runtime
        .build_snapshot(&workspace.to_string_lossy(), 64)
        .unwrap();
    let scanned_files = snapshot.workspace.scanned_files;

    std::fs::write(workspace.join("src/late.rs"), "fn late() {}\n").unwrap();
    let report = runtime.query_snapshot(&snapshot, "late", 8, &[]).unwrap();

    assert_eq!(report.snapshot.workspace.scanned_files, scanned_files);
    assert!(
        report
            .snapshot
            .files
            .iter()
            .all(|file| file.path_ref != "src/late.rs")
    );
    std::fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn repo_structure_query_merges_workspace_lsp_and_memory_sources() {
    let workspace = axon_test_support::test_temp_root().join("axon-repo-structure-query");
    let crate_dir = workspace.join("crates/demo/src");
    std::fs::create_dir_all(&crate_dir).unwrap();
    std::fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers=[\"crates/demo\"]\n",
    )
    .unwrap();
    std::fs::write(
        crate_dir.parent().unwrap().join("Cargo.toml"),
        "[package]\nname='demo'\n",
    )
    .unwrap();
    std::fs::write(crate_dir.join("main.rs"), "fn router() {}\n").unwrap();
    let query = RepoStructureQuery {
        workspace_root: workspace.display().to_string(),
        query_text: "router".to_owned(),
        max_files: 64,
        symbol_limit: 8,
        memory_records: vec![MemoryCodeSymbolRecord {
            symbol_ref: "symbol:crates/demo/src/main.rs:router".to_owned(),
            symbol_name: "router".to_owned(),
            kind: MemoryCodeSymbolKind::Function,
            path_ref: "crates/demo/src/main.rs".to_owned(),
            source_summary_hash: "hash".to_owned(),
            dependency_refs: vec!["symbol:crates/demo/src/main.rs:helper".to_owned()],
            reference_refs: Vec::new(),
            updated_at_ms: 1,
        }],
    };
    let lsp_report = LspWorkspaceSymbolReport {
        coverage: LspWorkspaceCoverage {
            workspace_root: workspace.display().to_string(),
            scanned_files: 1,
            truncated: false,
            matched_servers: vec!["rust-analyzer".to_owned()],
            filtered_servers: Vec::new(),
            file_matches: Vec::new(),
        },
        launchable_servers: vec!["rust-analyzer".to_owned()],
        hits: vec![LspWorkspaceSymbolHit {
            server_id: "rust-analyzer".to_owned(),
            response: json!({
                "result": [
                    {
                        "name": "router",
                        "kind": 12,
                        "location": {
                            "uri": "file:///crates/demo/src/main.rs"
                        }
                    }
                ]
            }),
        }],
    };

    let report = RepoStructureModelRuntime
        .query_with_lsp_report(&query, Some(&lsp_report))
        .unwrap();

    assert!(report.summary.contains("repo_query=router"));
    assert!(report.summary.contains("crates/demo/src/main.rs"));
    assert!(report.snapshot.workspace.module_refs.len() >= 2);
    assert!(
        report
            .snapshot
            .symbols
            .iter()
            .any(|symbol| { symbol.symbol_name == "router" && symbol.sources.len() == 2 })
    );
    assert!(report.snapshot.edges.iter().any(|edge| {
        edge.kind == RepoEdgeKind::DependsOn
            && edge.from_ref == "symbol:crates/demo/src/main.rs:router"
    }));
}

#[test]
fn repo_structure_refresh_plan_uses_memory_incremental_invalidation() {
    let report = RepoStructureModelRuntime
        .plan_refresh(&RepoStructureRefreshInput {
            memory_records: vec![MemoryCodeSymbolRecord {
                symbol_ref: "symbol:src/main.rs:router".to_owned(),
                symbol_name: "router".to_owned(),
                kind: MemoryCodeSymbolKind::Function,
                path_ref: "src/main.rs".to_owned(),
                source_summary_hash: "hash".to_owned(),
                dependency_refs: vec!["symbol:src/main.rs:helper".to_owned()],
                reference_refs: Vec::new(),
                updated_at_ms: 1,
            }],
            changed_path_refs: vec!["src/main.rs".to_owned()],
            deleted_path_refs: Vec::new(),
            now_ms: 2,
        })
        .unwrap();

    assert_eq!(report.mode, "incremental");
    assert!(report.partial_stale);
    assert_eq!(report.changed_path_refs, vec!["src/main.rs".to_owned()]);
    assert!(report.invalidation.as_ref().is_some_and(
        |plan| plan.direct_stale_symbol_refs == vec!["symbol:src/main.rs:router".to_owned()]
    ));
}
