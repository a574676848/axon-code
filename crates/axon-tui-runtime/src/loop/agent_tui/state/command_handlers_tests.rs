//! Agent TUI slash 命令行为测试。

use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

use super::AgentTuiState;
use super::command_handlers::AgentTuiCommandOutcome;
use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiCatalogLoadSnapshot, AgentTuiConfigDiagnostic,
    AgentTuiConfigDiagnosticStatus, AgentTuiConfigProjection, AgentTuiConfigStatus,
    AgentTuiLaunchRequest, AgentTuiModelCatalogItem, AgentTuiRunOverrides, AgentTuiTokenUsage,
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
    state
        .header
        .session_usage
        .add_assign(AgentTuiTokenUsage::new(457_000, 12_000, 12_000, 24_200, 0));
    // 上下文占用只有一份来源：会话级快照。/status 与头部读的是同一个读数。
    state.header.context_snapshot =
        crate::AgentTuiContextSnapshot::new(Some(235_000), Some(258_000), None, None);

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
    // 显式模型选择必须带回执标志，宿主据此把它作为本轮显式模型下传。
    let changes = state.take_settings_changes();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].model_explicit);
    assert_eq!(changes[0].model.as_deref(), Some("gpt-5.4"));
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
    // 推理强度变更会带着当前模型一起写回配置，但它不是模型选择。
    let changes = state.take_settings_changes();
    assert!(changes.iter().all(|change| !change.model_explicit));

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

fn session_item(
    session_id: &str,
    archived: bool,
) -> crate::r#loop::agent_tui::AgentTuiSessionCatalogItem {
    crate::r#loop::agent_tui::AgentTuiSessionCatalogItem::new(
        session_id,
        Some(format!("{session_id} 标题")),
        "openai",
        "gpt-5.5",
        1,
    )
    .with_archived(archived)
}

fn sessions_view(
    state: &AgentTuiState,
) -> Option<&crate::r#loop::agent_tui::bottom_view::SessionsView> {
    match state.bottom_view() {
        Some(crate::r#loop::agent_tui::bottom_view::BottomView::Sessions(view)) => Some(view),
        _ => None,
    }
}

#[test]
fn sessions_command_opens_popup_with_unarchived_sessions() {
    let mut state = test_state();
    state.header.catalog.sessions = vec![
        session_item("session-active", false),
        session_item("session-archived", true),
    ];

    assert!(matches!(
        state.handle_command("/sessions"),
        AgentTuiCommandOutcome::Handled
    ));

    let view = sessions_view(&state).expect("/sessions 应打开会话弹窗");
    assert_eq!(view.items().len(), 1);
    assert_eq!(view.selected_session_id(), Some("session-active"));
    assert!(state.composer.draft.text().is_empty());
}

#[test]
fn sessions_all_command_lists_archived_sessions() {
    let mut state = test_state();
    state.header.catalog.sessions = vec![
        session_item("session-active", false),
        session_item("session-archived", true),
    ];

    assert!(matches!(
        state.handle_command("/sessions --all"),
        AgentTuiCommandOutcome::Handled
    ));

    let view = sessions_view(&state).expect("/sessions --all 应打开会话弹窗");
    assert_eq!(view.items().len(), 2);
    assert!(view.items().iter().any(|item| item.archived));
}

