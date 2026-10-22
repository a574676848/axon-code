use super::{AxonCli, run_parsed_cli};

#[test]
fn lsp_definition_plan_only_reports_target_position() {
    let workspace = temp_lsp_workspace("definition");
    let file = workspace.join("src").join("main.rs");
    let cli = AxonCli::parse_from_args([
        "axon",
        "lsp",
        "definition",
        "--workspace-root",
        workspace.to_string_lossy().as_ref(),
        "--file",
        file.to_string_lossy().as_ref(),
        "--line",
        "0",
        "--character",
        "3",
        "--plan-only",
    ]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr.is_empty());
    assert!(result.stdout.contains("Axon LSP definition"));
    assert!(result.stdout.contains("position: 0:3"));

    let _ = std::fs::remove_dir_all(&workspace);
}

fn temp_lsp_workspace(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-lsp-cli-{label}-{unique}-{}",
        std::process::id()
    ));
    let src = workspace.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
    std::fs::write(src.join("main.rs"), "fn demo() {}\n").unwrap();
    workspace
}
