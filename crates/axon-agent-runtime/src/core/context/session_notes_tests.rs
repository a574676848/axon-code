use super::session_notes::*;
use super::session_notes_store::NotesStore;

#[test]
fn session_notes_persists_and_loads_correctly() {
    let temp_dir =
        axon_test_support::test_temp_root().join(format!("axon_notes_test_{}", std::process::id()));
    let notes = SessionNotes::new(GoalAnchor::new("测试目标", vec!["约束A".to_string()]), 1000);

    let save_res = notes.save_to_session(&temp_dir);
    assert!(save_res.is_ok());

    let loaded = SessionNotes::load_from_session(&temp_dir).expect("load success");
    assert!(loaded.is_some());
    let loaded_notes = loaded.unwrap();
    assert_eq!(loaded_notes.goal_anchor.user_objective, "测试目标");
    assert_eq!(loaded_notes.window_number, 1);
    assert_eq!(loaded_notes.last_saved_state_window_number, None);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn session_notes_renders_valid_canonical_checkpoint_from_notes_files() {
    let temp_dir = axon_test_support::test_temp_root()
        .join(format!("axon_notes_checkpoint_test_{}", std::process::id()));
    let notes = SessionNotes::new(
        GoalAnchor::new("执行压缩机制目标态重构", vec!["严禁使用 allow".to_string()]),
        3000,
    );
    NotesStore::write_file(
        &temp_dir,
        "checkpoint.md",
        "已验证: 分片存储落地\n未决: 完成 cron 清理 notes 升级\n下一步: 执行全仓 cargo test",
        3001,
    )
    .expect("write notes file");
    NotesStore::write_file(&temp_dir, "files.md", "crates/axon-agent-runtime", 3002)
        .expect("write notes file");

    let canonical_summary = notes
        .try_render_canonical_summary(&temp_dir, &[])
        .expect("生成 canonical checkpoint");
    assert!(canonical_summary.contains("P0:"));
    assert!(canonical_summary.contains("TASK:\n执行压缩机制目标态重构"));
    assert!(canonical_summary.contains("CONSTRAINTS:\n- 严禁使用 allow"));
    assert!(canonical_summary.contains("便签 checkpoint.md"));
    assert!(canonical_summary.contains("已验证: 分片存储落地"));
    assert!(canonical_summary.contains("未决: 完成 cron 清理 notes 升级"));
    assert!(canonical_summary.contains("VERIFIED:"));
    assert!(canonical_summary.contains("OPEN:"));
    assert!(canonical_summary.contains("builtin.notes_read_file"));
    assert!(canonical_summary.contains("builtin.history_search_contents"));
    assert!(canonical_summary.contains("TOOLS:\n- none"));

    let parsed = crate::CanonicalCheckpoint::parse(&canonical_summary, &[]);
    assert!(
        parsed.is_ok(),
        "生成的 canonical 摘要必须符合门禁: {:?}",
        parsed.err()
    );
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn notes_store_enforces_single_file_byte_limit() {
    use super::session_notes_store::{NOTES_MAX_FILE_BYTES, NotesStore};

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();

    let at_limit = "a".repeat(NOTES_MAX_FILE_BYTES);
    let receipt = NotesStore::write_file(session_dir, "limit.md", &at_limit, 1_000)
        .expect("恰好达到上限应允许写入");
    assert_eq!(receipt.written_bytes, NOTES_MAX_FILE_BYTES);

    let overflow = "a".repeat(NOTES_MAX_FILE_BYTES + 1);
    let error = NotesStore::write_file(session_dir, "overflow.md", &overflow, 1_000)
        .expect_err("超过上限必须拒绝写入");
    assert!(format!("{error:?}").contains("超过上限"));
}

#[test]
fn notes_store_rejects_append_growth_over_byte_limit() {
    use super::session_notes_store::{NOTES_MAX_FILE_BYTES, NotesStore};

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let base = "a".repeat(NOTES_MAX_FILE_BYTES - 1);
    NotesStore::write_file(session_dir, "append.md", &base, 1_000).expect("基线写入应成功");

    let error = NotesStore::append_to_file(session_dir, "append.md", "bb", 2_000)
        .expect_err("追加后越界必须拒绝");
    assert!(format!("{error:?}").contains("超过上限"));

    NotesStore::append_to_file(session_dir, "append.md", "b", 2_000)
        .expect("恰好达到上限应允许追加");
}

#[test]
fn notes_store_reads_negative_line_range_from_tail() {
    use super::session_notes_store::NotesStore;

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    NotesStore::write_file(session_dir, "lines.md", "l1\nl2\nl3\nl4", 1_000).expect("写入应成功");

    let tail =
        NotesStore::read_file(session_dir, "lines.md", Some(-2), None).expect("负起始行应可读取");
    assert_eq!(tail.text, "l3\nl4");
    assert_eq!(tail.start_line, 3);
    assert_eq!(tail.stop_line, 4);

    let head =
        NotesStore::read_file(session_dir, "lines.md", None, Some(-3)).expect("负结束行应可读取");
    assert_eq!(head.text, "l1\nl2");
    assert_eq!(head.stop_line, 2);
}

#[test]
fn notes_store_search_is_case_sensitive() {
    use super::session_notes_store::NotesStore;

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    NotesStore::write_file(session_dir, "case.md", "Alpha\nbeta\nalpha", 1_000)
        .expect("写入应成功");

    let upper = NotesStore::search_contents(session_dir, "Alpha", None, None, None, false)
        .expect("检索应成功");
    assert_eq!(upper.files.len(), 1);
    assert_eq!(upper.files[0].matches.len(), 1);
    assert_eq!(upper.files[0].matches[0].line_number, 1);

    let lower = NotesStore::search_contents(session_dir, "alpha", None, None, None, false)
        .expect("检索应成功");
    assert_eq!(lower.files.len(), 1);
    assert_eq!(lower.files[0].matches[0].line_number, 3);
}

#[test]
fn notes_store_rejects_path_traversal_and_local_paths() {
    use super::session_notes_store::normalize_note_path;

    for path in [
        "../escape.md",
        "/abs.md",
        "a/../b.md",
        "a//b.md",
        ".",
        "..",
        "a\\b.md",
        "C:/x.md",
        "",
    ] {
        assert!(normalize_note_path(path).is_err(), "路径必须被拒绝：{path}");
    }
    assert_eq!(
        normalize_note_path(" handoff/open-issues.md ").expect("合法路径应通过"),
        "handoff/open-issues.md"
    );
    assert_eq!(
        normalize_note_path("~literal.md").expect("`~` 按字面量处理"),
        "~literal.md"
    );
}

#[test]
fn notes_store_lists_files_by_name_order() {
    use super::session_notes_store::{NotesFileOrder, NotesFileOrderBy, NotesStore};

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    for path in ["b.md", "a.md", "dir/c.md"] {
        NotesStore::write_file(session_dir, path, "x", 1_000).expect("写入应成功");
    }

    let ascending = NotesStore::list_files_by_prefix(
        session_dir,
        None,
        None,
        NotesFileOrderBy::Name,
        NotesFileOrder::Ascending,
    )
    .expect("列举应成功");
    let paths = ascending
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["a.md", "b.md", "dir/c.md"]);
}

#[test]
fn notes_store_rejects_blank_queries_and_invalid_result_limits() {
    use super::session_notes_store::{NOTES_LIST_MAX_RESULTS, NotesFileOrder, NotesFileOrderBy};

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();

    for query in ["", "   ", "\t\n"] {
        let error = NotesStore::search_contents(session_dir, query, None, None, None, false)
            .expect_err("空白 query 必须 fail-closed");
        assert!(format!("{error:?}").contains("不能为空"));
    }

    let error = NotesStore::list_files_by_prefix(
        session_dir,
        None,
        Some(0),
        NotesFileOrderBy::Name,
        NotesFileOrder::Ascending,
    )
    .expect_err("max_results=0 必须被拒绝");
    assert!(format!("{error:?}").contains("结果数量"));

    let error = NotesStore::list_files_by_prefix(
        session_dir,
        None,
        Some(NOTES_LIST_MAX_RESULTS + 1),
        NotesFileOrderBy::Name,
        NotesFileOrder::Ascending,
    )
    .expect_err("超过 max_results 硬上限必须被拒绝");
    assert!(format!("{error:?}").contains("结果数量"));

    let error = NotesStore::search_contents(
        session_dir,
        "x",
        None,
        Some(NOTES_LIST_MAX_RESULTS + 1),
        None,
        false,
    )
    .expect_err("超过 max_files 硬上限必须被拒绝");
    assert!(format!("{error:?}").contains("结果数量"));
}
