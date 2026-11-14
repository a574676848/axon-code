//! HeaderBar 产品状态聚合模型的行为测试。

use super::*;

use crate::r#loop::agent_tui::{AgentTuiConfigStatus, AgentTuiRunOverrides};

#[test]
fn header_bar_view_model_orders_product_status_fields_by_priority() {
    let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
    let metrics = HeaderBarMetrics {
        status: HeaderBarStatus::Working,
        spinner: "⚡ Axon",
        cwd: "E:/zbg/axon-code".to_owned(),
        session_id: Some("019eead4-7469-7130-ad0b-47d4ec1a87a4".to_owned()),
        continue_last: false,
        plan_progress: Some(AgentTuiPlanProgress {
            completed: 1,
            total: 3,
        }),
        lsp_active_count: 2,
        ..HeaderBarMetrics::default()
    };

    let model =
        HeaderBarViewModel::from_sources(&config, &AgentTuiRunOverrides::default(), &metrics, &[]);

    assert_eq!(model.primary[0].text, "⚡ Axon");
    assert!(model.primary[0].breath_highlight);
    assert!(
        !model
            .primary
            .iter()
            .any(|segment| segment.text.contains("生成回复中"))
    );
    assert!(
        !model
            .primary
            .iter()
            .any(|segment| segment.text.contains("会话:"))
    );
    assert!(!model.hints.iter().any(|segment| segment.text == "Esc 中断"));
}

#[test]
fn header_bar_view_model_surfaces_session_totals_before_context() {
    let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
    let metrics = HeaderBarMetrics {
        token_usage: AgentTuiTokenUsage::new(1_000, 2_000, 300, 400, 0)
            .with_current_context_tokens(2_300),
        session_tokens: 6_260_000,
        session_elapsed_ms: 11_520_000,
        session_cache_hit_basis_points: Some(6_060),
        ..HeaderBarMetrics::default()
    };

    let model =
        HeaderBarViewModel::from_sources(&config, &AgentTuiRunOverrides::default(), &metrics, &[]);

    let kinds: Vec<_> = model.primary.iter().map(|segment| segment.kind).collect();
    let elapsed = kinds
        .iter()
        .position(|kind| *kind == HeaderBarSegmentKind::Elapsed)
        .expect("HeaderBar 应显示会话累计耗时");
    let session = kinds
        .iter()
        .position(|kind| *kind == HeaderBarSegmentKind::SessionTokens)
        .expect("HeaderBar 应显示会话累计用量");
    let cache_hit = kinds
        .iter()
        .position(|kind| *kind == HeaderBarSegmentKind::CacheHit)
        .expect("HeaderBar 应显示会话累计命中率");
    let context = kinds
        .iter()
        .position(|kind| *kind == HeaderBarSegmentKind::Context)
        .expect("HeaderBar 应显示上下文");
    assert_eq!(session, elapsed + 1, "耗时排在累计用量之前");
    assert_eq!(cache_hit, session + 1, "命中率紧跟累计用量");
    assert!(cache_hit < context, "会话读数都在上下文段之前");
    assert_eq!(model.primary[elapsed].text, "耗时 3h 12m 00s");
    assert_eq!(model.primary[session].text, "累计 6.26M");
    assert_eq!(model.primary[cache_hit].text, "命中率 60.60%");
}

