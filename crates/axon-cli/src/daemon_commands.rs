//! 通用 Daemon 的 CLI 组合根与 IPC 客户端。

use crate::runtime_paths::resolve_daemon_endpoint;
use axon_daemon::infrastructure::ipc::JsonlIpcClient;
use axon_protocol::{DaemonCommand as ProtocolCommand, DaemonRequest};
use clap::{Args, Subcommand};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[path = "daemon_delivery_health.rs"]
pub(crate) mod daemon_delivery_health;
#[path = "daemon_host_runner.rs"]
mod daemon_host_runner;
#[path = "daemon_service_manager.rs"]
mod daemon_service_manager;
#[path = "daemon_stop_request.rs"]
pub(crate) mod daemon_stop_request;
pub(crate) use daemon_service_manager::process_alive;

pub(crate) const DEFAULT_ENDPOINT: &str = crate::runtime_paths::DEFAULT_DAEMON_ENDPOINT;
pub(crate) const RUNTIME_DATA_DIR: &str = "data";

/// `axon daemon` 命令。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DaemonCommand {
    /// Daemon 动作。
    #[command(subcommand)]
    pub action: DaemonAction,
}

/// Daemon 动作集合。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum DaemonAction {
    /// 将 Daemon 注册为当前用户的操作系统服务。
    Install,
    /// 删除当前用户的 Daemon 操作系统服务。
    Uninstall,
    /// 启动已注册的 Daemon 服务。
    Start,
    /// 停止 Daemon 服务。
    Stop,
    /// 查看 Daemon 服务是否已注册且正在运行。
    Status,
    /// 重启 Daemon 服务。
    Restart,
    /// 启动后台 Host 和跨平台 JSONL IPC。
    Run(DaemonRunCommand),
    /// attach Session 并 replay 事件。
    Attach(DaemonAttachCommand),
    /// 创建或更新持久 interval schedule。
    Schedule(DaemonScheduleCommand),
    /// 请求升级排空。
    Drain(DaemonDrainCommand),
    /// 查看 Daemon 内部健康指标。
    Health(DaemonHealthCommand),
}

/// Daemon 启动参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DaemonRunCommand {
    /// 用户 Axon 根目录；Daemon 会把 runtime.sqlite3 放到其 data 子目录。
    #[arg(long)]
    pub root: PathBuf,
    /// Provider 配置路径。
    #[arg(long = "config-path")]
    pub config_path: PathBuf,
    /// 本地 IPC endpoint。
    #[arg(long, default_value = DEFAULT_ENDPOINT)]
    pub endpoint: String,
}

/// attach 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DaemonAttachCommand {
    /// 本地 IPC endpoint。
    #[arg(long, default_value = DEFAULT_ENDPOINT)]
    pub endpoint: String,
    /// Session ID。
    pub session_id: String,
    /// 从指定事件序号继续。
    #[arg(long, default_value_t = 0)]
    pub sequence: u64,
    /// 上次 attach 返回的 Host generation。
    #[arg(long)]
    pub generation: Option<String>,
}

/// 持久 interval schedule 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DaemonScheduleCommand {
    /// 本地 IPC endpoint。
    #[arg(long, default_value = DEFAULT_ENDPOINT)]
    pub endpoint: String,
    /// 稳定 schedule ID。
    pub schedule_id: String,
    /// 目标 Session；省略时仅保留调度记录。
    #[arg(long)]
    pub session_id: Option<String>,
    /// 调度 Action 类型。
    #[arg(long)]
    pub kind: String,
    /// 调度 Action 的 prompt。
    #[arg(long)]
    pub prompt: String,
    /// 调度归属的 Workspace ID。
    #[arg(long = "workspace-id")]
    pub workspace_id: String,
    /// interval 毫秒数。
    #[arg(long)]
    pub interval_ms: u64,
    /// 投递模式：follow_up 或 steer。
    #[arg(long, default_value = "follow_up")]
    pub delivery_mode: String,
}

/// Daemon 排空参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DaemonDrainCommand {
    /// 本地 IPC endpoint。
    #[arg(long, default_value = DEFAULT_ENDPOINT)]
    pub endpoint: String,
    /// 排空原因。
    #[arg(long, default_value = "cli_request")]
    pub reason: String,
}

/// Daemon 健康查询参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DaemonHealthCommand {
    /// 本地 IPC endpoint。
    #[arg(long, default_value = DEFAULT_ENDPOINT)]
    pub endpoint: String,
}

