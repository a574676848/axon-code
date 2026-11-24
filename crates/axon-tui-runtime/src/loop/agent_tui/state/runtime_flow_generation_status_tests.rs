//! 生成状态与命令 lifecycle transcript 投射测试。

use super::*;

#[test]
fn streaming_started_renders_provider_model_reply_status() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("等待模型"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::streaming_started_event(
            "openai", "gpt-5", 1,
        ),
    );

    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        1
    );
    assert!(active_generation_summary_contains(
        &state,
        "openai/gpt-5 生成回复中"
    ));
    assert!(state.is_working());

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "生成",
                "Generation",
                "生成回复",
                "generation",
                None,
                None,
                None,
            ),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        1
    );
    assert!(lifecycle_cell_has_phase(
        &state,
        RenderCellKind::GENERATION_STATUS,
        "streaming"
    ));
    assert!(active_generation_summary_contains(
        &state,
        "openai/gpt-5 生成回复中"
    ));
}

#[test]
fn streaming_started_uses_model_label_instead_of_runtime_selector() {
    let mut catalog = AgentTuiCatalog::default();
    catalog.models.push(AgentTuiModelCatalogItem::new(
        "Other DeepSeek",
        "deepseek-v4-pro",
        "other-provider",
    ));
    catalog.models.push(AgentTuiModelCatalogItem::new(
        "DeepSeek V4 Pro",
        "deepseek-v4-pro",
        "sm-wenwen",
    ));
    let mut state =
        crate::r#loop::agent_tui::state::AgentTuiState::new_for_test(&AgentTuiLaunchRequest {
            cwd: axon_test_support::test_temp_root(),
            prompt: None,
            workspace_instruction_paths: Vec::new(),
            config_status: AgentTuiConfigStatus::new("sm-wenwen", "DeepSeek V4 Pro", "已认证"),
            overrides: AgentTuiRunOverrides {
                model: Some("DeepSeek V4 Pro".to_owned()),
                ..AgentTuiRunOverrides::default()
            },
            catalog,
            runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
        });
    state.start_runtime_submit(&test_submission("等待模型"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "生成",
                "StreamingStarted",
                "sm-wenwen/DeepSeek V4 Pro 生成回复中",
                "streaming",
                Some("DeepSeek V4 Pro".to_owned()),
                None,
                Some(1),
            ),
        ),
    );

    assert!(active_generation_summary_contains(
        &state,
        "sm-wenwen/DeepSeek V4 Pro 生成回复中"
    ));
    assert!(!active_generation_summary_contains(
        &state,
        "sm-wenwen/deepseek-v4-pro"
    ));
    assert!(!active_generation_summary_contains(
        &state,
        "other-provider/Other DeepSeek"
    ));
}

#[test]
fn assistant_delta_fallback_uses_configured_provider_model_reply_status() {
    let mut catalog = AgentTuiCatalog::default();
    catalog
        .models
        .push(AgentTuiModelCatalogItem::new("mingc", "mingc", "local"));
    let mut state =
        crate::r#loop::agent_tui::state::AgentTuiState::new_for_test(&AgentTuiLaunchRequest {
            cwd: axon_test_support::test_temp_root(),
            prompt: None,
            workspace_instruction_paths: Vec::new(),
            config_status: AgentTuiConfigStatus::new("local", "mingc", "已认证"),
            overrides: AgentTuiRunOverrides {
                model: Some("mingc".to_owned()),
                ..AgentTuiRunOverrides::default()
            },
            catalog,
            runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
        });
    state.start_runtime_submit(&test_submission("等待模型"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "首个增量".to_owned(),
        ),
    );
    assert!(state.tick(std::time::Instant::now() + std::time::Duration::from_secs(1)));
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        0
    );
    assert_eq!(typed_message_source(&state).as_deref(), Some("首个增量"));
}

#[test]
fn assistant_snapshot_clears_generation_status_after_first_body() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("等待模型"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::streaming_started_event(
            "openai", "gpt-5", 1,
        ),
    );
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        1
    );

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_message_event_with_id(
            "assistant:snapshot",
            "provider-message:snapshot",
            "完整回复".to_owned(),
            false,
        ),
    );

    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        0
    );
    assert_eq!(typed_message_source(&state).as_deref(), Some("完整回复"));
}

#[test]
fn live_lifecycle_items_are_capped_and_keep_latest_entries() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("大量生命周期状态"));

    for index in 0..60 {
        state.apply_runtime_event(
            crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
                AgentTuiLifecycleStatusView::new(
                    "生成",
                    format!("GenerationStage{index}"),
                    format!("阶段 {index}"),
                    "generation",
                    None,
                    None,
                    None,
                ),
            ),
        );
    }

    let items = state
        .active_transcript_lines()
        .into_iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::GenerationStatus(cell)) => Some(&cell.items),
            _ => None,
        })
        .expect("live generation status cell should exist");

    assert_eq!(items.len(), 50);
    assert!(!items.iter().any(|item| item.kind == "GenerationStage0"));
    assert!(items.iter().any(|item| item.kind == "GenerationStage59"));
}

#[test]
fn command_lifecycle_status_projects_dedicated_command_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("切换模型"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new("设置", "Model", "gpt-5", "command", None, None, None),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::COMMAND), 1);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert!(
        state
            .cells
            .transcript
            .iter()
            .any(|line| matches!(line.data, Some(RenderCellData::Command(_))))
    );
}

#[test]
fn command_lifecycle_cell_does_not_overwrite_status_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("切换配置"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new("设置", "Model", "gpt-5", "command", None, None, None),
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "运行",
                "CompactionStarted",
                "压缩上下文",
                "notice",
                None,
                None,
                None,
            ),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::COMMAND), 1);
    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
}
