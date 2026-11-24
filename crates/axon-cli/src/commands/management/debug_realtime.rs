//! `axon debug realtime-metrics`：查询实时可观测性内存滑动窗口与分钟级持久化聚合（M3 只读观测面）。

use std::fmt::Write as _;
use std::path::Path;

use axon_domain::RealtimeWindowReport;
use serde::Serialize;

use crate::management_commands::{cli_error, json_result, resolved_config_path};
use crate::tool_orchestrator::now_ms;
use crate::usage_statistics::{MinuteRollupRow, UsageRepository, realtime_aggregator};
use crate::{CliRunResult, DebugRealtimeMetricsCommand, OutputFormat};

/// 实时可观测性诊断报告。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeMetricsDebugReport {
    /// 诊断报告生成时间戳（毫秒）。
    pub generated_at_ms: u64,
    /// 观察窗口时长（分钟）。
    pub window_minutes: u32,
    /// 内存滑动窗口快照（对齐 SSOT §5.2 响应模型）。
    pub window_report: RealtimeWindowReport,
    /// 最近 N 条分钟级持久化聚合桶摘要。
    pub recent_minute_rollups: Vec<MinuteRollupRow>,
}

pub(crate) fn run_realtime_metrics_debug(
    command: &DebugRealtimeMetricsCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let now = now_ms();
    let window_report = realtime_aggregator::sliding_window_snapshot(now, command.window);
    let resolved_path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let axon_root = match resolved_path.parent() {
        Some(parent) => parent,
        None => Path::new("."),
    };

    let minute_rollups = match UsageRepository::open(axon_root) {
        Ok(mut repo) => {
            if let Some(rebuild_ts) = command.rebuild_bucket {
                match repo.rebuild_minute_bucket(rebuild_ts) {
                    Ok(()) => {}
                    Err(error) => return cli_error(format!("重建分钟桶失败：{error}")),
                }
            }
            let window_ms = u64::from(command.window).saturating_mul(60_000);
            let since_ms = now.saturating_sub(window_ms);
            repo.query_minute_rollups(since_ms, command.minute_limit)
                .unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };

    let report = RealtimeMetricsDebugReport {
        generated_at_ms: now,
        window_minutes: command.window,
        window_report,
        recent_minute_rollups: minute_rollups,
    };

    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: realtime_metrics_debug_text(&report),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&report),
    }
}

fn realtime_metrics_debug_text(report: &RealtimeMetricsDebugReport) -> String {
    let mut out = format!(
        "Axon debug realtime-metrics (window: {})\n",
        report.window_report.window
    );
    let _ = writeln!(
        out,
        "- requests: {}",
        report.window_report.window_request_count
    );
    let _ = writeln!(
        out,
        "- total_tokens: {}",
        report.window_report.window_total_tokens
    );
    let _ = writeln!(
        out,
        "- current_rpm: {}",
        report.window_report.metrics.current_rpm
    );
    let _ = writeln!(
        out,
        "- current_tpm: {}",
        report.window_report.metrics.current_tpm
    );
    let _ = writeln!(
        out,
        "- average_rpm: {:.2}",
        report.window_report.metrics.average_rpm
    );
    let _ = writeln!(
        out,
        "- average_tpm: {:.2}",
        report.window_report.metrics.average_tpm
    );
    let _ = writeln!(
        out,
        "- cache_hit_rate_basis_points: {}",
        report.window_report.window_cache_hit_rate_basis_points
    );
    let _ = writeln!(
        out,
        "- cost_micros: {}",
        report.window_report.window_cost_micros
    );
    let _ = writeln!(
        out,
        "- ttft_current_ms: {}ms",
        report.window_report.ttft_current_ms
    );
    let _ = writeln!(
        out,
        "- ttft_average_ms: {}ms",
        report.window_report.ttft_average_ms
    );
    let _ = writeln!(
        out,
        "- latency_current_ms: {}ms",
        report.window_report.latency_current_ms
    );
    let _ = writeln!(
        out,
        "- latency_average_ms: {}ms",
        report.window_report.latency_average_ms
    );
    let _ = writeln!(
        out,
        "- recent_minute_buckets: {} rows",
        report.recent_minute_rollups.len()
    );
    for row in &report.recent_minute_rollups {
        let _ = writeln!(
            out,
            "  [{}] {}/{}/{}: {} reqs, {} tokens, cost: {} micros",
            row.bucket_start_ms,
            row.provider,
            row.model,
            row.status,
            row.request_count,
            row.total_input_tokens + row.output_tokens,
            row.priced_cost_micros
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_realtime_metrics_debug_text_and_json() {
        let temp = axon_test_support::test_temp_dir();
        let config_file = temp.path().join("config.toml");
        std::fs::write(&config_file, "").unwrap();

        let cmd = DebugRealtimeMetricsCommand {
            window: 15,
            minute_limit: 5,
            rebuild_bucket: None,
        };

        // 1. Text 输出
        let result_text = run_realtime_metrics_debug(&cmd, OutputFormat::Text, Some(&config_file));
        assert_eq!(result_text.exit_code, 0);
        assert!(
            result_text
                .stdout
                .contains("Axon debug realtime-metrics (window: 15m)")
        );
        assert!(result_text.stdout.contains("- requests:"));
        assert!(result_text.stdout.contains("- current_rpm:"));

        // 2. Json 输出
        let result_json = run_realtime_metrics_debug(&cmd, OutputFormat::Json, Some(&config_file));
        assert_eq!(result_json.exit_code, 0);
        assert!(result_json.stdout.contains("\"window\":\"15m\""));
        assert!(result_json.stdout.contains("\"windowRequestCount\""));
    }
}
