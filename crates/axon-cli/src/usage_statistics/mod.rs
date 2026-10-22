use std::path::Path;

use crate::{CliRunResult, OutputFormat};

pub(crate) mod billing;
pub(crate) mod cli_commands;
pub(crate) mod http_adapter;
#[cfg(test)]
mod http_adapter_tests;
mod model;
pub(crate) mod observability_model;
pub(crate) mod observability_service;
mod observability_trend;
mod pricing_catalog;
mod projection;
pub(crate) mod realtime_aggregator;
mod render;
pub(crate) mod repository;
mod service;
pub(crate) mod sync;

pub use billing::{BillingEngine, RecalculateReport};
pub use cli_commands::{PricingCommand, UsageCommand};
pub(crate) use cli_commands::{run_pricing_cli, run_usage_cli};
pub use model::{UsagePeriod, UsageSourceFilter};
pub(crate) use projection::run_usage_rollup_once;
pub(crate) use repository::UsageRepository;
pub use repository::minute_rollup::MinuteRollupRow;
pub(crate) use sync::run_pricing_sync_once;
pub use sync::{PricingSyncService, SyncOutcome};

use axon_domain::{
    METRIC_LLM_PROVIDER_GENERATION_SPEED, METRIC_LLM_PROVIDER_REASONING_EFFORT,
    METRIC_LLM_PROVIDER_REQUEST_PATH, METRIC_LLM_PROVIDER_SPEED_MODE, METRIC_LLM_PROVIDER_TTFT_MS,
    METRIC_LLM_PROVIDER_UPSTREAM_MODEL,
};

/// 遥测指标 ID 与 usage_facts 列名的双向映射表 (SSOT §2.4)。
pub const TELEMETRY_METRIC_FACT_COLUMNS: &[(&str, &str)] = &[
    (METRIC_LLM_PROVIDER_UPSTREAM_MODEL, "upstream_model"),
    (METRIC_LLM_PROVIDER_REASONING_EFFORT, "reasoning_effort"),
    (METRIC_LLM_PROVIDER_REQUEST_PATH, "request_path"),
    (METRIC_LLM_PROVIDER_SPEED_MODE, "speed_mode"),
    (METRIC_LLM_PROVIDER_TTFT_MS, "ttft_ms"),
    (METRIC_LLM_PROVIDER_GENERATION_SPEED, "generation_speed_tps"),
];

/// 根据指标 ID 查询对应的 usage_facts 列名。
pub fn metric_to_fact_column(metric_id: &str) -> Option<&'static str> {
    TELEMETRY_METRIC_FACT_COLUMNS
        .iter()
        .find(|(m, _)| *m == metric_id)
        .map(|(_, c)| *c)
}

/// 根据 usage_facts 列名查询对应的指标 ID。
pub fn fact_column_to_metric(column_name: &str) -> Option<&'static str> {
    TELEMETRY_METRIC_FACT_COLUMNS
        .iter()
        .find(|(_, c)| *c == column_name)
        .map(|(m, _)| *m)
}

pub(crate) const USAGE_ROLLUP_CRON_SCRIPT: &str = "axon.internal.usage-rollup";
pub(crate) const USAGE_ROLLUP_CRON_JOB_ID: &str = "axon.system.usage-rollup";
pub(crate) const USAGE_ROLLUP_CRON_JOB_NAME: &str = "Usage statistics rollup";
pub(crate) const USAGE_ROLLUP_INTERVAL_MS: u64 = 60_000;
pub(crate) const USAGE_PROJECTOR_NAME: &str = "rollout-usage-v1";

/*
 * 模型费率自动同步的系统任务。
 *
 * 任务**不默认存在**：只有用户在设置里打开自动同步时才创建或启用，关闭时转为暂停。
 * 周期固定为一天：models.dev 的费率不会更频繁地变动，而同步本身要访问外网。
 */
pub(crate) const PRICING_SYNC_CRON_SCRIPT: &str = "axon.internal.pricing-sync";
pub(crate) const PRICING_SYNC_CRON_JOB_ID: &str = "axon.system.pricing-sync";
pub(crate) const PRICING_SYNC_CRON_JOB_NAME: &str = "Models.dev pricing sync";
pub(crate) const PRICING_SYNC_INTERVAL_MS: u64 = 86_400_000;

const USAGE_MAX_RECENT_LIMIT: usize = 200;
const USAGE_DEFAULT_RECENT_LIMIT: usize = 20;

pub(crate) fn run_usage(
    command: &UsageCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    run_usage_cli(command, format, config_path)
}

pub(crate) fn run_pricing(
    command: &PricingCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    run_pricing_cli(command, format, config_path)
}

pub(crate) fn cleanup_usage_data(config_path: &Path, dry_run: bool) -> Result<u64, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let usage_path = axon_root.join("usage").join("usage.sqlite3");
    if !usage_path.exists() {
        return Ok(0);
    }
    let mut repository = repository::UsageRepository::open(&axon_root)?;
    if !dry_run {
        let _ =
            repository::UsageRepository::ensure_auto_vacuum_incremental(repository.connection());
    }
    repository.cleanup_retention(0, dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_metrics_and_fact_columns_map_bidirectionally() {
        let expected_columns = [
            "upstream_model",
            "reasoning_effort",
            "request_path",
            "speed_mode",
            "ttft_ms",
            "generation_speed_tps",
        ];

        assert_eq!(
            TELEMETRY_METRIC_FACT_COLUMNS.len(),
            expected_columns.len(),
            "M1 新增遥测列数量与映射表必须一致"
        );

        for column in expected_columns {
            let metric_id = fact_column_to_metric(column)
                .unwrap_or_else(|| panic!("列名 {column} 必须在指标注册表中存在映射"));
            let mapped_back = metric_to_fact_column(metric_id)
                .unwrap_or_else(|| panic!("指标 ID {metric_id} 必须能反向映射到列名"));
            assert_eq!(mapped_back, column, "指标 ID 与列名必须严格双向一致");
        }
    }
}
