use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use super::{
    AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiInputElementKind, AgentTuiInputResult,
    AgentTuiLaunchRequest, AgentTuiModelCatalogItem, AgentTuiQueuedAction, AgentTuiRunOverrides,
    state::AgentTuiState,
};
use crate::surface::cell::{RenderCellData, RenderCellKind, RenderCellMode};
use crate::surface::cell_event_data::AgentRenderLifecycleStatus;

#[test]
fn image_draft_is_blocked_when_current_model_lacks_vision() {
    let image = temp_file("axon-vision-off-image.png");
    let mut state = test_state_with_model(
        AgentTuiCatalog {
            models: vec![
                AgentTuiModelCatalogItem::new("tui-vision-off", "tui-vision-off", "fixture")
                    .with_supports_vision(false),
            ],
            ..AgentTuiCatalog::default()
        },
        "tui-vision-off",
    );

    state.replace_draft("分析".to_owned());
    state.paste(&image.display().to_string());

    assert!(state.composer.draft.text().contains("[Image #1]"));
    assert!(
        state.draft_blocks_unsupported_image("分析 [Image #1]"),
        "不支持视觉的模型必须阻断含图片附件的草稿"
    );
    assert!(
        state.take_submit_from_draft().is_none(),
        "预检未通过时不得产生提交"
    );
}

#[test]
fn image_draft_submits_when_current_model_supports_vision() {
    let image = temp_file("axon-vision-on-image.png");
    let mut state = test_state_with_model(
        AgentTuiCatalog {
            models: vec![
                AgentTuiModelCatalogItem::new("tui-vision-on", "tui-vision-on", "fixture")
                    .with_supports_vision(true),
            ],
            ..AgentTuiCatalog::default()
        },
        "tui-vision-on",
    );

    state.replace_draft("分析".to_owned());
    state.paste(&image.display().to_string());

    assert!(
        !state.draft_blocks_unsupported_image("分析 [Image #1]"),
        "支持视觉的模型不得被预检阻断"
    );
    let submission = state.take_submit_from_draft().expect("应生成提交");
    assert_eq!(submission.file_paths().len(), 1);
}

#[test]
fn plain_submit_preserves_input_elements() {
    let mut state = test_state(AgentTuiCatalog::default());
    let file = temp_file("axon-input-file.txt");
    let image = temp_file("axon-input-image.png");

    state.replace_draft("分析附件".to_owned());
    state.paste(&file.display().to_string());
    state.paste(&image.display().to_string());
    state.paste(&"x".repeat(1001));

    assert!(state.composer.draft.text().contains("[File #1]"));
    assert!(state.composer.draft.text().contains("[Image #2]"));
    assert!(!state.composer.draft.text().contains("axon-input-image.png"));

    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert!(matches!(
        submission.input,
        AgentTuiInputResult::Submit { .. }
    ));
    assert!(submission.text().contains("[File #1]"));
    assert!(submission.text().contains("[Image #2]"));
    assert!(!submission.text().contains("axon-input-image.png"));
    assert!(submission.text().contains(&"x".repeat(1001)));
    let file_paths = submission.file_paths();
    assert_eq!(file_paths.len(), 2);
    assert!(file_paths.contains(&file.canonicalize().expect("文件附件应可规范化")));
    assert!(file_paths.contains(&image.canonicalize().expect("图片附件应可规范化")));
    assert!(
        submission
            .input
            .elements()
            .iter()
            .any(|element| matches!(element.kind, AgentTuiInputElementKind::LargePaste { .. }))
    );
}

#[test]
fn deleting_inline_attachment_placeholder_removes_file_payload() {
    let mut state = test_state(AgentTuiCatalog::default());
    let image = temp_file("axon-delete-inline-image.png");

    state.replace_draft("分析".to_owned());
    state.paste(&image.display().to_string());
    state.move_cursor_end();
    state.backspace();

    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert_eq!(submission.text(), "分析");
    assert!(submission.file_paths().is_empty());
}

#[test]
fn history_save_failure_renders_typed_error_cell() {
    let runtime_root = axon_test_support::test_temp_dir();
    let runtime_tmp_file = runtime_root.path().join("runtime-file");
    fs::write(&runtime_tmp_file, b"fixture").expect("文件父路径应创建成功");
    let mut state = test_state(AgentTuiCatalog::default());
    state.composer.history_path = runtime_tmp_file.join("history.jsonl");

    state.replace_draft("触发历史保存".to_owned());
    let submission = state.take_submit_from_draft();
    let line = state.cells.transcript.last().expect("应输出历史保存失败");

    assert!(submission.is_some());
    assert!(matches!(
        line.mode,
        RenderCellMode::Cell(RenderCellKind::ERROR)
    ));
    match &line.data {
        Some(RenderCellData::Error(cell)) => assert_eq!(cell.next_step, None),
        _ => panic!("应输出 typed error cell"),
    }
}