#[test]
fn sessions_command_points_to_all_when_catalog_only_has_archived() {
    let mut state = test_state();
    state.header.catalog.sessions = vec![session_item("session-archived", true)];

    assert!(matches!(
        state.handle_command("/sessions"),
        AgentTuiCommandOutcome::Handled
    ));

    assert!(!state.bottom_view_open(), "空候选不应打开空弹窗");
    let Some(RenderCellData::Error(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("归档会话被过滤时应给出可执行提示");
    };
    assert!(cell.message.contains("--all"));
}

#[test]
fn sessions_command_rejects_unknown_argument() {
    let mut state = test_state();
    state.header.catalog.sessions = vec![session_item("session-active", false)];

    assert!(matches!(
        state.handle_command("/sessions --everything"),
        AgentTuiCommandOutcome::Handled
    ));

    assert!(!state.bottom_view_open());
    let Some(RenderCellData::Error(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("非法参数应生成用法错误");
    };
    assert!(cell.message.contains("/sessions [--all]"));
}

#[test]
fn new_and_reset_commands_split_session_ownership() {
    let mut state = test_state();
    state.header.overrides.session = Some("session-1".to_owned());
    state.header.session_summary.record_session(
        "session-1".to_owned(),
        None,
        Some("历史会话".to_owned()),
    );

    assert!(matches!(
        state.handle_command("/new"),
        AgentTuiCommandOutcome::Handled
    ));
    assert_eq!(state.header.overrides.session, None);
    assert_eq!(state.header.session_summary.session_id, None);

    let mut state = test_state();
    state.header.overrides.session = Some("session-1".to_owned());
    state.header.session_summary.record_session(
        "session-1".to_owned(),
        None,
        Some("历史会话".to_owned()),
    );

    assert!(matches!(
        state.handle_command("/reset"),
        AgentTuiCommandOutcome::Handled
    ));
    assert_eq!(
        state.header.overrides.session.as_deref(),
        Some("session-1"),
        "/reset 必须保留 session id"
    );
    assert_eq!(
        state.header.session_summary.session_id.as_deref(),
        Some("session-1"),
        "/reset 必须保留会话历史归属"
    );
}

#[test]
fn sessions_popup_enter_binds_selected_session() {
    let mut state = test_state();
    state.header.catalog.sessions = vec![
        session_item("session-a", false),
        session_item("session-b", false),
    ];
    assert!(matches!(
        state.handle_command("/sessions"),
        AgentTuiCommandOutcome::Handled
    ));

    assert!(
        state
            .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Down))
            .is_none()
    );
    assert!(
        state
            .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Enter))
            .is_none()
    );

    assert_eq!(
        state.header.overrides.session.as_deref(),
        Some("session-b"),
        "Enter 必须绑定当前选中会话"
    );
    assert_eq!(
        state.take_pending_session_switch().as_deref(),
        Some("session-b")
    );
}

