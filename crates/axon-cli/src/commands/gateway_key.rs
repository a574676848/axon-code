//! `axon gateway key`：在服务器上直接管理 Gateway 多 Key。
//!
//! 三条子命令围绕同一份存储：`create` 签发、`list` 列出、`revoke` 吊销。存储路径与
//! 守护进程完全同源（`remote.gateway.auth_keys`，为空时取运行时根下的默认相对路径），
//! 因此这里签发的 Key 对正在运行的守护进程立即生效，不需要重启，也不存在第二套读写。
//!
//! 明文只在 `create` 的回执里出现一次：存储层只留盐与摘要，本模块也不在任何其它输出里
//! 回显它。

use super::*;
use std::{fmt::Write as _, sync::Arc};

use axon_daemon::infrastructure::gateway_auth::{GatewayKeyStore, KeySummary};
use chrono::TimeZone as _;
use unicode_width::UnicodeWidthStr;

/// `axon gateway` 顶层参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct GatewayCommand {
    /// 子命令；缺省时给出用法说明。
    #[command(subcommand)]
    pub command: Option<GatewaySubcommand>,
}

/// `axon gateway` 的子命令集合。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum GatewaySubcommand {
    /// 管理 Gateway 多 Key。
    Key(GatewayKeyCommand),
}

/// `axon gateway key` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct GatewayKeyCommand {
    /// 子命令。
    #[command(subcommand)]
    pub command: GatewayKeySubcommand,
}

/// `axon gateway key` 的子命令集合。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum GatewayKeySubcommand {
    /// 签发一把新 Key：明文只在本次回执里出现一次。
    Create(GatewayKeyCreateCommand),
    /// 列出全部未吊销的 Key：只给标签、时间与前 6 位指纹。
    List(GatewayKeyListCommand),
    /// 吊销一把 Key：对下一次鉴权立即生效。
    Revoke(GatewayKeyRevokeCommand),
}

/// `axon gateway key create` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct GatewayKeyCreateCommand {
    /// Key 的用途标签（必填，用于审计追溯）。
    #[arg(long)]
    pub label: String,
    /// 输出格式；缺省时沿用全局 `--format`。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// `axon gateway key list` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct GatewayKeyListCommand {
    /// 输出格式；缺省时沿用全局 `--format`。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// `axon gateway key revoke` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct GatewayKeyRevokeCommand {
    /// 要吊销的 Key 标识（`axon gateway key list` 的第一列）。
    pub id: String,
    /// 输出格式；缺省时沿用全局 `--format`。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// 执行 `axon gateway`。
pub(crate) fn run_gateway(
    command: &GatewayCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let Some(GatewaySubcommand::Key(key)) = &command.command else {
        return cli_error("请指定子命令：axon gateway key <create|list|revoke>");
    };
    match &key.command {
        GatewayKeySubcommand::Create(command) => {
            run_key_create(command.format.unwrap_or(format), config_path, command)
        }
        GatewayKeySubcommand::List(command) => {
            run_key_list(command.format.unwrap_or(format), config_path)
        }
        GatewayKeySubcommand::Revoke(command) => {
            run_key_revoke(command.format.unwrap_or(format), config_path, command)
        }
    }
}

/// 列出全部未吊销的 Key。
fn run_key_list(format: OutputFormat, config_path: Option<&Path>) -> CliRunResult {
    let store = match open_store(config_path) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let keys = store.list();
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: render_key_table(&keys),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&serde_json::json!({ "keys": keys })),
    }
}

/// 签发一把 Key，并把明文打印一次。
fn run_key_create(
    format: OutputFormat,
    config_path: Option<&Path>,
    command: &GatewayKeyCreateCommand,
) -> CliRunResult {
    let label = command.label.trim();
    if label.is_empty() {
        return cli_error("--label 不能为空白：标签用于审计追溯");
    }
    let store = match open_store(config_path) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let issued = match store.create(label) {
        Ok(issued) => issued,
        Err(error) => return cli_error(error.to_string()),
    };
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "已签发 Key：{}\nid：{}\n明文：{}\n提示：明文只显示这一次，离开终端后无法再查看，请立刻保存到安全位置。\n",
                issued.label, issued.id, issued.plaintext
            ),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(&serde_json::json!({
            "id": issued.id,
            "label": issued.label,
            // 明文只出现在这一条回执里。
            "plaintext": issued.plaintext,
        })),
    }
}