#[test]
fn queued_user_messages_keep_fifo_order_without_overwriting() {
    let mut state = test_state(AgentTuiCatalog::default());

    state.replace_draft("第一条".to_owned());
    assert!(state.queue_draft_for_next_turn());
    state.replace_draft("第二条".to_owned());
    assert!(state.queue_draft_for_next_turn());
    state.replace_draft("!cargo check".to_owned());
    assert!(state.queue_draft_for_next_turn());

    let preview = state.queued_submit_preview().expect("应显示队列预览");
    assert!(preview.contains("待发送 1. 第一条"));
    assert!(preview.contains("待发送 2. 第二条"));
    assert!(preview.contains("待发送 3. !cargo check"));

    let first = state.take_queued_submit().expect("第一条应存在");
    let second = state.take_queued_submit().expect("第二条应存在");
    let third = state.take_queued_submit().expect("第三条应存在");

    assert_eq!(first.text(), "第一条");
    assert_eq!(second.text(), "第二条");
    assert_eq!(third.text(), "!cargo check");
    assert!(matches!(
        third.input,
        AgentTuiInputResult::Queued {
            action: AgentTuiQueuedAction::RunShell,
            ..
        }
    ));
    assert!(state.take_queued_submit().is_none());
}

#[test]
fn restoring_queued_submit_edits_most_recent_and_keeps_fifo_prefix() {
    let mut state = test_state(AgentTuiCatalog::default());

    state.replace_draft("第一条".to_owned());
    assert!(state.queue_draft_for_next_turn());
    state.replace_draft("第二条".to_owned());
    assert!(state.queue_draft_for_next_turn());

    assert!(state.restore_interrupt_input_to_draft());
    assert_eq!(state.composer.draft.text(), "第二条");

    let remaining = state.take_queued_submit().expect("较早队列项应保留");
    assert_eq!(remaining.text(), "第一条");
    assert!(state.take_queued_submit().is_none());
}

#[test]
fn empty_draft_is_not_queued_while_existing_queue_remains() {
    let mut state = test_state(AgentTuiCatalog::default());

    state.replace_draft("已有排队".to_owned());
    assert!(state.queue_draft_for_next_turn());
    state.replace_draft("   ".to_owned());

    assert!(!state.queue_draft_for_next_turn());
    assert!(state.has_queued_submit());
    assert_eq!(
        state
            .take_queued_submit()
            .map(|submit| submit.text().to_owned()),
        Some("已有排队".to_owned())
    );
}

#[test]
fn queued_preview_compacts_longer_fifo_queue() {
    let mut state = test_state(AgentTuiCatalog::default());

    for index in 1..=6 {
        state.replace_draft(format!("任务 {index}"));
        assert!(state.queue_draft_for_next_turn());
    }

    let preview = state.queued_submit_preview().expect("应显示队列预览");
    assert!(preview.contains("待发送 1. 任务 1"));
    assert!(preview.contains("待发送 2. 任务 2"));
    assert!(preview.contains("待发送 3. 任务 3"));
    assert!(!preview.contains("待发送 4. 任务 4"));
    assert!(!preview.contains("待发送 5. 任务 5"));
    assert!(preview.contains("另 3 条待处理"));
    assert_eq!(preview.lines().count(), 4);
    assert_eq!(state.queued_submit_preview_height(), 5);
}

#[test]
fn local_catalog_commands_do_not_submit_to_model() {
    let mcp_cases = [("/mcp", RenderCellKind::MCP)];

    for (command, expected_kind) in mcp_cases {
        let mut catalog = AgentTuiCatalog::default();
        catalog.skills.push("review".to_owned());
        catalog.mcp_entries.push("gitnexus/query".to_owned());
        let mut state = test_state(catalog);

        state.replace_draft(command.to_owned());
        let submission = state.take_submit_from_draft();
        let line = state
            .cells
            .transcript
            .last()
            .expect("本地命令应输出 transcript cell");

        assert!(submission.is_none(), "{command} 不应提交模型");
        assert!(
            state.composer.draft.text().is_empty(),
            "{command} 应清空输入区"
        );
        assert!(!state.composer_popup_open(), "{command} 应关闭弹层");
        assert!(
            matches!(line.mode, RenderCellMode::Cell(kind) if kind == expected_kind),
            "{command} 应输出对应结构化 cell"
        );
    }
}

#[test]
fn skills_list_command_opens_skill_reference_popup() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("/skills list".to_owned());
    let submission = state.take_submit_from_draft();

    assert!(submission.is_none(), "/skills list 不应提交模型");
    assert_eq!(state.composer.draft.text(), "$");
    assert!(
        state.composer_popup_open(),
        "/skills list 应打开 skill 引用弹层"
    );
}