#[test]
fn header_bar_view_model_omits_session_totals_until_any_usage() {
    let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
    let model = HeaderBarViewModel::from_sources(
        &config,
        &AgentTuiRunOverrides::default(),
        &HeaderBarMetrics::default(),
        &[],
    );

    assert!(
        !model.primary.iter().any(|segment| matches!(
            segment.kind,
            HeaderBarSegmentKind::SessionTokens | HeaderBarSegmentKind::Elapsed
        )),
        "一条会话都还没跑过时不渲染 `累计 0` 与 `耗时 0`"
    );
}
#[test]
fn header_bar_view_model_surfaces_context_cache_without_prompt_cache_side_effects() {
    let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
    let metrics = HeaderBarMetrics {
        token_usage: AgentTuiTokenUsage::new(1_000, 2_000, 300, 400, 0)
            .with_current_context_tokens(2_300),
        ..HeaderBarMetrics::default()
    };

    let model =
        HeaderBarViewModel::from_sources(&config, &AgentTuiRunOverrides::default(), &metrics, &[]);

    let context = model
        .primary
        .iter()
        .find(|segment| segment.kind == HeaderBarSegmentKind::Context)
        .expect("HeaderBar 应显示上下文/cache 状态");
    assert!(context.text.contains("上下文 2.3K"));
    assert!(context.text.contains("缓存 2K"));
    assert!(context.text.contains("写入 300"));
    assert!(
        !context.text.contains("命中"),
        "命中率是整条会话的读数，不挤在上下文段里"
    );
}

#[test]
fn header_bar_view_model_follows_session_model_window() {
    let config = AgentTuiConfigStatus::new("sm-wenwen-chat", "deepseek-v4.1-flash", "已认证");
    let models = vec![
        AgentTuiModelCatalogItem::new_with_context_window(
            "deepseek-v4.1-flash",
            "deepseek-v4.1-flash-1017",
            "sm-wenwen-chat",
            Some(1_000_000),
        ),
        AgentTuiModelCatalogItem::new_with_context_window(
            "gpt-6-astra",
            "gpt-6-astra",
            "codex-onetoken",
            Some(800_000),
        ),
    ];
    let metrics = HeaderBarMetrics {
        token_usage: AgentTuiTokenUsage::new(1_000, 2_000, 0, 0, 0)
            .with_current_context_tokens(400_000),
        session_run_model: Some(SessionRunModel {
            provider: "codex-onetoken".to_owned(),
            model: "gpt-6-astra".to_owned(),
        }),
        ..HeaderBarMetrics::default()
    };

    let view = HeaderBarViewModel::from_sources(
        &config,
        &AgentTuiRunOverrides::default(),
        &metrics,
        &models,
    );

    let model_segment = view
        .primary
        .iter()
        .find(|segment| segment.kind == HeaderBarSegmentKind::Model)
        .expect("HeaderBar 应显示模型段");
    assert!(
        model_segment.text.contains("gpt-6-astra"),
        "模型段应显示会话真实模型：{}",
        model_segment.text
    );
    let context = view
        .primary
        .iter()
        .find(|segment| segment.kind == HeaderBarSegmentKind::Context)
        .expect("HeaderBar 应显示上下文段");
    assert!(
        context.text.contains("800K"),
        "上下文窗口必须跟着会话模型，而不是配置默认的 1M：{}",
        context.text
    );
}

#[test]
fn header_bar_view_model_keeps_explicit_model_pick_over_session_model() {
    let config = AgentTuiConfigStatus::new("sm-wenwen-chat", "deepseek-v4.1-flash", "已认证");
    let models = vec![
        AgentTuiModelCatalogItem::new_with_context_window(
            "deepseek-v4.1-flash",
            "deepseek-v4.1-flash-1017",
            "sm-wenwen-chat",
            Some(1_000_000),
        ),
        AgentTuiModelCatalogItem::new_with_context_window(
            "gpt-6-astra",
            "gpt-6-astra",
            "codex-onetoken",
            Some(800_000),
        ),
    ];
    let overrides = AgentTuiRunOverrides {
        model: Some("deepseek-v4.1-flash".to_owned()),
        ..AgentTuiRunOverrides::default()
    };
    let metrics = HeaderBarMetrics {
        token_usage: AgentTuiTokenUsage::new(1_000, 2_000, 0, 0, 0)
            .with_current_context_tokens(400_000),
        session_run_model: Some(SessionRunModel {
            provider: "codex-onetoken".to_owned(),
            model: "gpt-6-astra".to_owned(),
        }),
        model_explicit: true,
        ..HeaderBarMetrics::default()
    };

    let view = HeaderBarViewModel::from_sources(&config, &overrides, &metrics, &models);

    let context = view
        .primary
        .iter()
        .find(|segment| segment.kind == HeaderBarSegmentKind::Context)
        .expect("HeaderBar 应显示上下文段");
    assert!(
        context.text.contains("1M"),
        "显式选择优先于会话事实：{}",
        context.text
    );
}

