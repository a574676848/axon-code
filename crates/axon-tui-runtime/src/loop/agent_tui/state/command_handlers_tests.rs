//! Agent TUI slash 命令行为测试。

use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

use super::AgentTuiState;
use super::command_handlers::AgentTuiCommandOutcome;
use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiConfigDiagnostic, AgentTuiConfigDiagnosticStatus,
    AgentTuiConfigStatus, AgentTuiLaunchRequest, AgentTuiModelCatalogItem, AgentTuiRunOverrides,
    AgentTuiTokenUsage,
};
use crate::surface::cell::RenderCellData;

#[test]
fn status_command_renders_codex_like_session_diagnostics() {
    let mut state = test_state();
    state.header.workspace_instruction_paths =
        vec!["AGENTS.md".to_owned(), "crates/AGENTS.md".to_owned()];
    state.header.config_status = AgentTuiConfigStatus::new("local", "local/gpt-5.5", "已认证")
        .with_diagnostics(vec![AgentTuiConfigDiagnostic::new(
            "provider.local",
            AgentTuiConfigDiagnosticStatus::Ok,
            "http://192.0.2.10:5434/v1 · credential=present · ok",
            None,
        )]);
    state.header.overrides.reasoning_effort = Some("medium".to_owned());
    state.header.overrides.session = Some("019eead4-7469-7130-ad0b-47d4ec1a87a4".to_owned());
    state
        .header
        .catalog
        .models
        .push(AgentTuiModelCatalogItem::new_with_context_window(
            "gpt-5.5",
            "local/gpt-5.5",
            "local",
            Some(258_000),
        ));
    state.header.session_usage.add_assign(
        AgentTuiTokenUsage::new(457_000, 12_000, 12_000, 24_200, 0)
            .with_current_context_tokens(235_000),
    );

    assert!(matches!(
        state.handle_command("/status"),
        AgentTuiCommandOutcome::Handled
    ));

    let Some(RenderCellData::Command(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("/status 应生成结构化命令状态块");
    };
    assert_eq!(
        status_summary(cell, "status.version"),
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        status_summary(cell, "status.repository"),
        "https://github.com/a574676848/axon-code"
    );
    assert_eq!(
        status_summary(cell, "status.model"),
        "gpt-5.5（推理 medium）"
    );
    assert_eq!(
        status_summary(cell, "status.provider"),
        "local - http://192.0.2.10:5434/v1"
    );
    assert!(
        status_summary(cell, "status.directory")
            .contains(std::env::temp_dir().to_string_lossy().as_ref())
    );
    assert_eq!(
        status_summary(cell, "status.agents"),
        "已加载 2 个：crates/AGENTS.md"
    );
    assert_eq!(
        status_summary(cell, "status.session"),
        "019eead4-7469-7130-ad0b-47d4ec1a87a4"
    );
    assert_eq!(
        status_summary(cell, "status.token_usage"),
        "505K 总计（457K 输入 + 24.2K 输出 + 12K 缓存写入 + 12K 缓存读取）"
    );
    assert_eq!(
        status_summary(cell, "status.context_window"),
        "剩余 8%（已用 235K / 258K）"
    );
}

#[test]
fn model_command_updates_header_bar_state_without_transcript_noise() {
    let mut state = test_state();
    state.header.catalog.models.push(
        crate::r#loop::agent_tui::AgentTuiModelCatalogItem::new_with_runtime_defaults(
            "gpt-5.4", "gpt-5.4", "cpa", None, None,
        ),
    );
    let transcript_len_before = state.cells.transcript.len();

    assert!(matches!(
        state.handle_command("/model gpt-5.4"),
        AgentTuiCommandOutcome::Handled
    ));

    assert_eq!(state.header.config_status.model, "gpt-5.4");
    assert_eq!(state.header.overrides.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(state.cells.transcript.len(), transcript_len_before);
}

#[test]
fn model_command_accepts_provider_selector_and_persists_model_label() {
    let mut state = test_state();
    state.header.catalog.models.push(
        crate::r#loop::agent_tui::AgentTuiModelCatalogItem::new_with_runtime_defaults(
            "gpt-5.4",
            "cpa/gpt-5.4",
            "cpa",
            None,
            None,
        ),
    );

    assert!(matches!(
        state.handle_command("/model cpa/gpt-5.4"),
        AgentTuiCommandOutcome::Handled
    ));

    assert_eq!(state.header.config_status.model, "gpt-5.4");
    assert_eq!(state.header.overrides.model.as_deref(), Some("gpt-5.4"));
}

#[test]
fn model_command_prefers_alias_over_duplicate_selector() {
    let mut state = test_state();
    state.header.catalog.models = vec![
        AgentTuiModelCatalogItem::new("alias-a", "alias-b", "provider"),
        AgentTuiModelCatalogItem::new("alias-b", "shared-upstream-id", "provider"),
    ];

    assert!(matches!(
        state.handle_command("/model alias-b"),
        AgentTuiCommandOutcome::Handled
    ));

    assert_eq!(state.header.config_status.model, "alias-b");
    assert_eq!(state.header.overrides.model.as_deref(), Some("alias-b"));
}