#[test]
fn slash_skills_argument_suggestion_uses_management_actions_only() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    catalog.skills.push("frontend-design".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("/skills l".to_owned());
    assert!(state.command_suggestions_open());
    assert!(state.accept_selected_command_suggestion_for_enter());

    assert_eq!(state.composer.draft.text(), "/skills list");
}

#[test]
fn restoring_queued_submit_renders_typed_history_cell() {
    let mut state = test_state(AgentTuiCatalog::default());

    state.replace_draft("下一轮任务".to_owned());
    assert!(state.queue_draft_for_next_turn());
    assert!(state.restore_interrupt_input_to_draft());
    let line = state.cells.transcript.last().expect("应输出恢复记录");

    assert!(matches!(
        line.mode,
        RenderCellMode::Cell(RenderCellKind::HISTORY)
    ));
    match &line.data {
        Some(RenderCellData::History(cell)) => {
            assert_eq!(cell.title, "输入");
            let item = cell.items.first().expect("应输出恢复条目");
            assert_eq!(item.name, "已恢复到输入区");
            assert_eq!(item.kind_label.as_deref(), Some("待重新发送"));
            assert_eq!(item.status_label.as_deref(), Some("下一轮任务"));
        }
        _ => panic!("应输出 typed history cell"),
    }
}

#[test]
fn popup_skill_selection_becomes_mention_element() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("$rev".to_owned());
    assert!(state.accept_selected_composer_popup_item());
    state.insert_str("检查提交");
    let submission = state.take_submit_from_draft().expect("应生成提交");

    assert!(submission.input.elements().iter().any(|element| matches!(
        &element.kind,
        AgentTuiInputElementKind::Mention { sigil: '$', name, target }
            if name == "review" && target == "skill://review"
    )));
}

#[test]
fn disabled_skill_is_hidden_from_reference_popup() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.skills.push("review".to_owned());
    catalog.disabled_skills.insert("review".to_owned());
    let mut state = test_state(catalog);

    state.replace_draft("$rev".to_owned());

    assert!(
        !state.composer_popup_open(),
        "禁用 skill 不应进入 $ 引用弹层"
    );
}

#[test]
fn slash_help_outputs_compact_history_cell() {
    let mut state = test_state(AgentTuiCatalog::default());

    state.replace_draft("/help".to_owned());
    let submission = state.take_submit_from_draft();
    let line = state.cells.transcript.last().expect("应输出命令 cell");

    assert!(submission.is_none());
    assert!(matches!(
        line.mode,
        RenderCellMode::Cell(RenderCellKind::HISTORY)
    ));
    assert_eq!(line.text, "命令");
    match &line.data {
        Some(RenderCellData::History(cell)) => {
            assert_eq!(
                cell.items.first().map(|item| item.name.as_str()),
                Some("/model")
            );
        }
        _ => panic!("应输出 typed history cell"),
    }
}

#[test]
fn repeated_slash_help_updates_single_history_surface() {
    let mut state = test_state(AgentTuiCatalog::default());

    state.replace_draft("/help".to_owned());
    assert!(state.take_submit_from_draft().is_none());
    state.replace_draft("/help".to_owned());
    assert!(state.take_submit_from_draft().is_none());

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
}

fn cell_count(state: &AgentTuiState, kind: RenderCellKind) -> usize {
    state
        .cells
        .transcript
        .iter()
        .filter(|line| matches!(line.mode, RenderCellMode::Cell(cell_kind) if cell_kind == kind))
        .count()
}

fn wait_for_composer_popup(state: &mut AgentTuiState) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        state.tick(Instant::now());
        if state.composer_popup_open() {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("composer popup 未在 500ms 内就绪");
}

fn test_state(catalog: AgentTuiCatalog) -> AgentTuiState {
    AgentTuiState::new_for_test(&test_request(catalog))
}

fn test_state_with_model(catalog: AgentTuiCatalog, model: &str) -> AgentTuiState {
    let mut request = test_request(catalog);
    request.config_status = AgentTuiConfigStatus::new("fixture", model, "已认证");
    AgentTuiState::new_for_test(&request)
}

fn test_request(catalog: AgentTuiCatalog) -> AgentTuiLaunchRequest {
    AgentTuiLaunchRequest {
        cwd: axon_test_support::test_temp_root(),
        prompt: None,
        workspace_instruction_paths: Vec::new(),
        config_status: AgentTuiConfigStatus::new("fixture", "model", "已认证"),
        overrides: AgentTuiRunOverrides::default(),
        catalog,
        runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
    }
}

fn temp_file(name: &str) -> PathBuf {
    let path = axon_test_support::test_temp_root().join(format!("axon-tui-{name}"));
    fs::write(&path, b"fixture").expect("应能创建临时附件");
    path
}

#[path = "input_popup_mention_tests.rs"]
mod input_popup_mention_tests;
