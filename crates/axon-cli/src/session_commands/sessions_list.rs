//! `axon sessions list` 实现。
//!
//! 与 `GET /api/v1/sessions` 共用同一份投影（`sessions_http_adapter::read_session_rows`），
//! 因此命令行与桌面端看到的标题、最近活动与轮次完全一致。默认只列未归档会话，
//! `--all` 才包含归档项，与 TUI 的 `/sessions --all` 口径相同。

use std::path::Path;

use clap::Args;

use axon_session::SqliteRuntimeStore;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
    runtime_paths::runtime_axon_data_path_static,
    sessions_http_adapter::read_session_rows,
};

/// `axon sessions list` 参数。
#[derive(Debug, Clone, Default, Args, PartialEq, Eq)]
pub struct SessionsListCommand {
    /// 最多列出多少条会话。
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// 包含已归档会话。
    #[arg(long)]
    pub all: bool,
}

pub(crate) fn run_sessions_list(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionsListCommand,
) -> CliRunResult {
    if command.limit == 0 {
        return cli_error("--limit 必须大于 0".to_owned());
    }
    let data_root = match runtime_axon_data_path_static(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let store = SqliteRuntimeStore::from_root(data_root);
    // 运行态库还没建出来时如实报告空清单，不是错误。
    let rows = read_session_rows(&store).unwrap_or_default();
    let visible = rows
        .into_iter()
        .filter(|row| command.all || !row.archived)
        .take(command.limit)
        .collect::<Vec<_>>();
    match format {
        OutputFormat::Json => {
            json_result(&visible.iter().map(|row| row.to_json()).collect::<Vec<_>>())
        }
        OutputFormat::Text => {
            let mut stdout = String::new();
            if visible.is_empty() {
                stdout.push_str("未找到会话\n");
            } else {
                for row in &visible {
                    let title = if row.title.trim().is_empty() {
                        row.session_id.as_str()
                    } else {
                        row.title.as_str()
                    };
                    let pin = if row.pinned { "*" } else { " " };
                    let archived = if row.archived { " [已归档]" } else { "" };
                    stdout.push_str(&format!(
                        "{pin}{}\t{}\tlast={}\tturns={}{archived}\n",
                        row.session_id,
                        title,
                        format_local_time(row.last_activity_at_ms),
                        row.turn_count,
                    ));
                }
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

/// 把毫秒时间戳格式化为本地时间；越界或不可表示时原样回退成毫秒数。
fn format_local_time(timestamp_ms: u64) -> String {
    use chrono::TimeZone as _;
    i64::try_from(timestamp_ms).ok().map_or_else(
        || format!("{timestamp_ms}ms"),
        |value| {
            chrono::Local
                .timestamp_millis_opt(value)
                .single()
                .map_or_else(
                    || format!("{timestamp_ms}ms"),
                    |time| time.format("%Y-%m-%d %H:%M").to_string(),
                )
        },
    )
}
