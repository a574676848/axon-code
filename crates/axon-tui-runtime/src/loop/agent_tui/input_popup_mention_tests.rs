//! 输入区的弹层与原子提及：光标跳过、整体删除与替换，以及斜杠命令的引用弹层。

use super::*;

#[test]
fn cursor_movement_skips_atomic_mention_boundaries() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("$rev".to_owned());
    assert!(state.accept_selected_composer_popup_item());
    state.move_cursor_home();

    state.move_cursor_right();
    assert_eq!(state.composer.draft.cursor(), "$review".chars().count());

    state.move_cursor_left();
    assert_eq!(state.composer.draft.cursor(), 0);
}

#[test]
fn backspace_inside_mention_removes_whole_atomic_element() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("先 $rev 后".to_owned());
    state.move_cursor_home();
    for _ in 0.."先 $rev".chars().count() {
        state.move_cursor_right();
    }
    assert!(state.accept_selected_composer_popup_item());
    state.move_cursor_home();
    for _ in 0.."先 ".chars().count() {
        state.move_cursor_right();
    }
    state.backspace();
    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert_eq!(submission.input.text(), "先 后");
    assert!(submission.input.elements().is_empty());
}

#[test]
fn popup_skill_selection_replaces_only_current_token() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("修改文件 $rev 并运行测试".to_owned());
    state.move_cursor_home();
    for _ in 0.."修改文件 $rev".chars().count() {
        state.move_cursor_right();
    }
    assert!(state.accept_selected_composer_popup_item());

    assert_eq!(state.composer.draft.text(), "修改文件 $review 并运行测试");
    assert_eq!(
        state.composer.draft.cursor(),
        "修改文件 $review ".chars().count()
    );
}

#[test]
fn popup_uses_token_at_cursor_instead_of_last_token() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    catalog.skills.push("frontend-design".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("用 $rev 处理 $front".to_owned());
    state.move_cursor_home();
    for _ in 0.."用 $rev".chars().count() {
        state.move_cursor_right();
    }

    assert!(
        state.composer_popup_open(),
        "光标所在 $rev 应打开 skill 弹层"
    );
    assert!(state.accept_selected_composer_popup_item());

    assert_eq!(state.composer.draft.text(), "用 $review 处理 $front");
    assert_eq!(state.composer.draft.cursor(), "用 $review ".chars().count());
}