/// 执行 Daemon CLI 命令。
pub fn run_daemon(command: &DaemonCommand) -> crate::CliRunResult {
    match &command.action {
        DaemonAction::Install => run_service_action(ServiceAction::Install),
        DaemonAction::Uninstall => run_service_action(ServiceAction::Uninstall),
        DaemonAction::Start => run_service_action(ServiceAction::Start),
        DaemonAction::Stop => run_service_action(ServiceAction::Stop),
        DaemonAction::Status => run_service_action(ServiceAction::Status),
        DaemonAction::Restart => run_service_action(ServiceAction::Restart),
        DaemonAction::Run(command) => daemon_host_runner::run_daemon_host(command),
        DaemonAction::Attach(command) => send_request(
            &command.endpoint,
            ProtocolCommand::Attach {
                session_id: command.session_id.clone(),
                resume_cursor: command.generation.as_ref().map(|generation| {
                    axon_domain::ConsumerCursor::new(generation, command.sequence)
                }),
                capabilities: Vec::new(),
            },
        ),
        DaemonAction::Schedule(command) => send_request(
            &command.endpoint,
            ProtocolCommand::UpsertSchedule {
                schedule_id: command.schedule_id.clone(),
                session_id: command.session_id.clone(),
                kind: command.kind.clone(),
                interval_ms: command.interval_ms,
                delivery_mode: command.delivery_mode.clone(),
                payload: json!({"prompt": command.prompt, "workspaceId": command.workspace_id}),
            },
        ),
        DaemonAction::Drain(command) => send_request(
            &command.endpoint,
            ProtocolCommand::Drain {
                reason: command.reason.clone(),
                target_version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        ),
        DaemonAction::Health(command) => {
            send_request(&command.endpoint, ProtocolCommand::GetHealth)
        }
    }
}

#[derive(Clone, Copy)]
enum ServiceAction {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
    Restart,
}

fn run_service_action(action: ServiceAction) -> crate::CliRunResult {
    let config_path = match crate::resolved_config_path(None) {
        Ok(path) => path,
        Err(error) => return cli_error_result(error.to_owned()),
    };
    let root = match axon_config::runtime_axon_root_path(&config_path) {
        Ok(root) => root,
        Err(error) => return cli_error_result(format!("{error:?}")),
    };
    let runtime_dir = root.join(RUNTIME_DATA_DIR);
    let result = match action {
        ServiceAction::Install => daemon_service_manager::install(&config_path, &runtime_dir),
        ServiceAction::Uninstall => daemon_service_manager::uninstall(&runtime_dir),
        ServiceAction::Start => daemon_service_manager::start(&config_path, &runtime_dir),
        ServiceAction::Stop => daemon_service_manager::stop(&runtime_dir),
        ServiceAction::Status => {
            let report = serde_json::json!({
                "installed": daemon_service_manager::installed(),
                "running": daemon_service_manager::service_running(&runtime_dir),
            });
            return crate::CliRunResult {
                exit_code: 0,
                stdout: format!("{report}\n"),
                stderr: String::new(),
            };
        }
        ServiceAction::Restart => daemon_service_manager::restart(&config_path, &runtime_dir),
    };
    result.map_or_else(cli_error_result, |()| crate::CliRunResult {
        exit_code: 0,
        stdout: "Daemon 服务操作完成。\n".to_owned(),
        stderr: String::new(),
    })
}

fn cli_error_result(error: String) -> crate::CliRunResult {
    crate::CliRunResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: error,
    }
}

fn send_request(endpoint: &str, command: ProtocolCommand) -> crate::CliRunResult {
    let request = DaemonRequest {
        message_type: "command".to_owned(),
        request_id: format!("cli-{}", now_ms()),
        protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
        client_id: "axon-cli".to_owned(),
        session_id: None,
        command,
    };
    let endpoint = match resolve_daemon_endpoint(endpoint, None) {
        Ok(endpoint) => endpoint,
        Err(error) => return cli_error_result(error),
    };
    match JsonlIpcClient::connect(&endpoint).and_then(|mut client| client.send(&request)) {
        Ok(response) => crate::CliRunResult {
            exit_code: u8::from(!response.ok),
            stdout: response.body.to_string(),
            stderr: String::new(),
        },
        Err(error) => cli_error_result(format!(
            "无法连接 Axon Daemon（IPC 端点 {endpoint}）：{error}\n"
        )),
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
#[path = "daemon_commands_tests.rs"]
mod tests;
