//! 可观测性 RESTful API 与 CLI 共享的数据模型 (DTO)。
//!
//! 覆盖 SSOT §5.1 ~ §5.6 六大端点契约、统一错误模型及查询参数。

use serde::{Deserialize, Serialize};

// ============================================================================
// 统一错误模型 (SSOT §5.0 / §2.3)
// ============================================================================

/// 统一错误响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservabilityErrorResponse {
    pub error: ObservabilityErrorBody,
}

impl ObservabilityErrorResponse {
    #[must_use]
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<serde_json::Value>,
    ) -> Self {
        Self {
            error: ObservabilityErrorBody {
                code: code.into(),
                message: message.into(),
                details,
            },
        }
    }

    #[must_use]
    pub fn invalid_argument(
        message: impl Into<String>,
        details: Option<serde_json::Value>,
    ) -> Self {
        Self::new("INVALID_ARGUMENT", message, details)
    }

    #[must_use]
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new("UNAUTHORIZED", message, None)
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("NOT_FOUND", message, None)
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("INTERNAL", message, None)
    }
}

/// 统一错误体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservabilityErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

// ============================================================================
// 抽稀信息披露 (SSOT §2.1)
// ============================================================================

/// 样本抽稀披露信息。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingDisclosure {
    /// 原始匹配样本总数。
    pub sampled_of: usize,
    /// 实际采纳样本数。
    pub sample_count: usize,
    /// 抽稀比率 (0.0 ~ 1.0)。
    pub sampling_ratio: f64,
}

// ============================================================================
// 延迟与效率诊断模型 (SSOT §5.4 / §2.1 - M4)
// ============================================================================

/// 诊断分位数结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsPercentiles {
    pub p50_ttft_ms: Option<u64>,
    pub p90_ttft_ms: Option<u64>,
    pub p95_ttft_ms: Option<u64>,
    pub p99_ttft_ms: Option<u64>,
    pub p50_latency_ms: Option<u64>,
    pub p90_latency_ms: Option<u64>,
    pub p95_latency_ms: Option<u64>,
    pub p99_latency_ms: Option<u64>,
}

/// 诊断散点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScatterPoint {
    pub ttft_ms: u64,
    pub total_duration_ms: u64,
    pub tokens: u64,
}

/// 模型单价效益项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEfficiencyItem {
    pub model: String,
    pub cost_per1m_tokens: f64,
}

/// 诊断报告响应 (GET /api/v1/observability/diagnostics)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub sample_count: usize,
    pub percentiles: DiagnosticsPercentiles,
    pub scatter_points: Vec<ScatterPoint>,
    pub model_efficiency: Vec<ModelEfficiencyItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingDisclosure>,
}

// ============================================================================
// 审计日志检索与导出模型 (SSOT §5.5 / §2.1 - M4)
// ============================================================================

/// 单条审计事件条目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogEvent {
    pub recorded_at_ms: u64,
    pub model: String,
    pub upstream_model: Option<String>,
    pub source: String,
    pub reasoning_effort: Option<String>,
    pub speed_mode: Option<String>,
    pub status: String,
    pub request_path: Option<String>,
    pub total_duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub generation_speed_tps: Option<f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_hit_rate_basis_points: u16,
    pub pricing_style: String,
    pub cost_micros: Option<u64>,
}

/// 审计日志检索报告 (GET /api/v1/observability/audit/logs)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogsReport {
    pub total_count: u64,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub events: Vec<AuditLogEvent>,
}

// ============================================================================
// 大盘概览模型 (SSOT §5.1)
// ============================================================================

/// 大盘汇总数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewSummary {
    pub request_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub success_rate_basis_points: u16,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub cache_hit_rate_basis_points: u16,
    pub current_rpm: f64,
    pub current_tpm: u64,
    pub total_cost_micros: u64,
    pub cost_formatted: String,
}

/// Sparklines 迷你走势线。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sparklines {
    pub requests: Vec<u64>,
    pub tokens: Vec<u64>,
    pub costs: Vec<f64>,
}

/// 活动矩阵热力图分块。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatmapData {
    pub metric: String,
    pub buckets: Vec<axon_domain::HeatmapBucket>,
}