#[test]
fn popup_multiple_mentions_keep_distinct_protocol_targets() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    catalog.connectors.push("jira".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("先 $rev 再 %jir".to_owned());
    state.move_cursor_home();
    for _ in 0.."先 $rev".chars().count() {
        state.move_cursor_right();
    }
    assert!(state.accept_selected_composer_popup_item());
    state.move_cursor_end();
    assert!(state.accept_selected_composer_popup_item());
    assert_eq!(state.composer.draft.text(), "先 $review 再 %jira ");

    let submission = state.take_submit_from_draft().expect("应生成提交");
    let targets = submission
        .input
        .elements()
        .iter()
        .filter_map(|element| match &element.kind {
            AgentTuiInputElementKind::Mention { target, .. } => Some(target.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(targets.contains(&"skill://review"));
    assert!(targets.contains(&"connector://jira"));
    assert_eq!(submission.input.text(), "先 $review 再 %jira");
}

#[test]
fn typing_inside_atomic_mention_replaces_whole_element() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("使用 $rev 完成".to_owned());
    state.move_cursor_home();
    for _ in 0.."使用 $rev".chars().count() {
        state.move_cursor_right();
    }
    assert!(state.accept_selected_composer_popup_item());
    assert_eq!(state.composer.draft.text(), "使用 $review 完成");

    state.composer.draft.set_cursor("使用 $re".chars().count());
    state.push('x');
    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert_eq!(submission.input.text(), "使用 x 完成");
    assert!(
        submission
            .input
            .elements()
            .iter()
            .all(|element| !matches!(element.kind, AgentTuiInputElementKind::Mention { .. })),
        "原子 mention 被替换后不应残留结构化 metadata"
    );
}

#[test]
fn selected_skill_mention_projects_protocol_target_on_submit() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("请 $rev 检查".to_owned());
    state.move_cursor_home();
    for _ in 0.."请 $rev".chars().count() {
        state.move_cursor_right();
    }
    assert!(state.accept_selected_composer_popup_item());
    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert_eq!(submission.input.text(), "请 $review 检查");
    assert!(submission.input.elements().iter().any(|element| matches!(
        &element.kind,
        AgentTuiInputElementKind::Mention { sigil: '$', name, target }
            if name == "review" && target == "skill://review"
    )));
}

#[test]
fn popup_file_selection_becomes_file_mention_without_clearing_context() {
    let dir = axon_test_support::test_temp_dir();
    std::fs::write(dir.path().join("README.md"), "fixture").expect("测试文件应可写入");
    let mut request = test_request(AgentTuiCatalog::default());
    request.cwd = dir.path().to_path_buf();
    let mut state = AgentTuiState::new_for_test(&request);

    state.replace_draft("读取 @read".to_owned());
    wait_for_composer_popup(&mut state);
    assert!(state.accept_selected_composer_popup_item());
    state.insert_str("并总结");
    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert_eq!(submission.input.text(), "读取 @README.md 并总结");
    assert_eq!(submission.cwd, request.cwd);
    assert!(submission.input.elements().iter().any(|element| matches!(
        &element.kind,
        AgentTuiInputElementKind::Mention { sigil: '@', name, target }
            if name == "README.md" && target == "file://README.md"
    )));
    assert!(
        submission.file_paths().is_empty(),
        "@ 文件引用不应退回附件路径"
    );
}

#[test]
fn popup_nested_file_selection_binds_relative_file_target() {
    let dir = axon_test_support::test_temp_dir();
    let nested = dir.path().join("crates").join("axon").join("src");
    std::fs::create_dir_all(&nested).expect("嵌套目录应可创建");
    std::fs::write(nested.join("model_router.rs"), "fixture").expect("测试文件应可写入");
    let mut request = test_request(AgentTuiCatalog::default());
    request.cwd = dir.path().to_path_buf();
    let mut state = AgentTuiState::new_for_test(&request);

    state.replace_draft("读取 @file:crates/axon/src/mo 并说明".to_owned());
    state.move_cursor_home();
    for _ in 0.."读取 @file:crates/axon/src/mo".chars().count() {
        state.move_cursor_right();
    }
    wait_for_composer_popup(&mut state);
    assert!(state.accept_selected_composer_popup_item());
    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert_eq!(
        submission.input.text(),
        "读取 @crates/axon/src/model_router.rs 并说明"
    );
    assert!(submission.input.elements().iter().any(|element| matches!(
        &element.kind,
        AgentTuiInputElementKind::Mention { sigil: '@', name, target }
            if name == "crates/axon/src/model_router.rs"
                && target == "file://crates/axon/src/model_router.rs"
    )));
}

#[test]
fn popup_image_completion_filters_non_image_files() {
    let dir = axon_test_support::test_temp_dir();
    std::fs::write(dir.path().join("logo.png"), "fixture").expect("图片文件应可写入");
    std::fs::write(dir.path().join("logo.txt"), "fixture").expect("文本文件应可写入");
    let mut request = test_request(AgentTuiCatalog::default());
    request.cwd = dir.path().to_path_buf();
    let mut state = AgentTuiState::new_for_test(&request);

    state.replace_draft("查看 @image:lo".to_owned());
    wait_for_composer_popup(&mut state);
    assert!(state.accept_selected_composer_popup_item());
    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert_eq!(submission.input.text(), "查看 @logo.png");
    assert!(submission.input.elements().iter().any(|element| matches!(
        &element.kind,
        AgentTuiInputElementKind::Mention { sigil: '@', name, target }
            if name == "logo.png" && target == "file://logo.png"
    )));
    assert!(!submission.input.text().contains("logo.txt"));
}

#[test]
fn shell_popup_accept_enters_shell_command_mode() {
    let mut state = test_state(AgentTuiCatalog::default());

    state.replace_draft("!sh".to_owned());
    assert!(state.composer_popup_open());
    assert!(state.accept_selected_composer_popup_item());

    assert_eq!(state.composer.draft.text(), "! ");
    assert!(!state.composer_popup_open());
}

#[test]
fn repeated_slash_mcp_updates_single_mcp_surface() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.mcp_entries.push("gitnexus/query".to_owned());
    catalog.mcp_entries.push("openviking/search".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("/mcp".to_owned());
    assert!(state.take_submit_from_draft().is_none());
    state.replace_draft("/mcp".to_owned());
    assert!(state.take_submit_from_draft().is_none());

    assert_eq!(cell_count(&state, RenderCellKind::MCP), 1);
    assert!(state.cells.transcript.iter().any(|line| matches!(
        &line.data,
        Some(RenderCellData::Mcp(cell))
            if cell.items.len() == 1
                && cell.items[0].server == "MCP catalog"
                && cell.items[0].status == AgentRenderLifecycleStatus::Running
    )));
}
