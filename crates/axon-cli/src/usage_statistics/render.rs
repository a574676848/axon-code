use std::fmt::Write as _;

use chrono::{Local, TimeZone as _};

use super::model::{UsageDashboardReport, UsageMetrics};

pub(super) fn render_text(report: &UsageDashboardReport) -> String {
    let mut output = String::new();
    let summary = &report.summary;
    let _ = writeln!(output, "使用统计（{}）", report.period.label());
    let _ = writeln!(
        output,
        "请求 {}  Provider 调用 {}  成功率 {}  平均 Provider 延迟 {}ms",
        format_number(summary.request_count),
        format_number(summary.provider_call_count),
        format_basis_points(summary.success_rate_basis_points),
        format_number(summary.average_provider_latency_ms),
    );
    let _ = writeln!(
        output,
        "Tokens {}  完整输入 {}，缓存命中 {}，缓存写入 {}，输出 {}，推理 {}，总计 {}，命中率 {}",
        format_number(summary.total_tokens),
        format_number(summary.total_input_tokens),
        format_number(summary.cache_read_input_tokens),
        format_number(summary.cache_write_input_tokens),
        format_number(summary.output_tokens),
        format_number(summary.reasoning_output_tokens),
        format_number(summary.total_tokens),
        axon_display::format_basis_points_percent(summary.cache_hit_rate_basis_points),
    );
    let _ = writeln!(
        output,
        "成本 {}  投影 {}",
        format_cost(summary),
        projection_status(report),
    );
    render_providers(&mut output, report);
    render_models(&mut output, report);
    render_requests(&mut output, report);
    output
}

fn render_providers(output: &mut String, report: &UsageDashboardReport) {
    let _ = writeln!(output, "\nProvider 统计");
    for item in &report.providers {
        let _ = writeln!(
            output,
            "- {}: 请求={} Tokens={} 成功率={} 平均延迟={}ms",
            item.provider,
            format_number(item.metrics.request_count),
            format_number(item.metrics.total_tokens),
            format_basis_points(item.metrics.success_rate_basis_points),
            format_number(item.metrics.average_provider_latency_ms),
        );
    }
    if report.providers.is_empty() {
        let _ = writeln!(output, "- 暂无数据");
    }
}

fn render_models(output: &mut String, report: &UsageDashboardReport) {
    let _ = writeln!(output, "\n模型统计");
    for item in &report.models {
        let _ = writeln!(
            output,
            "- {}: 请求={} Tokens={} 平均成本={}",
            item.model,
            format_number(item.metrics.request_count),
            format_number(item.metrics.total_tokens),
            average_cost(&item.metrics),
        );
    }
    if report.models.is_empty() {
        let _ = writeln!(output, "- 暂无数据");
    }
}

fn render_requests(output: &mut String, report: &UsageDashboardReport) {
    let _ = writeln!(output, "\n最近请求");
    for item in &report.recent_requests {
        let _ = writeln!(
            output,
            "- {} {} / {} source={} status={} Tokens={} calls={} latency={}ms",
            format_time(item.recorded_at_ms),
            item.provider,
            item.model,
            item.source,
            item.status,
            format_number(item.total_tokens),
            item.provider_calls,
            item.provider_duration_ms,
        );
    }
    if report.recent_requests.is_empty() {
        let _ = writeln!(output, "- 暂无数据");
    }
}

fn projection_status(report: &UsageDashboardReport) -> String {
    if report.projection.last_success_at_ms.is_none() {
        return "尚未生成，请运行 axon usage --refresh".to_owned();
    }
    if report.projection.stale {
        return "有待聚合数据，Gateway Cron 将自动补齐".to_owned();
    }
    format!(
        "已同步 {} 条事实",
        format_number(report.projection.projected_fact_count)
    )
}

fn format_cost(metrics: &UsageMetrics) -> String {
    match metrics.cost_status {
        "priced" => format_micros(metrics.priced_cost_micros),
        "partial" => format!(
            "部分定价 {}（{} 条未定价）",
            format_micros(metrics.priced_cost_micros),
            metrics.unpriced_request_count
        ),
        _ => "未定价".to_owned(),
    }
}

fn average_cost(metrics: &UsageMetrics) -> String {
    let priced_requests = metrics
        .request_count
        .saturating_sub(metrics.unpriced_request_count);
    let Some(average_micros) = metrics.priced_cost_micros.checked_div(priced_requests) else {
        return "未定价".to_owned();
    };
    format_micros(average_micros)
}

fn format_micros(micros: u64) -> String {
    format!("${}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

fn format_basis_points(value: u16) -> String {
    axon_display::format_basis_points_percent(value)
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn format_time(timestamp_ms: u64) -> String {
    i64::try_from(timestamp_ms)
        .ok()
        .and_then(|timestamp| Local.timestamp_millis_opt(timestamp).single())
        .map_or_else(
            || timestamp_ms.to_string(),
            |value| value.format("%m-%d %H:%M:%S").to_string(),
        )
}
