//! HeaderBar 产品级状态聚合模型。

use std::time::Duration;

use crate::r#loop::agent_tui::{
    AgentTuiConfigStatus, AgentTuiModelCatalogItem, AgentTuiPlanProgress, AgentTuiRunOverrides,
    AgentTuiTokenUsage, display_units,
    runtime_identity::{display_model, effective_model},
};

use crate::surface::display_width::DisplayWidthText;
use crate::surface::replay_snapshot::ReplaySnapshotHeaderState;

const DEFAULT_REASONING_EFFORT: &str = "none";
const CWD_VISIBLE_CHARS: usize = 24;
const MODEL_VISIBLE_CHARS: usize = 20;
const HEADER_PRIMARY_SEPARATOR: &str = " · ";
const CONTEXT_WARNING_PERCENT: u64 = 70;
const CONTEXT_CRITICAL_PERCENT: u64 = 85;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum HeaderBarStatus {
    #[default]
    Ready,
    Working,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HeaderBarMetrics {
    pub(crate) token_usage: AgentTuiTokenUsage,
    pub(crate) plan_progress: Option<AgentTuiPlanProgress>,
    pub(crate) lsp_active_count: usize,
    pub(crate) elapsed: Option<Duration>,
    pub(crate) first_token_avg_ms: Option<u64>,
    pub(crate) generation_tps_x10: Option<u64>,
    pub(crate) status: HeaderBarStatus,
    pub(crate) cwd: String,
    pub(crate) session_id: Option<String>,
    pub(crate) continue_last: bool,
    pub(crate) replay_snapshot: Option<ReplaySnapshotHeaderState>,
    pub(crate) spinner: &'static str,
    pub(crate) animation_tick: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderBarViewModel {
    pub(crate) primary: Vec<HeaderBarSegment>,
    pub(crate) hints: Vec<HeaderBarSegment>,
}

impl HeaderBarViewModel {
    pub(crate) fn from_sources(
        config_status: &AgentTuiConfigStatus,
        overrides: &AgentTuiRunOverrides,
        metrics: &HeaderBarMetrics,
        models: &[AgentTuiModelCatalogItem],
    ) -> Self {
        let mut primary = Vec::new();
        if let Some(indicator) = status_indicator(metrics) {
            primary.push(
                HeaderBarSegment::new(HeaderBarSegmentKind::Status, indicator)
                    .with_breath_highlight(metrics.animation_tick),
            );
        }
        primary.extend([
            HeaderBarSegment::new(
                HeaderBarSegmentKind::Model,
                short_metric(
                    "模型",
                    compact_text(
                        &display_model(config_status, overrides),
                        MODEL_VISIBLE_CHARS,
                    ),
                ),
            ),
            HeaderBarSegment::new(
                HeaderBarSegmentKind::Cwd,
                compact_text(&metrics.cwd, CWD_VISIBLE_CHARS),
            ),
            HeaderBarSegment::new(
                HeaderBarSegmentKind::Reasoning,
                short_metric("思考", reasoning_effort(overrides, models, config_status)),
            ),
        ]);
        if let Some(context) = context_usage_value(metrics, config_status, overrides, models) {
            primary.push(
                HeaderBarSegment::new(HeaderBarSegmentKind::Context, context.text)
                    .with_tone(context.tone),
            );
        }
        if let Some(first_token_avg_ms) = metrics.first_token_avg_ms {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::FirstToken,
                short_metric(
                    "首字",
                    display_units::format_duration_millis_compact(first_token_avg_ms),
                ),
            ));
        }
        if let Some(generation_tps_x10) = metrics.generation_tps_x10 {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::GenerationRate,
                short_metric("速率", format_tokens_per_second_x10(generation_tps_x10)),
            ));
        }
        if let Some(elapsed) = elapsed_value(metrics.elapsed) {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::Elapsed,
                elapsed,
            ));
        }
        if let Some(progress) = plan_progress_value(metrics.plan_progress) {
            primary.push(HeaderBarSegment::new(HeaderBarSegmentKind::Plan, progress));
        }
        if metrics.lsp_active_count > 0 {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::Lsp,
                short_metric("LSP", metrics.lsp_active_count.to_string()),
            ));
        }

        let mut hints = Vec::new();
        if let Some(replay) = &metrics.replay_snapshot
            && replay.is_loading()
        {
            hints.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::ReplaySnapshot,
                replay.loaded_cells_label(),
            ));
            hints.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::ReplayInput,
                replay.input_label().to_owned(),
            ));
        }
        Self { primary, hints }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderBarSegment {
    pub(crate) kind: HeaderBarSegmentKind,
    pub(crate) text: String,
    pub(crate) priority: HeaderBarPriority,
    pub(crate) breath_highlight: bool,
    pub(crate) tone: HeaderBarSegmentTone,
}