#[test]
fn context_pressure_uses_stable_warning_and_critical_thresholds() {
    assert_eq!(
        context_pressure_tone(699, 1_000),
        HeaderBarSegmentTone::Default
    );
    assert_eq!(
        context_pressure_tone(700, 1_000),
        HeaderBarSegmentTone::Warning
    );
    assert_eq!(
        context_pressure_tone(850, 1_000),
        HeaderBarSegmentTone::Critical
    );
}

#[test]
fn header_bar_view_model_projects_resume_loading_without_blocking_input() {
    let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
    let metrics = HeaderBarMetrics {
        spinner: "⚡ Axon",
        animation_tick: 2,
        cwd: "E:/zbg/axon-code".to_owned(),
        replay_snapshot: Some(ReplaySnapshotHeaderState::loading(7, true)),
        ..HeaderBarMetrics::default()
    };

    let model =
        HeaderBarViewModel::from_sources(&config, &AgentTuiRunOverrides::default(), &metrics, &[]);

    assert_eq!(model.primary[0].text, "⚡ Axon");
    assert!(!model.primary[0].breath_highlight);
    assert!(
        model
            .hints
            .iter()
            .any(|segment| segment.text == "7 cells 已加载")
    );
    assert!(model.hints.iter().any(|segment| segment.text == "输入可用"));
}

#[test]
fn header_bar_view_model_projects_completed_resume_snapshot_state() {
    let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
    let metrics = HeaderBarMetrics {
        cwd: "E:/zbg/axon-code".to_owned(),
        replay_snapshot: Some(ReplaySnapshotHeaderState::completed(12, true)),
        ..HeaderBarMetrics::default()
    };

    let model =
        HeaderBarViewModel::from_sources(&config, &AgentTuiRunOverrides::default(), &metrics, &[]);

    assert!(
        !model
            .primary
            .iter()
            .any(|segment| segment.text.contains("快照:"))
    );
    assert!(!model.hints.iter().any(|segment| segment.text == "输入可用"));
}

#[test]
fn header_bar_view_model_renders_generation_performance_after_context() {
    let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
    let metrics = HeaderBarMetrics {
        token_usage: AgentTuiTokenUsage::new(10, 0, 0, 40, 0),
        first_token_avg_ms: Some(8_000),
        generation_tps_x10: Some(400),
        plan_progress: Some(AgentTuiPlanProgress {
            completed: 1,
            total: 3,
        }),
        lsp_active_count: 2,
        ..HeaderBarMetrics::default()
    };

    let model =
        HeaderBarViewModel::from_sources(&config, &AgentTuiRunOverrides::default(), &metrics, &[]);

    let metrics = model
        .primary
        .iter()
        .filter(|segment| {
            matches!(
                segment.kind,
                HeaderBarSegmentKind::FirstToken
                    | HeaderBarSegmentKind::GenerationRate
                    | HeaderBarSegmentKind::Plan
                    | HeaderBarSegmentKind::Lsp
            )
        })
        .map(|segment| (segment.kind, segment.text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        metrics,
        [
            (HeaderBarSegmentKind::FirstToken, "首字 8s"),
            (HeaderBarSegmentKind::GenerationRate, "速率 40 tok/s"),
            (HeaderBarSegmentKind::Plan, "计划 1/3"),
            (HeaderBarSegmentKind::Lsp, "LSP 2"),
        ]
    );
}
