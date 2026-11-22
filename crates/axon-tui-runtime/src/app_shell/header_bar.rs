//! HeaderBar 产品级状态聚合模型。

use crate::app_shell::context_snapshot::AgentTuiContextSnapshot;
use crate::r#loop::agent_tui::{
    AgentTuiConfigStatus, AgentTuiModelCatalogItem, AgentTuiPlanProgress, AgentTuiRunOverrides,
    AgentTuiTokenUsage, display_units,
    runtime_identity::{SessionRunModel, display_model, effective_model},
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
    /// 本轮最近一次请求的用量构成（缓存读写的唯一来源）。
    ///
    /// 上下文占用不从这里读：占用与换窗读数统一读 `context_snapshot`，两处因此不会各说一套。
    pub(crate) token_usage: AgentTuiTokenUsage,
    /// 会话级上下文快照读数：头部占用段与换窗段的唯一来源。
    pub(crate) context_snapshot: AgentTuiContextSnapshot,
    /// 整条会话的累计 token 消耗，排在上下文段之前。
    pub(crate) session_tokens: u64,
    /// 整条会话的累计墙钟耗时（毫秒），紧跟累计 token。
    pub(crate) session_elapsed_ms: u64,
    /// 整条会话的累计缓存命中率（basis points）；还没有输入观测时为空。
    pub(crate) session_cache_hit_basis_points: Option<u16>,
    pub(crate) plan_progress: Option<AgentTuiPlanProgress>,
    pub(crate) lsp_active_count: usize,
    pub(crate) first_token_avg_ms: Option<u64>,
    pub(crate) generation_tps_x10: Option<u64>,
    pub(crate) status: HeaderBarStatus,
    pub(crate) cwd: String,
    pub(crate) session_id: Option<String>,
    pub(crate) continue_last: bool,
    pub(crate) replay_snapshot: Option<ReplaySnapshotHeaderState>,
    pub(crate) spinner: &'static str,
    pub(crate) animation_tick: u64,
    /// 本会话最近一轮真实使用的模型；头部据此显示真实模型与其上下文窗口。
    pub(crate) session_run_model: Option<SessionRunModel>,
    /// 用户在本进程里显式选过模型：显式选择优先于会话事实。
    pub(crate) model_explicit: bool,
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
        // 生效模型只解析一次：模型名、思考强度与上下文窗口必须来自同一份事实。
        let current = current_model_catalog_item(config_status, overrides, metrics, models);
        primary.extend([
            HeaderBarSegment::new(
                HeaderBarSegmentKind::Model,
                short_metric(
                    "模型",
                    compact_text(
                        &model_label(config_status, overrides, current),
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
                short_metric("思考", reasoning_effort(overrides, current)),
            ),
        ]);
        // 会话累计：耗时 → 累计 → 命中率，三段回答同一件事「这条会话花了多少」。
        // 耗时排在累计之前，命中率紧跟其后：先看花了多少时间与多少 token，再看其中省下多少。
        if metrics.session_elapsed_ms > 0 {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::Elapsed,
                short_metric(
                    "耗时",
                    display_units::format_duration_millis_compact(metrics.session_elapsed_ms),
                ),
            ));
        }
        if metrics.session_tokens > 0 {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::SessionTokens,
                short_metric(
                    "累计",
                    AgentTuiTokenUsage::render_token_count(metrics.session_tokens),
                ),
            ));
        }
        if let Some(basis_points) = metrics.session_cache_hit_basis_points {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::CacheHit,
                short_metric(
                    "命中率",
                    axon_display::format_basis_points_percent(basis_points),
                ),
            ));
        }
        if let Some(context) = context_usage_value(metrics, current) {
            primary.push(
                HeaderBarSegment::new(HeaderBarSegmentKind::Context, context.text)
                    .with_tone(context.tone),
            );
        }
        // 换窗读数与占用段同源：都取自同一份会话级快照，未知时不显示。
        if let Some(rollover) = metrics.context_snapshot.rollover_text() {
            primary.push(HeaderBarSegment::new(
                HeaderBarSegmentKind::Rollover,
                rollover,
            ));
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
    SessionTokens,
    CacheHit,
    Context,
    Rollover,
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
            Self::SessionTokens
            | Self::CacheHit
            | Self::Context
            | Self::Rollover
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
    current: Option<&AgentTuiModelCatalogItem>,
) -> String {
    overrides
        .reasoning_effort
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| current.and_then(|model| model.reasoning_effort.as_deref()))
        .unwrap_or(DEFAULT_REASONING_EFFORT)
        .to_owned()
}

/// 头部上下文占用段。
///
/// 占用与窗口只从会话级快照读：读不到就整段不显示，既不显示 `0`，也不拿用量帧的
/// 归一化总量或上一次读数顶替。缓存读写是本轮最近一次请求的构成，仍取用量帧。
fn context_usage_value(
    metrics: &HeaderBarMetrics,
    current: Option<&AgentTuiModelCatalogItem>,
) -> Option<ContextUsageValue> {
    let snapshot = &metrics.context_snapshot;
    let model_window = current.and_then(|model| model.context_window);
    let usage = metrics.token_usage;
    let text = snapshot.context_text(
        model_window,
        usage.cache_read_input_tokens,
        usage.cache_write_input_tokens,
    )?;
    let tone = snapshot
        .context_pressure_percent(model_window)
        .map_or(HeaderBarSegmentTone::Default, |percent| {
            context_pressure_tone(percent)
        });
    Some(ContextUsageValue { text, tone })
}

struct ContextUsageValue {
    text: String,
    tone: HeaderBarSegmentTone,
}

fn context_pressure_tone(percent: u64) -> HeaderBarSegmentTone {
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
    metrics: &HeaderBarMetrics,
    models: &'a [AgentTuiModelCatalogItem],
) -> Option<&'a AgentTuiModelCatalogItem> {
    // 用户显式选过模型：头部显示这次选择（与运行侧显式模型一致）。
    if !metrics.model_explicit {
        // 否则优先会话真实使用的模型：运行侧恢复会话时就是按它选模型，
        // 头部读同一份事实，模型名与上下文窗口才不会各说一套。
        if let Some(run) = metrics.session_run_model.as_ref()
            && let Some(item) = models
                .iter()
                .find(|candidate| {
                    candidate.provider == run.provider && candidate.matches(&run.model)
                })
                .or_else(|| {
                    models
                        .iter()
                        .find(|candidate| candidate.matches(&run.model))
                })
        {
            return Some(item);
        }
    }
    let model = effective_model(config_status, overrides);
    let display = display_model(config_status, overrides);
    models
        .iter()
        .find(|candidate| candidate.label == display)
        .or_else(|| models.iter().find(|candidate| candidate.matches(&model)))
}

/// 头部显示的模型名：命中目录时用目录展示名，否则回退配置口径。
fn model_label(
    config_status: &AgentTuiConfigStatus,
    overrides: &AgentTuiRunOverrides,
    current: Option<&AgentTuiModelCatalogItem>,
) -> String {
    current.map_or_else(
        || display_model(config_status, overrides),
        |model| model.label.clone(),
    )
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
#[path = "header_bar_tests.rs"]
mod tests;