impl HeaderBarSegment {
    fn new(kind: HeaderBarSegmentKind, text: String) -> Self {
        Self {
            kind,
            text,
            priority: kind.priority(),
            breath_highlight: false,
            tone: HeaderBarSegmentTone::Default,
        }
    }

    fn with_breath_highlight(mut self, animation_tick: u64) -> Self {
        self.breath_highlight = crate::surface::animation::axon_logo_breath_highlight(
            usize::try_from(animation_tick).unwrap_or(usize::MAX),
        );
        self
    }

    fn with_tone(mut self, tone: HeaderBarSegmentTone) -> Self {
        self.tone = tone;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderBarSegmentKind {
    Status,
    Model,
    Cwd,
    Context,
    ReplaySnapshot,
    ReplayInput,
    Elapsed,
    FirstToken,
    GenerationRate,
    Reasoning,
    Plan,
    Lsp,
}

impl HeaderBarSegmentKind {
    const fn priority(self) -> HeaderBarPriority {
        match self {
            Self::Status | Self::Model | Self::Cwd => HeaderBarPriority::P0,
            Self::Context
            | Self::Elapsed
            | Self::FirstToken
            | Self::GenerationRate
            | Self::Reasoning
            | Self::Plan => HeaderBarPriority::P2,
            Self::ReplaySnapshot | Self::ReplayInput | Self::Lsp => HeaderBarPriority::P3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HeaderBarPriority {
    P0,
    P1,
    P2,
    P3,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum HeaderBarSegmentTone {
    #[default]
    Default,
    Warning,
    Critical,
}

pub(crate) struct HeaderViewModel<'a> {
    pub(crate) config_status: &'a AgentTuiConfigStatus,
    pub(crate) overrides: &'a AgentTuiRunOverrides,
    pub(crate) metrics: HeaderBarMetrics,
    pub(crate) models: &'a [AgentTuiModelCatalogItem],
}

impl HeaderViewModel<'_> {
    pub(crate) fn product_view_model(&self) -> HeaderBarViewModel {
        HeaderBarViewModel::from_sources(
            self.config_status,
            self.overrides,
            &self.metrics,
            self.models,
        )
    }
}

pub(crate) struct AppShellFrameViewModel<'a> {
    pub(crate) header: HeaderViewModel<'a>,
}

pub(crate) fn primary_separator() -> &'static str {
    HEADER_PRIMARY_SEPARATOR
}

fn status_indicator(metrics: &HeaderBarMetrics) -> Option<String> {
    if metrics
        .replay_snapshot
        .is_some_and(|replay| replay.status_label().is_some() && replay.is_loading())
    {
        return non_empty_spinner(metrics);
    }
    match metrics.status {
        HeaderBarStatus::Working => non_empty_spinner(metrics),
        HeaderBarStatus::Ready => None,
    }
}

fn non_empty_spinner(metrics: &HeaderBarMetrics) -> Option<String> {
    let spinner = metrics.spinner.trim();
    (!spinner.is_empty()).then(|| spinner.to_owned())
}

fn reasoning_effort(
    overrides: &AgentTuiRunOverrides,
    models: &[AgentTuiModelCatalogItem],
    config_status: &AgentTuiConfigStatus,
) -> String {
    overrides
        .reasoning_effort
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            current_model_catalog_item(config_status, overrides, models)
                .and_then(|model| model.reasoning_effort.as_deref())
        })
        .unwrap_or(DEFAULT_REASONING_EFFORT)
        .to_owned()
}