/// 吊销一把 Key。
fn run_key_revoke(
    format: OutputFormat,
    config_path: Option<&Path>,
    command: &GatewayKeyRevokeCommand,
) -> CliRunResult {
    let id = command.id.trim();
    if id.is_empty() {
        return cli_error("Key 标识不能为空");
    }
    let store = match open_store(config_path) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    match store.revoke(id) {
        Ok(()) => match format {
            OutputFormat::Text => CliRunResult {
                exit_code: 0,
                stdout: format!("已吊销 Key：{id}\n该 Key 对下一次鉴权立即失效。\n"),
                stderr: String::new(),
            },
            OutputFormat::Json => json_result(&serde_json::json!({ "id": id, "revoked": true })),
        },
        Err(error) => cli_error(error.to_string()),
    }
}

/// 打开与守护进程同源的 Key 存储。
///
/// 解析口径与 Daemon 逐字一致：配置路径决定 Axon 运行时根，`remote.gateway.auth_keys`
/// 为空时落在运行时根下的默认相对路径。**不另写一套存储读写**，否则运维在这里签发的
/// Key 在守护进程里查不到，而两边都自认为成功。
fn open_store(config_path: Option<&Path>) -> Result<Arc<GatewayKeyStore>, String> {
    let path = resolved_config_path(config_path)?;
    let loaded =
        ConfigLoader::load_from_path(&path).map_err(|error| format!("加载配置失败：{error:?}"))?;
    let root = crate::runtime_paths::runtime_axon_root_path(&path)?;
    GatewayKeyStore::open(&root, &loaded.config.remote.gateway.auth_keys)
        .map_err(|error| error.to_string())
}

/// 把 Key 清单渲染成一张按显示宽度对齐的表格。
///
/// 中文按两列计宽，因此表头与数据不会因为标签是中文而错位。
fn render_key_table(keys: &[KeySummary]) -> String {
    if keys.is_empty() {
        return "还没有签发任何 Key。用 axon gateway key create --label <名> 签发第一把。\n"
            .to_owned();
    }
    let rows: Vec<[String; 5]> = keys
        .iter()
        .map(|key| {
            [
                key.id.clone(),
                key.label.clone(),
                format_time_ms(key.created_at_ms),
                key.last_used_at_ms
                    .map_or_else(|| "从未使用".to_owned(), format_time_ms),
                key.fingerprint.clone(),
            ]
        })
        .collect();
    let header = ["ID", "标签", "创建时间", "最后使用", "指纹"];
    let mut widths = header.map(UnicodeWidthStr::width);
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
    }
    let mut stdout = String::new();
    let cells: Vec<&str> = header.to_vec();
    let _ = writeln!(stdout, "{}", render_row(&cells, &widths));
    for row in &rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        let _ = writeln!(stdout, "{}", render_row(&cells, &widths));
    }
    let _ = writeln!(stdout, "共 {} 把 Key", rows.len());
    stdout
}

/// 渲染一行：每列按显示宽度补齐，列间两个空格。
fn render_row(cells: &[&str], widths: &[usize]) -> String {
    let mut line = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            line.push_str("  ");
        }
        line.push_str(cell);
        if index + 1 < cells.len() {
            let width = widths.get(index).copied().unwrap_or(0);
            let fill = width.saturating_sub(UnicodeWidthStr::width(*cell));
            line.push_str(&" ".repeat(fill));
        }
    }
    line.trim_end().to_owned()
}

/// 毫秒时间戳渲染成本地时间；非法时间戳原样回退成数字，绝不显示伪造的时刻。
fn format_time_ms(timestamp_ms: u64) -> String {
    i64::try_from(timestamp_ms)
        .ok()
        .and_then(|value| chrono::Local.timestamp_millis_opt(value).single())
        .map_or_else(
            || timestamp_ms.to_string(),
            |value| value.format("%Y-%m-%d %H:%M").to_string(),
        )
}
