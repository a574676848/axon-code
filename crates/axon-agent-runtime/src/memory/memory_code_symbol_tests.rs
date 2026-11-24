use super::*;

#[test]
fn code_symbol_indexer_extracts_dependencies_and_references() {
    let source =
        "import { run } from './runner';\nexport function build() { return VerifyPlan.run(); }\n";
    let records = extract_symbols_for_path("src/build.test.ts", source, 42);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].symbol_name, "build");
    assert_eq!(records[0].dependency_refs, vec!["dep:runner"]);
    assert!(
        records[0]
            .reference_refs
            .contains(&"ref:VerifyPlan".to_owned())
    );
    assert!(
        records[0]
            .reference_refs
            .contains(&"test-cover:src:build.test.ts".to_owned())
    );
}

#[test]
fn incremental_index_rebuilds_more_than_legacy_symbol_cap() {
    const SYMBOL_COUNT: usize = 2_100;
    let temp = tempfile::TempDir::new().unwrap();
    let source = (0..SYMBOL_COUNT)
        .map(|index| format!("export function symbol_{index}() {{}}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(temp.path().join("large.ts"), source).unwrap();

    let report = MemoryCodeSymbolRuntime
        .apply_incremental_index(MemoryCodeSymbolIncrementalIndexInput {
            existing_records: Vec::new(),
            changed_path_refs: vec!["large.ts".to_owned()],
            deleted_path_refs: Vec::new(),
            project_root: temp.path().to_path_buf(),
            now_ms: 42,
        })
        .unwrap();

    assert_eq!(report.rebuilt_symbol_count, SYMBOL_COUNT);
}
