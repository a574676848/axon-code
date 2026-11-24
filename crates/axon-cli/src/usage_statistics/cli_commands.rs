//! `axon usage` 与 `axon pricing` 命令行实现（SSOT §5.4 ~ §5.6 / §2.3 - M6）。

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result, resolved_config_path},
};

use super::model::UsagePeriod;
use super::observability_model::{DiagnosticsReport, SavePricingRulePayload};
use super::observability_service::{
    delete_pricing_rule_service, export_audit_logs_service, list_pricing_rules_service,
    query_audit_logs_service, query_diagnostics_service, recalculate_pricing_service,
    save_pricing_rule_service, sync_pricing_service,
};
use super::repository::audit::AuditLogQuery;

// ============================================================================
// `axon usage` 参数与子命令
// ============================================================================

/// `axon usage` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct UsageCommand {
    /// 可选的 usage 子命令（diagnostics、audit、export）。若未指定，展示看板摘要。
    #[command(subcommand)]
    pub action: Option<UsageSubcommand>,

    /// 查询周期：today、7d、30d 或 all。
    #[arg(long, value_enum, default_value_t = UsagePeriod::Today)]
    pub period: UsagePeriod,
    /// 仅查询指定 Provider。
    #[arg(long)]
    pub provider: Option<String>,
    /// 仅查询指定模型。
    #[arg(long)]
    pub model: Option<String>,
    /// 来源过滤器。
    #[arg(long, value_enum, default_value_t = super::model::UsageSourceFilter::All)]
    pub source: super::model::UsageSourceFilter,
    /// 仅查询指定 session。
    #[arg(long)]
    pub session: Option<String>,
    /// 仅查询指定 run（assistant part）。
    #[arg(long)]
    pub run: Option<String>,
    /// 最近请求返回数量。
    #[arg(long, default_value_t = super::USAGE_DEFAULT_RECENT_LIMIT)]
    pub limit: usize,
    /// 查询前立即执行一次增量聚合；默认由 Gateway Cron 定时聚合。
    #[arg(long)]
    pub refresh: bool,
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// `axon usage` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum UsageSubcommand {
    /// 延迟分位数、散点与模型效益深度诊断。
    Diagnostics(UsageDiagnosticsCommand),
    /// 90 天审计日志游标分页检索。
    Audit(UsageAuditCommand),
    /// 审计日志 CSV/JSON 流式导出。
    Export(UsageExportCommand),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct UsageDiagnosticsCommand {
    /// 诊断周期（仅支持 today 或 7d）。
    #[arg(long, default_value = "7d")]
    pub period: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct UsageAuditCommand {
    /// 筛选模型名称。
    #[arg(long)]
    pub model: Option<String>,
    /// 筛选来源。
    #[arg(long)]
    pub source: Option<String>,
    /// 筛选请求状态（如 completed, failed, active）。
    #[arg(long)]
    pub status: Option<String>,
    /// 起始毫秒时间戳。
    #[arg(long)]
    pub from_ms: Option<u64>,
    /// 截止毫秒时间戳。
    #[arg(long)]
    pub to_ms: Option<u64>,
    /// Base64 分页游标。
    #[arg(long)]
    pub cursor: Option<String>,
    /// 单页记录数限制（最大 500）。
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct UsageExportCommand {
    /// 导出格式：csv 或 json。
    #[arg(long, default_value = "csv")]
    pub format: String,
    /// 导出目标文件路径；不传则标准输出。
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// 筛选模型名称。
    #[arg(long)]
    pub model: Option<String>,
    /// 筛选来源。
    #[arg(long)]
    pub source: Option<String>,
    /// 筛选状态。
    #[arg(long)]
    pub status: Option<String>,
    /// 起始毫秒时间戳。
    #[arg(long)]
    pub from_ms: Option<u64>,
    /// 截止毫秒时间戳。
    #[arg(long)]
    pub to_ms: Option<u64>,
}

// ============================================================================
// `axon pricing` 参数与子命令
// ============================================================================

/// `axon pricing` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct PricingCommand {
    /// 价格管理子命令（list、add、delete、sync、recalculate）。
    #[command(subcommand)]
    pub action: Option<PricingSubcommand>,

    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// `axon pricing` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum PricingSubcommand {
    /// 列出所有模型价格规则（默认）。
    List,
    /// 添加或更新模型价格规则。
    Add(AddPricingRuleArgs),
    /// 删除模型价格规则。
    Delete(DeletePricingRuleArgs),
    /// 从 models.dev 同步最新官方模型价格。
    Sync,
    /// 触发历史用量重新核算计费。
    Recalculate(RecalculatePricingArgs),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct AddPricingRuleArgs {
    /// 模型匹配模式通配符（如 gpt-5*, claude-3*）。
    #[arg(long)]
    pub pattern: String,
    /// 计费风格（openai 或 claude）。
    #[arg(long)]
    pub style: String,
    /// 每 100 万输入 Token 价格（微美元）。
    #[arg(long)]
    pub input_price: u64,
    /// 每 100 万输出 Token 价格（微美元）。
    #[arg(long)]
    pub output_price: u64,
    /// 每 100 万缓存读取 Token 价格（微美元）。
    #[arg(long, default_value_t = 0)]
    pub cache_read_price: u64,
    /// 每 100 万缓存写入 Token 价格（微美元）。
    #[arg(long, default_value_t = 0)]
    pub cache_write_price: u64,
    /// 倍率基点（默认 10000 = 1.0x）。
    #[arg(long, default_value_t = 10000)]
    pub multiplier: u32,
    /// 规则优先级（数字越大优先级越高）。
    #[arg(long, default_value_t = 0)]
    pub priority: i32,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DeletePricingRuleArgs {
    /// 待删除规则 ID。
    #[arg(long)]
    pub id: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RecalculatePricingArgs {
    /// 起始毫秒时间戳（可选）。
    #[arg(long)]
    pub from_ms: Option<u64>,
}

// ============================================================================
// 运行入口
// ============================================================================

pub(crate) fn run_usage_cli(
    command: &UsageCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let config_path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };

    match &command.action {
        Some(UsageSubcommand::Diagnostics(cmd)) => run_usage_diagnostics(cmd, format, &config_path),
        Some(UsageSubcommand::Audit(cmd)) => run_usage_audit(cmd, format, &config_path),
        Some(UsageSubcommand::Export(cmd)) => run_usage_export(cmd, &config_path),
        None => run_usage_dashboard(command, format, &config_path),
    }
}

pub(crate) fn run_pricing_cli(
    command: &PricingCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let config_path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };

    let action = command.action.as_ref().unwrap_or(&PricingSubcommand::List);
    match action {
        PricingSubcommand::List => run_pricing_list(format, &config_path),
        PricingSubcommand::Add(args) => run_pricing_add(args, format, &config_path),
        PricingSubcommand::Delete(args) => run_pricing_delete(args, format, &config_path),
        PricingSubcommand::Sync => run_pricing_sync(format, &config_path),
        PricingSubcommand::Recalculate(args) => run_pricing_recalculate(args, format, &config_path),
    }
}

// ============================================================================
// Usage 命令子项实现
// ============================================================================

fn run_usage_dashboard(
    command: &UsageCommand,
    format: OutputFormat,
    config_path: &Path,
) -> CliRunResult {
    if command.limit == 0 || command.limit > super::USAGE_MAX_RECENT_LIMIT {
        return cli_error(format!(
            "usage --limit 必须在 1..={} 范围内",
            super::USAGE_MAX_RECENT_LIMIT
        ));
    }
    if command.refresh
        && let Err(error) = super::projection::run_usage_rollup_once(config_path)
    {
        return cli_error(error);
    }
    let report = match super::service::query_dashboard(
        config_path,
        &super::service::UsageDashboardQuery {
            period: command.period,
            provider: command.provider.as_deref(),
            model: command.model.as_deref(),
            source: command.source,
            session_id: command.session.as_deref(),
            run_id: command.run.as_deref(),
            recent_limit: command.limit,
        },
    ) {
        Ok(report) => report,
        Err(error) => return cli_error(error),
    };
    match format {
        OutputFormat::Json => json_result(&report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: super::render::render_text(&report),
            stderr: String::new(),
        },
    }
}

fn run_usage_diagnostics(
    cmd: &UsageDiagnosticsCommand,
    format: OutputFormat,
    config_path: &Path,
) -> CliRunResult {
    let report = match query_diagnostics_service(config_path, Some(&cmd.period)) {
        Ok(r) => r,
        Err(e) => return cli_error(e),
    };
    match format {
        OutputFormat::Json => json_result(&report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: render_diagnostics_text(&report),
            stderr: String::new(),
        },
    }
}

fn render_diagnostics_text(report: &DiagnosticsReport) -> String {
    let mut out = String::new();
    out.push_str("【延迟与效率深度诊断】\n");
    out.push_str(&format!("  样本总数: {}\n", report.sample_count));
    if let Some(s) = &report.sampling {
        out.push_str(&format!(
            "  抽稀披露: 原始 {} 条，采纳 {} 条 (比率 {:.1}%)\n",
            s.sampled_of,
            s.sample_count,
            s.sampling_ratio * 100.0
        ));
    }
    out.push_str("  首字延迟分位数 (TTFT):\n");
    out.push_str(&format!(
        "    p50: {}ms | p90: {}ms | p95: {}ms | p99: {}ms\n",
        report.percentiles.p50_ttft_ms.unwrap_or(0),
        report.percentiles.p90_ttft_ms.unwrap_or(0),
        report.percentiles.p95_ttft_ms.unwrap_or(0),
        report.percentiles.p99_ttft_ms.unwrap_or(0)
    ));
    out.push_str("  执行延迟分位数 (Latency):\n");
    out.push_str(&format!(
        "    p50: {}ms | p90: {}ms | p95: {}ms | p99: {}ms\n",
        report.percentiles.p50_latency_ms.unwrap_or(0),
        report.percentiles.p90_latency_ms.unwrap_or(0),
        report.percentiles.p95_latency_ms.unwrap_or(0),
        report.percentiles.p99_latency_ms.unwrap_or(0)
    ));
    out.push_str(&format!(
        "  诊断散点总计: {} 点\n",
        report.scatter_points.len()
    ));
    out.push_str("  模型单价效益分析 (USD / 1M tokens):\n");
    for item in &report.model_efficiency {
        out.push_str(&format!(
            "    - {}: ${:.4} / 1M tokens\n",
            item.model, item.cost_per1m_tokens
        ));
    }
    out
}

fn run_usage_audit(
    cmd: &UsageAuditCommand,
    format: OutputFormat,
    config_path: &Path,
) -> CliRunResult {
    let query = AuditLogQuery {
        model: cmd.model.as_deref(),
        source: cmd.source.as_deref(),
        status: cmd.status.as_deref(),
        from_ms: cmd.from_ms,
        to_ms: cmd.to_ms,
        cursor: cmd.cursor.as_deref(),
        limit: cmd.limit,
    };
    let report = match query_audit_logs_service(config_path, &query) {
        Ok(r) => r,
        Err(e) => return cli_error(e),
    };
    match format {
        OutputFormat::Json => json_result(&report),
        OutputFormat::Text => {
            let mut out = String::new();
            out.push_str(&format!(
                "【审计日志】符合条件总数: {}\n",
                report.total_count
            ));
            out.push_str(&format!(
                "  当前页条数: {} (hasMore: {})\n",
                report.events.len(),
                report.has_more
            ));
            if let Some(c) = &report.next_cursor {
                out.push_str(&format!("  下页游标: {c}\n"));
            }
            out.push_str("  --------------------------------------------------------------------------------\n");
            for ev in &report.events {
                out.push_str(&format!(
                    "  [{}] model={} cost={}µ$ tokens={} pricing={}\n",
                    ev.recorded_at_ms,
                    ev.model,
                    ev.cost_micros.unwrap_or(0),
                    ev.input_tokens + ev.output_tokens,
                    ev.pricing_style
                ));
            }
            CliRunResult {
                exit_code: 0,
                stdout: out,
                stderr: String::new(),
            }
        }
    }
}

fn run_usage_export(cmd: &UsageExportCommand, config_path: &Path) -> CliRunResult {
    let query = AuditLogQuery {
        model: cmd.model.as_deref(),
        source: cmd.source.as_deref(),
        status: cmd.status.as_deref(),
        from_ms: cmd.from_ms,
        to_ms: cmd.to_ms,
        cursor: None,
        limit: 500,
    };
    let mut buffer = Vec::new();
    let count = match export_audit_logs_service(config_path, &query, &cmd.format, &mut buffer) {
        Ok(c) => c,
        Err(e) => return cli_error(e),
    };

    if let Some(out_path) = &cmd.output {
        if let Err(e) = std::fs::write(out_path, &buffer) {
            return cli_error(format!("写入导出文件失败: {e}"));
        }
        CliRunResult {
            exit_code: 0,
            stdout: format!("已成功导出 {count} 条审计记录至 {}\n", out_path.display()),
            stderr: String::new(),
        }
    } else {
        let content = String::from_utf8_lossy(&buffer).to_string();
        CliRunResult {
            exit_code: 0,
            stdout: content,
            stderr: String::new(),
        }
    }
}

// ============================================================================
// Pricing 命令实现
// ============================================================================

fn run_pricing_list(format: OutputFormat, config_path: &Path) -> CliRunResult {
    let rules = match list_pricing_rules_service(config_path) {
        Ok(r) => r,
        Err(e) => return cli_error(e),
    };
    match format {
        OutputFormat::Json => json_result(&rules),
        OutputFormat::Text => {
            let mut out = String::new();
            out.push_str("【模型费率规则列表】\n");
            if rules.is_empty() {
                out.push_str("  (暂无价格规则)\n");
            } else {
                for r in &rules {
                    out.push_str(&format!(
                        "  [{}] pattern={} style={} input={}µ$ output={}µ$ priority={}\n",
                        r.id,
                        r.model_pattern,
                        r.pricing_style.as_str(),
                        r.input_price_per_m_micros,
                        r.output_price_per_m_micros,
                        r.priority
                    ));
                }
            }
            CliRunResult {
                exit_code: 0,
                stdout: out,
                stderr: String::new(),
            }
        }
    }
}

fn run_pricing_add(
    args: &AddPricingRuleArgs,
    format: OutputFormat,
    config_path: &Path,
) -> CliRunResult {
    let payload = SavePricingRulePayload {
        model_pattern: args.pattern.clone(),
        pricing_style: args.style.clone(),
        input_price_per_m_micros: args.input_price,
        output_price_per_m_micros: args.output_price,
        cache_read_price_per_m_micros: args.cache_read_price,
        cache_write_price_per_m_micros: args.cache_write_price,
        multiplier_basis_points: args.multiplier,
        priority: args.priority,
    };
    let id = match save_pricing_rule_service(config_path, payload) {
        Ok(id) => id,
        Err(e) => return cli_error(e),
    };
    match format {
        OutputFormat::Json => json_result(&serde_json::json!({ "id": id, "status": "created" })),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!("成功创建价格规则: {id}\n"),
            stderr: String::new(),
        },
    }
}

fn run_pricing_delete(
    args: &DeletePricingRuleArgs,
    format: OutputFormat,
    config_path: &Path,
) -> CliRunResult {
    match delete_pricing_rule_service(config_path, &args.id) {
        Ok(true) => match format {
            OutputFormat::Json => {
                json_result(&serde_json::json!({ "id": args.id, "status": "deleted" }))
            }
            OutputFormat::Text => CliRunResult {
                exit_code: 0,
                stdout: format!("成功删除价格规则: {}\n", args.id),
                stderr: String::new(),
            },
        },
        Ok(false) => cli_error(format!("未找到指定的规则: {}", args.id)),
        Err(e) => cli_error(e),
    }
}

fn run_pricing_sync(format: OutputFormat, config_path: &Path) -> CliRunResult {
    let resp = match block_on_async(sync_pricing_service(config_path)).and_then(|r| r) {
        Ok(r) => r,
        Err(e) => return cli_error(e),
    };
    match format {
        OutputFormat::Json => json_result(&resp),
        OutputFormat::Text => {
            // 「写入 0 条」不等于同步成功：按后端给出的结论分别成句，
            // 否则一次上游不可达会被读成「同步成功，只是没有新规则」。
            let detail = resp.detail.clone().unwrap_or_default();
            let text = match resp.outcome.as_str() {
                "success" => format!(
                    "从 models.dev 同步 {} 条模型价格规则 (更新于 {})\n",
                    resp.synced_rules_count, resp.updated_at_ms
                ),
                "debounced" | "rate_limited" => {
                    format!("本次未同步：{detail}（本地规则保持原样）\n")
                }
                _ => format!("同步未完成，本地规则保持原样：{detail}\n"),
            };
            CliRunResult {
                exit_code: 0,
                stdout: text,
                stderr: String::new(),
            }
        }
    }
}

fn run_pricing_recalculate(
    args: &RecalculatePricingArgs,
    format: OutputFormat,
    config_path: &Path,
) -> CliRunResult {
    let resp = match block_on_async(recalculate_pricing_service(config_path, args.from_ms))
        .and_then(|r| r)
    {
        Ok(r) => r,
        Err(e) => return cli_error(e),
    };
    match format {
        OutputFormat::Json => json_result(&resp),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "重算完成：共核算 {} 条事实，波及 {} 个时序聚合桶，耗时 {}ms\n",
                resp.recalculated_count, resp.affected_rollup_buckets, resp.elapsed_ms
            ),
            stderr: String::new(),
        },
    }
}

fn block_on_async<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| Ok(handle.block_on(future)))
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("创建 Tokio 运行时失败: {e}"))
            .map(|rt| rt.block_on(future))
    }
}