/// 健康度时序分块。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthTimelineData {
    pub buckets: Vec<axon_domain::HealthTimelineBucket>,
}

/// 大盘概览响应 (GET /api/v1/observability/overview)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewReport {
    pub summary: OverviewSummary,
    pub sparklines: Sparklines,
    pub activity_heatmap: HeatmapData,
    pub health_timeline: HealthTimelineData,
}

// ============================================================================
// 用量分布与排行模型 (SSOT §5.3)
// ============================================================================

/// 用量分布分项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionItem {
    pub key: String,
    pub tokens: u64,
    pub percentage: f64,
    pub request_count: u64,
    pub cost_micros: u64,
    pub cache_hit_rate_basis_points: u16,
    pub tpm: u64,
    pub rpm: f64,
}

/// 用量分布时间序列点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionTimeSeriesPoint {
    pub timestamp_ms: u64,
    pub uncached_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub request_count: u64,
    pub cost_micros: u64,
}

/// 用量分布报告响应 (GET /api/v1/observability/distribution)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionReport {
    pub total_tokens: u64,
    pub total_cost_micros: u64,
    pub items: Vec<DistributionItem>,
    pub time_series: Vec<DistributionTimeSeriesPoint>,
}

// ============================================================================
// 模型价格管理请求与响应 (SSOT §5.6)
// ============================================================================

/// 价格规则新建/更新请求 Payload。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePricingRulePayload {
    pub model_pattern: String,
    pub pricing_style: String,
    pub input_price_per_m_micros: u64,
    pub output_price_per_m_micros: u64,
    pub cache_read_price_per_m_micros: u64,
    pub cache_write_price_per_m_micros: u64,
    #[serde(default = "default_multiplier")]
    pub multiplier_basis_points: u32,
    #[serde(default)]
    pub priority: i32,
}

fn default_multiplier() -> u32 {
    10000
}

/// 价格同步响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPricingResponse {
    /// 本次实际写入的规则条数。
    pub synced_rules_count: usize,
    /// 费率来源。
    pub source: String,
    /// 本次同步的时间戳。
    pub updated_at_ms: u64,
    /// 本次结论：`success`、`debounced`、`rate_limited` 或 `fallback`。
    ///
    /// 「写入 0 条」有四种完全不同的原因，只给条数会让调用方无从下手：
    /// 该等一会儿、该明天再说、还是该去查网络。
    pub outcome: String,
    /// 结论的补充说明：防抖剩余时间或上游失败原因；成功时为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 历史重算请求 Payload。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecalculatePricingPayload {
    #[serde(default)]
    pub from_ms: Option<u64>,
}

/// 历史重算响应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecalculatePricingResponse {
    pub recalculated_count: u64,
    pub affected_rollup_buckets: usize,
    pub elapsed_ms: u64,
}

// ============================================================================
// 费率自动同步 (SSOT §5.6)
// ============================================================================

/// 自动同步的目标状态请求体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPricingAutoSyncPayload {
    /// true 创建或启用每日同步任务，false 转为暂停。
    pub enabled: bool,
}

/// 自动同步的状态报告 (GET/POST /api/v1/observability/pricing/auto-sync)。
///
/// `enabled` 由系统 Cron 任务的存在与状态得出，不是界面自己记的一个开关：
/// 任务被别处暂停时，这里如实报 false。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingAutoSyncReport {
    /// 自动同步当前是否生效（任务存在且处于 active）。
    pub enabled: bool,
    /// 系统任务标识。
    pub job_id: String,
    /// 任务是否已经创建过；false 表示从未开启过。
    pub job_exists: bool,
    /// 任务状态：active / paused / removed / missing。
    pub status: String,
    /// 计划表达式。
    pub schedule: String,
    /// 上次运行结束时间。
    pub last_run_at_ms: Option<u64>,
    /// 下次运行时间。
    pub next_run_at_ms: Option<u64>,
    /// 上次运行是否成功。
    pub last_run_success: Option<bool>,
    /// 上次运行的失败原因；成功时为 None。
    pub last_run_summary: Option<String>,
}