#[test]
fn think_command_without_argument_reports_the_current_effort() {
    let mut state = test_state();
    state.header.overrides.reasoning_effort = Some("xhigh".to_owned());

    assert!(matches!(
        state.handle_command("/think"),
        AgentTuiCommandOutcome::Handled
    ));

    let Some(RenderCellData::Command(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("/think 不带参数应给出当前强度读数");
    };
    assert_eq!(status_summary(cell, "当前"), "xhigh");
    assert!(
        status_summary(cell, "可选").contains("none|minimal|low|medium|high|xhigh|max"),
        "读数里要带上这一轮真正能用的候选值"
    );
    assert!(
        !state.header.effort_explicit,
        "只是读一次当前值，不能当成显式选择"
    );
}

#[test]
fn config_reload_refreshes_models_and_defaults_without_overriding_explicit_picks() {
    let mut state = test_state();
    state.apply_catalog_load_snapshot(config_reload(&["gpt-5.4"], "gpt-5.4", "medium"));

    assert_eq!(state.header.catalog.models.len(), 1);
    assert_eq!(state.header.overrides.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(
        state.header.overrides.reasoning_effort.as_deref(),
        Some("medium")
    );

    // 用户显式改了强度：配置复读不得把它改回去。
    assert!(matches!(
        state.handle_command("/think low"),
        AgentTuiCommandOutcome::Handled
    ));
    state.apply_catalog_load_snapshot(config_reload(
        &["claude-sonnet-4-6"],
        "claude-sonnet-4-6",
        "high",
    ));

    assert_eq!(
        state.header.overrides.reasoning_effort.as_deref(),
        Some("low"),
        "显式选过的强度不该被配置默认值覆盖"
    );
    assert_eq!(
        state.header.overrides.model.as_deref(),
        Some("claude-sonnet-4-6"),
        "没显式选过模型时，模型跟着配置走"
    );

    // 显式选过的模型从配置里消失（删除条目或停用供应商）：退回配置默认值。
    assert!(matches!(
        state.handle_command("/model claude-sonnet-4-6"),
        AgentTuiCommandOutcome::Handled
    ));
    let _ = state.take_settings_changes();
    state.apply_catalog_load_snapshot(config_reload(&["gpt-5.4"], "gpt-5.4", "high"));

    assert_eq!(
        state.header.overrides.model.as_deref(),
        Some("gpt-5.4"),
        "显式选过的模型已经不可用时必须退回配置默认值"
    );
    /*
     * 放弃显式选择这件事必须同步给宿主：宿主手里还有一份「随轮下发的显式模型」，
     * 只改界面不改它，这一轮仍会点名那个已经删掉的名字。同步的方式是「清空」，
     * 不是「钉到新的默认值」：钉住之后配置默认值再变一次，界面与下发就会再次分叉。
     */
    let changes = state.take_settings_changes();
    assert_eq!(changes.len(), 1, "只在放弃显式选择时补一条设置变更");
    assert!(changes[0].model_explicit);
    assert_eq!(
        changes[0].model, None,
        "宿主收到的是清空，不是把旧默认值钉成新的显式选择"
    );

    // 默认值此后改写：界面继续跟随，且不再产生设置变更（宿主已回到「无显式模型」）。
    state.apply_catalog_load_snapshot(config_reload(&["gpt-5.4"], "gpt-5.4", "high"));
    assert!(state.take_settings_changes().is_empty());
    state.apply_catalog_load_snapshot(config_reload(&["glm-5", "gpt-5.4"], "glm-5", "high"));
    assert_eq!(
        state.header.overrides.model.as_deref(),
        Some("glm-5"),
        "清空之后默认值改写仍要跟上"
    );
    assert!(state.take_settings_changes().is_empty());
}

#[test]
fn config_reload_without_any_usable_model_still_clears_the_explicit_pick() {
    let mut state = test_state();
    state.apply_catalog_load_snapshot(config_reload(&["gpt-5.4"], "gpt-5.4", "medium"));
    assert!(matches!(
        state.handle_command("/model gpt-5.4"),
        AgentTuiCommandOutcome::Handled
    ));
    let _ = state.take_settings_changes();

    // 配置里一个可用模型都不剩：没有默认值可以回填，但显式选择照样要作废。
    state.apply_catalog_load_snapshot(AgentTuiCatalogLoadSnapshot {
        config: Some(AgentTuiConfigProjection {
            models: Vec::new(),
            providers: Vec::new(),
            default_model: None,
            default_reasoning_effort: Some("medium".to_owned()),
        }),
        ..AgentTuiCatalogLoadSnapshot::default()
    });

    assert!(state.header.catalog.models.is_empty());
    let changes = state.take_settings_changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].model, None,
        "没有默认值可回填时同样推清空，而不是把已经失效的名字钉给宿主"
    );
}

#[test]
fn config_reload_without_projection_keeps_the_existing_models() {
    let mut state = test_state();
    state.apply_catalog_load_snapshot(config_reload(&["gpt-5.4"], "gpt-5.4", "medium"));

    state.apply_catalog_load_snapshot(AgentTuiCatalogLoadSnapshot::default());

    assert_eq!(
        state.header.catalog.models.len(),
        1,
        "不含配置事实的快照（例如 MCP 复读失败）不能把模型清单清空"
    );
    assert_eq!(state.header.overrides.model.as_deref(), Some("gpt-5.4"));
}

fn config_reload(
    models: &[&str],
    default_model: &str,
    default_reasoning_effort: &str,
) -> AgentTuiCatalogLoadSnapshot {
    AgentTuiCatalogLoadSnapshot {
        config: Some(AgentTuiConfigProjection {
            models: models
                .iter()
                .map(|name| AgentTuiModelCatalogItem::new(*name, *name, "provider"))
                .collect(),
            providers: vec!["provider".to_owned()],
            default_model: Some(default_model.to_owned()),
            default_reasoning_effort: Some(default_reasoning_effort.to_owned()),
        }),
        ..AgentTuiCatalogLoadSnapshot::default()
    }
}

fn status_summary(cell: &crate::surface::cell::RenderCommandCell, kind: &str) -> String {
    cell.items
        .iter()
        .find(|item| item.kind == kind)
        .unwrap_or_else(|| panic!("/status 应包含 {kind} 条目"))
        .summary
        .clone()
}
