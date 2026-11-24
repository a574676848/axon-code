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
    // 只写事实便签、不写交接便签 checkpoint.md：此时运行时按目标锚点与便签投影。
    NotesStore::write_file(&temp_dir, "files.md", "crates/axon-agent-runtime", 3002)
        .expect("write notes file");

    let canonical_summary = notes
        .try_render_canonical_summary(&temp_dir, &[])
        .expect("生成 canonical checkpoint");
    assert!(canonical_summary.contains("P0:"));
    assert!(canonical_summary.contains("TASK:\n执行压缩机制目标态重构"));
    assert!(canonical_summary.contains("CONSTRAINTS:\n- 严禁使用 allow"));
    assert!(canonical_summary.contains("便签 files.md"));
    assert!(canonical_summary.contains("crates/axon-agent-runtime"));
    assert!(canonical_summary.contains("VERIFIED:"));
    assert!(canonical_summary.contains("TOOLS:\n- none"));
    // 回读路线只由换窗运行时 note（insert_then_compress_history）承担：投影里再出现一次
    // 就是同一句话两份，模型还会把摘要当成操作说明。
    assert!(!canonical_summary.contains("OPEN:"));
    assert!(!canonical_summary.contains("builtin.notes_read_file"));
    assert!(!canonical_summary.contains("builtin.history_"));

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
        normalize_note_path(" archive/old-notes.md ").expect("合法路径应通过"),
        "archive/old-notes.md"
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

/// 写入的便签必须原样采用：运行时不改写、不重排、不补段。
#[test]
fn session_notes_reuses_model_checkpoint_note_verbatim() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let notes = SessionNotes::new(
        GoalAnchor::new(
            "目标锚点不得覆盖模型便签",
            vec!["约束项不得覆盖模型便签".to_string()],
        ),
        4_000,
    );
    let submitted = "P0:\nTASK:\n模型自己写的任务\n\nCONSTRAINTS:\n- 模型自己写的约束\n\nVERIFIED:\n- 已验证事实\n\nOPEN:\n- 未决问题\n\nTOOLS:\n- none";
    NotesStore::write_file(
        session_dir,
        SESSION_NOTES_CHECKPOINT_NOTE_PATH,
        submitted,
        4_001,
    )
    .expect("写入模型便签");

    let summary = notes
        .try_render_canonical_summary(session_dir, &[])
        .expect("模型便签应被采用");

    assert_eq!(
        summary, submitted,
        "便签必须逐字节原样采用，不得被运行时改写"
    );
    assert!(
        !summary.contains("目标锚点不得覆盖模型便签"),
        "便签优先级必须高于目标锚点投影"
    );
}

/// 便签只看路径：内容怎么写都原样采用，不被改写成模板。
#[test]
fn session_notes_accepts_free_form_checkpoint_note() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let notes = SessionNotes::new(
        GoalAnchor::new("投影目标", vec!["投影约束".to_string()]),
        5_000,
    );
    let long_tail = "尾部事实".repeat(200);
    let submitted = format!("# Goal Checkpoint\n{long_tail}");
    NotesStore::write_file(
        session_dir,
        SESSION_NOTES_CHECKPOINT_NOTE_PATH,
        &submitted,
        5_001,
    )
    .expect("写入自由格式摘要");

    let summary = notes
        .try_render_canonical_summary(session_dir, &[])
        .expect("自由格式摘要必须被采用");

    assert_eq!(summary, submitted, "便签必须逐字节原样采用，不得改写成模板");
    assert!(
        !summary.contains("投影目标"),
        "模型提交优先于目标锚点投影：{summary}"
    );
}

/// 超预算的提交摘要走投影而不是拒绝换窗：换窗是会话自救的唯一动作。
#[test]
fn over_budget_checkpoint_note_falls_back_to_projection() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let notes = SessionNotes::new(GoalAnchor::new("预算目标", Vec::new()), 6_000);
    let over_budget_bytes =
        usize::try_from(SESSION_NOTES_CHECKPOINT_MAX_TOKENS).expect("预算必须可转换") * 5;
    let oversized = "x".repeat(over_budget_bytes);
    NotesStore::write_file(
        session_dir,
        SESSION_NOTES_CHECKPOINT_NOTE_PATH,
        &oversized,
        6_001,
    )
    .expect("写入超预算摘要");

    let (summary, source) = notes
        .render_canonical_summary_with_source(session_dir, &[])
        .expect("超预算摘要不得阻断换窗");

    assert_eq!(
        source,
        CheckpointSource::Projection,
        "超预算摘要必须降级为投影"
    );
    assert!(
        summary.contains("预算目标"),
        "投影必须落在当前目标锚点上：{summary}"
    );
    assert!(
        !summary.contains(&"x".repeat(1_000)),
        "超预算摘要的正文不得带进新窗口"
    );
}

/// 投影自己也超预算时仍然 fail-closed：便签总量失控不能靠换窗掩盖。
#[test]
fn session_notes_fails_closed_when_projection_exceeds_budget() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let notes = SessionNotes::new(GoalAnchor::new("投影预算目标", Vec::new()), 6_500);
    for index in 0..12 {
        NotesStore::write_file(
            session_dir,
            &format!("bulk{index}.md"),
            &"事实".repeat(600),
            6_501,
        )
        .expect("写入大号便签");
    }

    let error = notes
        .try_render_canonical_summary(session_dir, &[])
        .expect_err("投影超预算必须拒绝");

    assert!(
        format!("{error:?}").contains("超出预算"),
        "错误信息必须说明超出预算：{error:?}"
    );
}