#[test]
fn think_command_accepts_max_and_rejects_non_api_ultra() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("/think max"),
        AgentTuiCommandOutcome::Handled
    ));
    assert_eq!(
        state.header.overrides.reasoning_effort.as_deref(),
        Some("max")
    );

    assert!(matches!(
        state.handle_command("/think ultra"),
        AgentTuiCommandOutcome::Handled
    ));
    assert_eq!(
        state.header.overrides.reasoning_effort.as_deref(),
        Some("max")
    );
    let Some(RenderCellData::Error(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("非 API effort 应生成用法错误");
    };
    assert!(
        cell.message
            .contains("none|minimal|low|medium|high|xhigh|max")
    );
}

#[test]
fn skills_list_command_opens_skill_reference_popup() {
    let mut state = test_state();
    state.header.catalog.skills = vec!["imagegen".to_owned(), "openai-docs".to_owned()];
    state.replace_draft("/skills list".to_owned());

    assert!(matches!(
        state.handle_command("/skills list"),
        AgentTuiCommandOutcome::Handled
    ));

    assert_eq!(state.composer.draft.text(), "$");
    assert!(state.composer_popup_open());
}

#[test]
fn skills_command_opens_interactive_menu() {
    let mut state = test_state();
    state.header.catalog.skills = vec!["review".to_owned()];

    assert!(matches!(
        state.handle_command("/skills"),
        AgentTuiCommandOutcome::Handled
    ));

    assert!(state.bottom_view_open());
    assert!(state.composer.draft.text().is_empty());
}

#[test]
fn skills_toggle_command_opens_toggle_view() {
    let mut state = test_state();
    state.header.catalog.skills = vec!["review".to_owned()];

    assert!(matches!(
        state.handle_command("/skills toggle"),
        AgentTuiCommandOutcome::Handled
    ));

    assert!(state.bottom_view_open());
}

#[test]
fn skills_toggle_close_records_persistable_skill_state() {
    let mut state = test_state();
    state.header.catalog.skills = vec!["review".to_owned()];

    assert!(matches!(
        state.handle_command("/skills toggle"),
        AgentTuiCommandOutcome::Handled
    ));
    assert!(
        state
            .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Char(' ')))
            .is_none()
    );
    assert!(
        state
            .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Enter))
            .is_none()
    );

    assert!(state.header.catalog.disabled_skills.contains("review"));
    let changes = state.take_settings_changes();
    assert!(changes.iter().any(|change| {
        change
            .skill_states
            .iter()
            .any(|skill| skill.name == "review" && !skill.enabled)
    }));
}

#[test]
fn skills_toggle_close_without_changes_preserves_disabled_skills_without_settings_change() {
    let mut state = test_state();
    state.header.catalog.skills = vec!["review".to_owned(), "docs".to_owned()];
    state
        .header
        .catalog
        .disabled_skills
        .insert("docs".to_owned());

    assert!(matches!(
        state.handle_command("/skills toggle"),
        AgentTuiCommandOutcome::Handled
    ));
    assert!(
        state
            .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Enter))
            .is_none()
    );

    assert!(state.header.catalog.disabled_skills.contains("docs"));
    assert!(!state.header.catalog.disabled_skills.contains("review"));
    assert!(state.take_settings_changes().is_empty());
}

#[test]
fn skills_filter_argument_is_management_usage_error() {
    let mut state = test_state();
    state.header.catalog.skills = vec!["imagegen".to_owned(), "openai-docs".to_owned()];

    assert!(matches!(
        state.handle_command("/skills docs"),
        AgentTuiCommandOutcome::Handled
    ));

    let Some(RenderCellData::Error(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("/skills filter 应生成用法错误");
    };
    assert_eq!(cell.title, "用法");
    assert_eq!(cell.message, "/skills [list|toggle]");
}

#[test]
fn bare_skills_and_mcp_text_are_not_local_management_commands() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("skills list"),
        AgentTuiCommandOutcome::NotCommand
    ));
    assert!(matches!(
        state.handle_command("mcp list"),
        AgentTuiCommandOutcome::NotCommand
    ));
}

fn test_state() -> AgentTuiState {
    AgentTuiState::new_for_test(&AgentTuiLaunchRequest {
        cwd: axon_test_support::test_temp_root(),
        prompt: None,
        workspace_instruction_paths: Vec::new(),
        config_status: AgentTuiConfigStatus::new("fixture", "model", "已认证"),
        overrides: AgentTuiRunOverrides::default(),
        catalog: AgentTuiCatalog::default(),
        runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
    })
}

fn status_summary(cell: &crate::surface::cell::RenderCommandCell, kind: &str) -> String {
    cell.items
        .iter()
        .find(|item| item.kind == kind)
        .unwrap_or_else(|| panic!("/status 应包含 {kind} 条目"))
        .summary
        .clone()
}
