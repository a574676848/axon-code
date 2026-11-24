use super::*;
use crate::WorkspaceFileSnapshotLookup;

fn temp_workspace(tag: &str) -> PathBuf {
    let dir =
        axon_test_support::test_temp_root().join(format!("axon-fs-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("readme.txt"), "axon read only line\n").unwrap();
    fs::write(dir.join("sub").join("nested.txt"), "nested\n").unwrap();
    dir
}

fn read_input(root: &Path, path: &str) -> ReadFileToolExecutionInput {
    ReadFileToolExecutionInput {
        root: root.to_path_buf(),
        path: PathBuf::from(path),
        snapshot_session: Some(Arc::new(WorkspaceFileSession::default())),
        offset: None,
        limit: None,
        remote_requested: false,
        filter_level: None,
    }
}

#[test]
fn read_file_returns_bounded_content_within_root() {
    let workspace = temp_workspace("read");
    let receipt = ReadOnlyFsToolRuntime
        .read_file(read_input(&workspace, "readme.txt"))
        .unwrap();

    assert_eq!(receipt.status, FsToolExecutionStatus::Completed);
    assert_eq!(receipt.preview, "1:axon read only line");
    assert_eq!(
        (receipt.line_start, receipt.line_end, receipt.total_lines),
        (1, 1, 1)
    );
    assert_eq!(receipt.next_offset, None);
    assert!(!receipt.filesystem_write_performed);
    assert_eq!(receipt.path, "readme.txt");
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_preserves_long_single_line_without_column_truncation() {
    let workspace = temp_workspace("read-long-line");
    let long_line = "a".repeat(60 * 1_024);
    fs::write(workspace.join("long.txt"), &long_line).unwrap();

    let receipt = ReadOnlyFsToolRuntime
        .read_file(read_input(&workspace, "long.txt"))
        .unwrap();

    assert_eq!(receipt.preview, format!("1:{long_line}"));
    assert_eq!(receipt.seen_lines, [1]);
    assert_eq!(receipt.next_offset, None);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn read_file_pages_with_stable_line_numbers_and_next_offset() {
    let workspace = temp_workspace("read-page");
    fs::write(workspace.join("page.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    let mut input = read_input(&workspace, "page.txt");
    input.offset = Some(2);
    input.limit = Some(2);

    let receipt = ReadOnlyFsToolRuntime.read_file(input).unwrap();

    assert_eq!(receipt.preview, "2:two\n3:three");
    assert_eq!(receipt.total_lines, 4);
    assert_eq!(receipt.next_offset, Some(4));
    assert_eq!(receipt.truncation_reason.as_deref(), Some("line_limit"));
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn paged_reads_merge_seen_lines_into_one_session_snapshot() {
    let workspace = temp_workspace("read-session");
    let target = workspace.join("page.txt");
    fs::write(&target, "one\ntwo\nthree\nfour\n").unwrap();
    let session = Arc::new(WorkspaceFileSession::default());
    let mut first = read_input(&workspace, "page.txt");
    first.snapshot_session = Some(Arc::clone(&session));
    first.limit = Some(2);
    let first_receipt = ReadOnlyFsToolRuntime.read_file(first).unwrap();
    let mut second = read_input(&workspace, "page.txt");
    second.snapshot_session = Some(Arc::clone(&session));
    second.offset = first_receipt.next_offset;
    second.limit = Some(2);
    let second_receipt = ReadOnlyFsToolRuntime.read_file(second).unwrap();

    let lookup = session.lookup(
        &target.canonicalize().unwrap(),
        second_receipt.snapshot_tag.as_ref().unwrap(),
    );
    assert!(matches!(
        lookup,
        WorkspaceFileSnapshotLookup::Found(snapshot)
            if snapshot.seen_lines() == &std::collections::BTreeSet::from([1, 2, 3, 4])
    ));
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_blocks_remote_trigger() {
    let workspace = temp_workspace("read-remote");
    let mut input = read_input(&workspace, "readme.txt");
    input.remote_requested = true;
    let receipt = ReadOnlyFsToolRuntime.read_file(input).unwrap();

    assert_eq!(receipt.status, FsToolExecutionStatus::Blocked);
    assert!(receipt.preview.is_empty());
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_accepts_path_outside_root_when_requested() {
    let workspace = temp_workspace("escape");
    let receipt = ReadOnlyFsToolRuntime
        .read_file(read_input(&workspace.join("sub"), "../readme.txt"))
        .expect("跨工作区路径由审批策略决定，工具层不应拒绝");
    assert_eq!(receipt.status, FsToolExecutionStatus::Completed);
    assert!(receipt.preview.contains("axon read only line"));
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_accepts_absolute_path_with_chinese_and_spaces() {
    let workspace = temp_workspace("中文 空格");
    let target = workspace.join("中文 文件.txt");
    fs::write(&target, "中文内容\n").unwrap();

    let receipt = ReadOnlyFsToolRuntime
        .read_file(read_input(&workspace, &target.to_string_lossy()))
        .expect("中文、空格与绝对路径组合不应被工具层拒绝");

    assert_eq!(receipt.status, FsToolExecutionStatus::Completed);
    assert!(receipt.preview.contains("中文内容"));
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_minimal_filter_removes_comments_before_truncation() {
    let workspace = temp_workspace("read-filter");
    fs::write(
        workspace.join("main.rs"),
        "// module note\nfn main() {\n    /* noisy */\n    run();\n}\n",
    )
    .unwrap();
    let mut input = read_input(&workspace, "main.rs");
    input.filter_level = Some(LspFilterLevel::Minimal);
    let receipt = ReadOnlyFsToolRuntime.read_file(input).unwrap();

    assert_eq!(receipt.filter_level_applied, Some(LspFilterLevel::Minimal));
    assert!(receipt.preview.contains("fn main()"));
    assert!(!receipt.preview.contains("module note"));
    assert!(receipt.filtered_chars < receipt.original_chars);
    assert!(!receipt.lsp_used);
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_aggressive_filter_uses_regex_fallback_without_lsp() {
    let workspace = temp_workspace("read-aggressive-filter");
    fs::write(
        workspace.join("main.rs"),
        "use std::path::Path;\n\npub fn run(path: &Path) -> usize {\n    let value = path.display().to_string();\n    value.len()\n}\n",
    )
    .unwrap();
    let mut input = read_input(&workspace, "main.rs");
    input.filter_level = Some(LspFilterLevel::Aggressive);
    let receipt = ReadOnlyFsToolRuntime.read_file(input).unwrap();

    assert_eq!(
        receipt.filter_level_applied,
        Some(LspFilterLevel::Aggressive)
    );
    assert!(receipt.preview.contains("use std::path::Path;"));
    assert!(
        receipt
            .preview
            .contains("pub fn run(path: &Path) -> usize {")
    );
    assert!(receipt.preview.contains("// ..."));
    assert!(!receipt.preview.contains("path.display().to_string()"));
    assert!(!receipt.lsp_used);
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_downgrades_aggressive_filter_for_large_files() {
    let workspace = temp_workspace("read-large-aggressive");
    let content = format!(
        "pub fn main() {{\n    let value = {};\n}}\n",
        "1".repeat(adaptive_filter::DEFAULT_LARGE_FILE_THRESHOLD_BYTES as usize)
    );
    fs::write(workspace.join("main.rs"), content).unwrap();

    let mut input = read_input(&workspace, "main.rs");
    input.filter_level = Some(LspFilterLevel::Aggressive);
    let receipt = ReadOnlyFsToolRuntime.read_file(input).unwrap();

    assert_eq!(receipt.filter_level_applied, Some(LspFilterLevel::Minimal));
    assert!(!receipt.lsp_used);
    assert!(receipt.preview.contains("pub fn main()"));
    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn read_file_notebook_uses_derived_text_without_editable_snapshot() {
    let workspace = temp_workspace("read-notebook");
    fs::write(
        workspace.join("sample.ipynb"),
        r##"{"cells":[{"cell_type":"markdown","source":["# Axon\n"]}]}"##,
    )
    .unwrap();
    let receipt = ReadOnlyFsToolRuntime
        .read_file(read_input(&workspace, "sample.ipynb"))
        .unwrap();

    assert_eq!(receipt.content_kind, ReadFileContentKind::Notebook);
    assert!(receipt.preview.contains("Cell 1 [markdown]"));
    assert!(receipt.preview.contains("# Axon"));
    assert!(receipt.snapshot_tag.is_none());
    assert!(receipt.seen_lines.is_empty());
    let _ = fs::remove_dir_all(&workspace);
}