/// 超过预算的便签走投影并只留路径：正文再长也不会整段顶进新窗口。
#[test]
fn session_notes_references_long_notes_instead_of_inlining_body() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let notes = SessionNotes::new(GoalAnchor::new("长便签目标", Vec::new()), 7_000);
    // 24_000 字符（约 72 KiB）超过 32 KiB 字节硬上限，必然回落投影。
    let long_body = "长便签正文".repeat(4_000);
    NotesStore::write_file(session_dir, "checkpoint.md", &long_body, 7_001).expect("写入长便签");

    let summary = notes
        .try_render_canonical_summary(session_dir, &[])
        .expect("长便签不应导致换窗失败");

    assert!(
        summary.contains("便签 checkpoint.md"),
        "长便签必须保留可定位的路径"
    );
    assert!(
        summary.contains("builtin.notes_read_file"),
        "长便签必须给出回读指引"
    );
    assert!(
        !summary.contains(&"长便签正文".repeat(20)),
        "超过内联上限的便签正文不得整段带入摘要"
    );
}

/// v4 文件必须可读并就地升到 v5，且未显式保存前不改写磁盘上的旧文件。
#[test]
fn session_notes_reads_v4_file_and_upgrades_in_memory() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    std::fs::create_dir_all(session_dir).expect("创建会话目录");
    let legacy = r#"{
  "schema_version": 4,
  "goal_anchor": {
    "user_objective": "v4 目标",
    "non_negotiable_constraints": ["v4 约束"]
  },
  "window_number": 3,
  "first_window_id": "w1",
  "previous_window_id": "w2",
  "current_window_id": "w3",
  "last_rollover_lifecycle_id": null,
  "last_archived_previous_window_id": null,
  "last_rollover_revision": null,
  "revision": 7,
  "updated_at_ms": 1234
}"#;
    let legacy_path = session_dir.join(SESSION_NOTES_FILE_NAME);
    std::fs::write(&legacy_path, legacy).expect("写入 v4 文件");

    let notes = SessionNotes::load_from_session(session_dir)
        .expect("v4 文件必须可读")
        .expect("v4 文件必须存在");

    assert_eq!(notes.schema_version, SESSION_NOTES_SCHEMA_VERSION);
    assert_eq!(notes.window_number, 3);
    assert_eq!(notes.goal_anchor.user_objective, "v4 目标");
    assert_eq!(notes.checkpoint_source, None, "旧文件缺省来源必须为 None");
    assert_eq!(notes.checkpoint_window_number, None);
    assert_eq!(
        std::fs::read_to_string(&legacy_path).expect("读回旧文件"),
        legacy,
        "迁移只发生在内存，未显式保存前不得改写旧文件"
    );
}

/// 低于最低支持版本必须显式报错，并说明当前支持区间。
#[test]
fn session_notes_rejects_unsupported_schema_version() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    std::fs::create_dir_all(session_dir).expect("创建会话目录");
    std::fs::write(
        session_dir.join(SESSION_NOTES_FILE_NAME),
        r#"{"schema_version": 3}"#,
    )
    .expect("写入 v3 文件");

    let error =
        SessionNotes::load_from_session(session_dir).expect_err("低于最低支持版本必须显式拒绝");

    let message = format!("{error:?}");
    assert!(message.contains("schema 版本不支持"), "{message}");
    assert!(message.contains("当前支持 4 到 5"), "{message}");
}

/// 新建便签落盘时写当前版本。
#[test]
fn session_notes_saves_current_schema_version() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let notes = SessionNotes::new(GoalAnchor::new("新目标", Vec::new()), 9_000);

    let path = notes.save_to_session(session_dir).expect("保存成功");
    let saved = std::fs::read_to_string(&path).expect("读取保存结果");

    assert!(
        saved.contains("\"schema_version\": 5"),
        "新写文件必须是当前版本：{saved}"
    );
}

/// 便签数量超过列举硬上限时必须显式失败：换窗摘要不能静默漏掉后续便签。
#[test]
fn notes_store_fails_closed_when_note_count_exceeds_limit() {
    use crate::session_notes_store::NOTES_LIST_MAX_RESULTS;

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    for index in 0..=NOTES_LIST_MAX_RESULTS {
        NotesStore::write_file(session_dir, &format!("n{index}.md"), "x", 1_000).expect("写入便签");
    }

    let error = NotesStore::render_checkpoint(session_dir).expect_err("超量便签必须拒绝");

    let message = format!("{error:?}");
    assert!(message.contains("超过上限"), "{message}");
    assert!(
        message.contains("无法完整覆盖"),
        "错误信息必须说明摘要不完整：{message}"
    );
}

/// 相邻窗口写入的便签仍是有效交接：换窗只按预算取舍，不再做过期判定。
#[test]
fn checkpoint_note_from_previous_window_is_still_reused() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let mut notes = SessionNotes::new(GoalAnchor::new("正常交接目标", Vec::new()), 9_100);
    notes.window_number = 2;
    notes.checkpoint_window_number = Some(1);
    let submitted = "P0:\nTASK:\n上一窗提交的任务\n\nCONSTRAINTS:\n- 约束\n\nVERIFIED:\n- 事实\n\nOPEN:\n- 无\n\nTOOLS:\n- none";
    NotesStore::write_file(
        session_dir,
        SESSION_NOTES_CHECKPOINT_NOTE_PATH,
        submitted,
        9_101,
    )
    .expect("写入提交摘要");

    let (summary, source) = notes
        .render_canonical_summary_with_source(session_dir, &[])
        .expect("正常交接必须成功");

    assert_eq!(source, CheckpointSource::Model);
    assert_eq!(summary, submitted);
}