fn context_usage_value(
    metrics: &HeaderBarMetrics,
    config_status: &AgentTuiConfigStatus,
    overrides: &AgentTuiRunOverrides,
    models: &[AgentTuiModelCatalogItem],
) -> Option<ContextUsageValue> {
    let usage = metrics.token_usage;
    let used = usage.current_context_tokens();
    if used == 0 {
        return None;
    }
    let context_window = current_model_catalog_item(config_status, overrides, models)
        .and_then(|model| model.context_window)
        .filter(|window| *window > 0);
    let mut value = match context_window {
        Some(window) => format!(
            "上下文 {}/{}",
            AgentTuiTokenUsage::render_token_count(used),
            AgentTuiTokenUsage::render_token_count(window)
        ),
        None => short_metric("上下文", AgentTuiTokenUsage::render_token_count(used)),
    };
    if usage.cache_read_input_tokens > 0 {
        value.push_str(" 缓存 ");
        value.push_str(&AgentTuiTokenUsage::render_token_count(
            usage.cache_read_input_tokens,
        ));
    }
    value.push_str(" 命中 ");
    value.push_str(&axon_display::format_basis_points_percent(
        usage.cache_hit_rate_basis_points(),
    ));
    if usage.cache_write_input_tokens > 0 {
        value.push_str(" 写入 ");
        value.push_str(&AgentTuiTokenUsage::render_token_count(
            usage.cache_write_input_tokens,
        ));
    }
    Some(ContextUsageValue {
        text: value,
        tone: context_window.map_or(HeaderBarSegmentTone::Default, |window| {
            context_pressure_tone(used, window)
        }),
    })
}

struct ContextUsageValue {
    text: String,
    tone: HeaderBarSegmentTone,
}

fn context_pressure_tone(used: u64, window: u64) -> HeaderBarSegmentTone {
    let percent = used.saturating_mul(100) / window.max(1);
    if percent >= CONTEXT_CRITICAL_PERCENT {
        HeaderBarSegmentTone::Critical
    } else if percent >= CONTEXT_WARNING_PERCENT {
        HeaderBarSegmentTone::Warning
    } else {
        HeaderBarSegmentTone::Default
    }
}

fn current_model_catalog_item<'a>(
    config_status: &AgentTuiConfigStatus,
    overrides: &AgentTuiRunOverrides,
    models: &'a [AgentTuiModelCatalogItem],
) -> Option<&'a AgentTuiModelCatalogItem> {
    let model = effective_model(config_status, overrides);
    let display = display_model(config_status, overrides);
    models
        .iter()
        .find(|candidate| candidate.label == display)
        .or_else(|| models.iter().find(|candidate| candidate.matches(&model)))
}

fn elapsed_value(elapsed: Option<Duration>) -> Option<String> {
    elapsed.map(|value| {
        short_metric(
            "耗时",
            display_units::format_duration_millis_compact(
                u64::try_from(value.as_millis()).unwrap_or(u64::MAX),
            ),
        )
    })
}

fn format_tokens_per_second_x10(value: u64) -> String {
    if value.is_multiple_of(10) {
        format!("{} tok/s", value / 10)
    } else {
        format!("{}.{value_mod} tok/s", value / 10, value_mod = value % 10)
    }
}

fn plan_progress_value(progress: Option<AgentTuiPlanProgress>) -> Option<String> {
    let progress = progress?;
    (progress.total > 0)
        .then(|| short_metric("计划", format!("{}/{}", progress.completed, progress.total)))
}

fn compact_text(text: &str, max_chars: usize) -> String {
    DisplayWidthText::ellipsize(text, max_chars)
}

fn short_metric(label: &str, value: impl Into<String>) -> String {
    format!("{label} {}", value.into())
}

#[cfg(test)]
mod tests {
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

        let model = HeaderBarViewModel::from_sources(
            &config,
            &AgentTuiRunOverrides::default(),
            &metrics,
            &[],
        );

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
    fn header_bar_view_model_surfaces_context_cache_without_prompt_cache_side_effects() {
        let config = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
        let metrics = HeaderBarMetrics {
            token_usage: AgentTuiTokenUsage::new(1_000, 2_000, 300, 400, 0)
                .with_current_context_tokens(2_300),
            ..HeaderBarMetrics::default()
        };

        let model = HeaderBarViewModel::from_sources(
            &config,
            &AgentTuiRunOverrides::default(),
            &metrics,
            &[],
        );

        let context = model
            .primary
            .iter()
            .find(|segment| segment.kind == HeaderBarSegmentKind::Context)
            .expect("HeaderBar 应显示上下文/cache 状态");
        assert!(context.text.contains("上下文 2.3K"));
        assert!(context.text.contains("缓存 2K"));
        assert!(context.text.contains("命中 60.60%"));
        assert!(context.text.contains("写入 300"));
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

        let model = HeaderBarViewModel::from_sources(
            &config,
            &AgentTuiRunOverrides::default(),
            &metrics,
            &[],
        );

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

        let model = HeaderBarViewModel::from_sources(
            &config,
            &AgentTuiRunOverrides::default(),
            &metrics,
            &[],
        );

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

        let model = HeaderBarViewModel::from_sources(
            &config,
            &AgentTuiRunOverrides::default(),
            &metrics,
            &[],
        );

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
}
