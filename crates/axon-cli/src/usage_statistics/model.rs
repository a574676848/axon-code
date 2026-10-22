use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// 使用统计查询周期。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, ValueEnum)]
pub enum UsagePeriod {
    /// 当地时区当天。
    #[default]
    #[serde(rename = "today")]
    Today,
    /// 包含当天在内的最近 7 个自然日。
    #[value(name = "7d")]
    #[serde(rename = "7d")]
    SevenDays,
    /// 包含当天在内的最近 30 个自然日。
    #[value(name = "30d")]
    #[serde(rename = "30d")]
    ThirtyDays,
    /// 全部已聚合数据。
    #[serde(rename = "all")]
    All,
}

impl UsagePeriod {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Today => "今天",
            Self::SevenDays => "最近 7 天",
            Self::ThirtyDays => "最近 30 天",
            Self::All => "全部",
        }
    }
}

/// 使用统计来源过滤器。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum UsageSourceFilter {
    /// 不限制来源。
    #[default]
    All,
    /// Axon 顶层 Coding turn。
    AxonCli,
    /// 子代理 Coding turn。
    Subagent,
}

impl UsageSourceFilter {
    pub(super) const fn as_query(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::AxonCli => Some(UsageSource::AxonCli.as_str()),
            Self::Subagent => Some(UsageSource::Subagent.as_str()),
        }
    }
}

/// 统计事实来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageSource {
    AxonCli,
    Subagent,
}

impl UsageSource {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::AxonCli => "axon_cli",
            Self::Subagent => "subagent",
        }
    }
}

/// Coding turn 终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageStatus {
    Completed,
    Failed,
    Interrupted,
    Active,
}

impl UsageStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Active => "active",
        }
    }
}

/// 从 rollout 投影出的使用事实。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct UsageFact {
    pub(super) session_id: String,
    pub(super) assistant_part_id: String,
    pub(super) recorded_at_ms: u64,
    pub(super) hour_bucket_ms: u64,
    pub(super) day_bucket_ms: u64,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) source: UsageSource,
    pub(super) status: UsageStatus,
    pub(super) total_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) cache_write_input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) provider_calls: u64,
    pub(super) provider_duration_ms: u64,
    pub(super) usage_observed: bool,
    pub(super) cost_micros: Option<u64>,
    pub(super) pricing_rule_id: Option<String>,
    pub(super) recalculated_at_ms: Option<u64>,
    pub(super) source_fingerprint: String,
    pub(super) upstream_model: Option<String>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) request_path: Option<String>,
    pub(super) speed_mode: Option<String>,
    pub(super) ttft_ms: Option<u64>,
    pub(super) generation_speed_tps: Option<f64>,
}

/// 聚合表的分组结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UsageRollupRow {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) source: String,
    pub(super) status: String,
    pub(super) request_count: u64,
    pub(super) provider_call_count: u64,
    pub(super) total_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) cache_write_input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) provider_duration_ms: u64,
    pub(super) measured_latency_count: u64,
    pub(super) priced_cost_micros: u64,
    pub(super) unpriced_request_count: u64,
}

/// 最近请求事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageRequestView {
    pub(super) recorded_at_ms: u64,
    pub(super) session_id: String,
    pub(super) assistant_part_id: String,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) source: String,
    pub(super) status: String,
    pub(super) total_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) cache_write_input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) provider_calls: u64,
    pub(super) provider_duration_ms: u64,
    pub(super) cost_micros: Option<u64>,
}

/// 聚合指标。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageMetrics {
    pub(super) request_count: u64,
    pub(super) provider_call_count: u64,
    pub(super) successful_request_count: u64,
    pub(super) total_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) cache_write_input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) cache_hit_rate_basis_points: u16,
    pub(super) success_rate_basis_points: u16,
    pub(super) average_provider_latency_ms: u64,
    pub(super) priced_cost_micros: u64,
    pub(super) unpriced_request_count: u64,
    pub(super) cost_status: &'static str,
}

/// Provider 聚合视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProviderUsageView {
    pub(super) provider: String,
    #[serde(flatten)]
    pub(super) metrics: UsageMetrics,
}

/// 模型聚合视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModelUsageView {
    pub(super) model: String,
    #[serde(flatten)]
    pub(super) metrics: UsageMetrics,
}

/// 小时趋势视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageTrendPoint {
    pub(super) bucket_start_ms: u64,
    #[serde(flatten)]
    pub(super) metrics: UsageMetrics,
}

/// 投影状态。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageProjectionState {
    pub(super) source_watermark_rowid: u64,
    pub(super) last_success_at_ms: Option<u64>,
    pub(super) projected_fact_count: u64,
}

/// 查询过滤条件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageFilters {
    pub(super) provider: Option<String>,
    pub(super) model: Option<String>,
    pub(super) source: UsageSourceFilter,
    pub(super) session_id: Option<String>,
    pub(super) run_id: Option<String>,
}

/// CLI 与后续看板共用的查询报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageDashboardReport {
    pub(super) schema_version: u16,
    pub(super) generated_at_ms: u64,
    pub(super) period: UsagePeriod,
    pub(super) period_start_ms: u64,
    pub(super) filters: UsageFilters,
    pub(super) summary: UsageMetrics,
    pub(super) providers: Vec<ProviderUsageView>,
    pub(super) models: Vec<ModelUsageView>,
    pub(super) trend: Vec<UsageTrendPoint>,
    pub(super) recent_requests: Vec<UsageRequestView>,
    pub(super) projection: UsageProjectionView,
}

/// 面向查询端的投影新鲜度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageProjectionView {
    pub(super) last_success_at_ms: Option<u64>,
    pub(super) source_watermark_rowid: u64,
    pub(super) projected_fact_count: u64,
    pub(super) stale: bool,
}

/// Usage projector 执行报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageProjectionReport {
    pub projected_records: u64,
    pub changed_facts: u64,
    pub source_watermark_rowid: u64,
    pub completed_at_ms: u64,
}

/// Usage 投影只关心的 rollout 事件类型；其他类型保持前向兼容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(super) enum RolloutUsageEventKind {
    ProviderCompleted,
    Completed,
    Failed,
    Interrupted,
    #[serde(other)]
    Other,
}

/// Usage 投影读取的最小事件字段。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RolloutUsageEvent {
    pub(super) kind: RolloutUsageEventKind,
    #[serde(default)]
    pub(super) duration_ms: Option<u64>,
}

/// Rollout usage 的最小反序列化合同。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RolloutUsageRecord {
    pub(super) session_id: String,
    #[serde(default)]
    pub(super) parent_session_id: Option<String>,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) assistant_part_id: String,
    #[serde(default)]
    pub(super) usage: Option<axon_agent_runtime::TokenUsageTelemetry>,
    #[serde(default)]
    pub(super) turn_events: Vec<RolloutUsageEvent>,
    #[serde(default)]
    pub(super) upstream_model: Option<String>,
    #[serde(default)]
    pub(super) reasoning_effort: Option<String>,
    #[serde(default)]
    pub(super) request_path: Option<String>,
    #[serde(default)]
    pub(super) speed_mode: Option<String>,
    #[serde(default)]
    pub(super) ttft_ms: Option<u64>,
    #[serde(default)]
    pub(super) generation_speed_tps: Option<f64>,
}
