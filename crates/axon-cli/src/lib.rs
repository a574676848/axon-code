//! Axon CLI 参数解析与命令注册。
#[path = "args/cli_args.rs"]
mod cli_args;
#[path = "args/command_surface.rs"]
mod command_surface;
#[path = "args/completion_args.rs"]
mod completion_args;
#[path = "runtime/constants.rs"]
mod constants;
mod cron_commands;
mod cron_http_adapter;
mod daemon_commands;
mod display_units;
#[cfg(all(test, feature = "tui"))]
#[path = "tests/doctor_cli_tests.rs"]
mod doctor_cli_tests;
#[cfg(feature = "tui")]
#[path = "runtime/doctor_receipts.rs"]
mod doctor_receipts;
#[path = "args/exec_args.rs"]
mod exec_args;
#[path = "commands/exec.rs"]
mod exec_commands;
#[path = "commands/export.rs"]
mod export_commands;
#[path = "commands/export_report_contract.rs"]
mod export_report_contract;
mod goal_http_adapter;
mod interactive_tui;
#[path = "commands/lsp.rs"]
mod lsp_commands;
#[path = "args/management_args.rs"]
mod management_args;
#[path = "commands/management.rs"]
mod management_commands;
#[path = "commands/mcp/command_plan.rs"]
mod mcp_command_plan;
mod mcp_commands;
#[path = "commands/mcp/config_file.rs"]
mod mcp_config_file;
#[path = "mcp_http_adapter.rs"]
mod mcp_http_adapter;
#[path = "commands/mcp/local_store.rs"]
mod mcp_local_store;
mod memory_commands;
#[path = "memory_http_adapter.rs"]
mod memory_http_adapter;
#[path = "memory_http_scope.rs"]
mod memory_http_scope;
#[path = "args/package_args.rs"]
mod package_args;
mod package_commands;
mod persona_commands;
pub use persona_commands::{PersonaCommand, PersonaSubcommand};
#[path = "persona_http_adapter.rs"]
mod persona_http_adapter;
#[path = "commands/management/provider_report.rs"]
mod provider_report;
mod remote_commands;
mod run_commands;
#[path = "runtime/capability_routing.rs"]
mod runtime_capability_routing;
#[path = "runtime/memory_consolidation.rs"]
mod runtime_memory_consolidation;
#[path = "runtime/paths.rs"]
mod runtime_paths;
#[path = "runtime/persona_context.rs"]
mod runtime_persona_context;
#[path = "runtime/principal_context.rs"]
mod runtime_principal_context;
#[path = "runtime/prompt_fragments.rs"]
mod runtime_prompt_fragments;
#[path = "runtime/skill_context_fragments.rs"]
mod runtime_skill_context_fragments;
mod runtime_tools;
#[path = "runtime/user_agents.rs"]
mod runtime_user_agents;
#[path = "runtime/workspace_instructions.rs"]
mod runtime_workspace_instructions;
#[path = "commands/serve.rs"]
mod serve_commands;
mod session_commands;
#[path = "args/setup_args.rs"]
mod setup_args;
#[path = "commands/setup_check.rs"]
mod setup_check;
#[path = "commands/setup.rs"]
mod setup_commands;
#[path = "commands/skills.rs"]
mod skills_commands;
#[path = "runtime/stable_ids.rs"]
mod stable_ids;
#[path = "tools/orchestrator.rs"]
mod tool_orchestrator;
#[path = "tools/fs.rs"]
mod tool_orchestrator_fs;
#[path = "tools/fs_progress.rs"]
mod tool_orchestrator_fs_progress;
#[path = "tools/write_effects.rs"]
mod tool_orchestrator_fs_write_effects;
#[path = "tools/goal_tool.rs"]
mod tool_orchestrator_goal;
#[path = "tools/hashline.rs"]
mod tool_orchestrator_hashline;
#[path = "tools/job.rs"]
mod tool_orchestrator_job;
#[path = "tools/runtime_capabilities.rs"]
mod tool_orchestrator_runtime_capabilities;
#[path = "tools/shell.rs"]
mod tool_orchestrator_shell;
#[path = "runtime/turn_execution_ledger.rs"]
mod turn_execution_ledger;
mod upgrade_commands;
mod usage_statistics;
pub mod user_question;
mod workbench_http_adapter;

use std::{
    fmt::Write as _,
    io::Write as IoWrite,
    path::{Path, PathBuf},
    process::Command,
};

use axon_config::{
    ConfigLoader, ModelConfig, PROVIDER_PROTOCOL_ANTHROPIC, PROVIDER_PROTOCOL_DASHSCOPE,
    PROVIDER_PROTOCOL_OPENAI, PROVIDER_PROTOCOL_OPENAI_CHAT,
    PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL, PROVIDER_WIRE_TRANSPORT_HTTP,
    PROVIDER_WIRE_TRANSPORT_WEBSOCKET, ProviderConfig, add_provider, config_key_specs,
    get_config_value, remove_provider, set_config_value, to_toml_string,
};
use clap::{Args, CommandFactory, Subcommand, ValueEnum};
use serde::Serialize;

pub use cli_args::{
    AxonCli, AxonCommand, CliInvocation, CliRunResult, GlobalOptions, LogLevel, OutputFormat,
    RunCommand, ServeCommand, plan_invocation, registered_command_names,
};
pub use command_surface::{
    DebugBuiltinCommand, DebugBuiltinRunCommand, DebugBuiltinSubcommand,
    DebugChannelMetricsCommand, DebugCommand, DebugMailboxCommand, DebugMailboxState,
    DebugRealtimeMetricsCommand, DebugSubcommand, DoctorCommand, LspCommand, LspDiagnosticsCommand,
    LspDiagnosticsModeArg, LspPositionCommand, LspServersCommand, LspSubcommand,
    LspWorkspaceSymbolCommand, StopCommand, UpgradeCommand,
};
pub use completion_args::{CompletionCommand, CompletionShell};
use constants::provider_stream_idle_timeout;
pub use constants::{
    DOCTOR_CHECK_CLI, DOCTOR_CHECK_CONFIG, DOCTOR_CHECK_LSP, DOCTOR_CHECK_MCP, DOCTOR_CHECK_MEMORY,
    DOCTOR_CHECK_PERSONA, DOCTOR_CHECK_PREFLIGHT, DOCTOR_CHECK_PROVIDER,
    DOCTOR_CHECK_PROVIDER_RECOVERY, DOCTOR_CHECK_RUNTIME, DOCTOR_CHECK_SMOKE, DOCTOR_CHECK_TOOL,
    PROVIDER_TEST_TIMEOUT_MS,
};
pub use cron_commands::CronCommand;
use cron_commands::run_cron;
use daemon_commands::run_daemon;
pub use daemon_commands::{
    DaemonAction, DaemonAttachCommand, DaemonCommand, DaemonDrainCommand, DaemonRunCommand,
    DaemonScheduleCommand,
};
pub use exec_args::{ExecCommand, ExecKind};
use exec_commands::run_exec;
pub use export_commands::ExportCommand;
use export_commands::run_export;
#[cfg(feature = "tui")]
use interactive_tui::run_doctor;
#[cfg(not(feature = "tui"))]
use interactive_tui::run_doctor_without_terminal;
#[cfg(feature = "tui")]
pub use interactive_tui::{DoctorCheck, DoctorReport, DoctorStatus};
use lsp_commands::run_lsp;
pub use management_args::{
    ConfigCommand, ConfigGetCommand, ConfigSetCommand, ConfigSubcommand, ModelsCommand,
    ProviderAddCommand, ProviderCommand, ProviderNameCommand, ProviderProtocol,
    ProviderWireTransport, ProvidersCommand,
};
pub use management_commands::DebugReport;
use management_commands::{
    cli_error, json_result, resolved_config_path, run_completion, run_config, run_debug,
    run_models, run_providers,
};
use mcp_commands::run_mcp;
pub use mcp_commands::{McpCommand, McpSubcommand};
use memory_commands::run_memory;
pub use memory_commands::{
    MemoryCommand, MemoryConsistencyCommand, MemoryConsolidateCommand, MemoryKindFilter,
    MemorySubcommand,
};
pub use package_args::PackageCommand;
use package_commands::run_package;
use persona_commands::run_persona;
use remote_commands::run_remote;
pub use remote_commands::{
    RemoteCommand, RemoteListCommand, RemoteRenderCommand, RemoteSimulateCommand, RemoteSubcommand,
};
use run_commands::{request_stop, run_cli_daemon_interaction, run_planned_invocation};
use serve_commands::run_serve;
pub use session_commands::{SessionCommand, SessionSubcommand};
pub use setup_args::{
    AuthCommand, AuthStatusCommand, AuthSubcommand, SetupCommand, SetupProviderProtocol,
    SetupWireTransport,
};
use setup_commands::{run_auth, run_setup};
pub use skills_commands::SkillsCommand;
use skills_commands::run_skills;
use upgrade_commands::run_upgrade;
pub(crate) use upgrade_commands::{StartupUpdateGate, run_startup_update_gate};
pub use usage_statistics::{
    BillingEngine, PricingSyncService, RecalculateReport, SyncOutcome,
    TELEMETRY_METRIC_FACT_COLUMNS, UsageCommand, UsagePeriod, UsageSourceFilter,
    fact_column_to_metric, metric_to_fact_column,
};

/// 执行已解析的 CLI。
#[must_use]
pub fn run_parsed_cli(cli: &AxonCli) -> CliRunResult {
    run_parsed_cli_internal(cli, None)
}

/// 执行已解析的 CLI：`axon run` Text 模式实时写出 assistant 增量，JSON 模式写出终态 JSONL。
#[must_use]
pub fn run_parsed_cli_with_writer(cli: &AxonCli, writer: &mut dyn IoWrite) -> CliRunResult {
    run_parsed_cli_internal(cli, Some(writer))
}

fn run_parsed_cli_internal(
    cli: &AxonCli,
    run_json_writer: Option<&mut dyn IoWrite>,
) -> CliRunResult {
    #[cfg(windows)]
    if let Some(AxonCommand::Daemon(command)) = &cli.command
        && matches!(command.action, DaemonAction::Run(_))
    {
        return run_daemon(command);
    }
    let _runtime_release_lease = upgrade_commands::acquire_runtime_release_lease();
    upgrade_commands::cleanup_releases_after_startup();
    // 初始化日志系统
    let _ = init_logger_from_config(cli);

    if let Some(AxonCommand::GovernanceWorker(command)) = &cli.command {
        return match run_commands::run_governance_worker(&command.config_path) {
            Ok(_) => CliRunResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
            Err(error) => CliRunResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: error,
            },
        };
    }

    if let Some(guide_result) = setup_guide_result(cli) {
        return guide_result;
    }
    if let Some(result) = run_management_command(cli) {
        return result;
    }

    match &cli.command {
        Some(AxonCommand::Run(command)) => run_command_from_cli(cli, command, run_json_writer),
        Some(AxonCommand::Serve(command)) => {
            run_serve(command, cli.global.format, cli.global.config.as_deref())
        }
        Some(AxonCommand::Stop(command)) => {
            run_stop(command, cli.global.format, cli.global.config.as_deref())
        }
        Some(AxonCommand::Exec(command)) => run_exec(
            command,
            cli.global.format,
            cli.global.config.as_deref(),
            cli.global.cwd.as_deref(),
        ),
        #[cfg(feature = "tui")]
        Some(AxonCommand::Doctor(command)) => run_doctor(command, cli.global.format),
        #[cfg(not(feature = "tui"))]
        Some(AxonCommand::Doctor(command)) => {
            run_doctor_without_terminal(command, cli.global.format)
        }
        Some(AxonCommand::Completion(command)) => run_completion(command),
        Some(AxonCommand::Upgrade(command)) => {
            run_upgrade(command, command.format.unwrap_or(cli.global.format))
        }
        Some(AxonCommand::Package(command)) => {
            run_package(command, command.format.unwrap_or(cli.global.format))
        }
        Some(AxonCommand::Persona(command)) => run_persona(command, cli.global.config.as_deref()),
        Some(AxonCommand::Memory(command)) => {
            run_memory(command, cli.global.format, cli.global.config.as_deref())
        }
        Some(AxonCommand::Export(command)) => {
            run_export(command, cli.global.format, cli.global.config.as_deref())
        }
        Some(AxonCommand::Daemon(command)) => run_daemon(command),
        Some(AxonCommand::Cron(command)) => {
            run_cron(command, cli.global.format, cli.global.config.as_deref())
        }
        Some(AxonCommand::Usage(command)) => usage_statistics::run_usage(
            command,
            command.format.unwrap_or(cli.global.format),
            cli.global.config.as_deref(),
        ),
        Some(AxonCommand::Pricing(command)) => usage_statistics::run_pricing(
            command,
            command.format.unwrap_or(cli.global.format),
            cli.global.config.as_deref(),
        ),
        Some(AxonCommand::Remote(command)) => {
            run_remote(command, cli.global.format, cli.global.config.as_deref())
        }
        Some(AxonCommand::Lsp(command)) => {
            run_lsp(command, cli.global.format, cli.global.cwd.as_deref())
        }
        Some(AxonCommand::Debug(command)) => run_debug(
            command,
            cli.global.format,
            cli.global.config.as_deref(),
            cli.global.cwd.as_deref(),
        ),
        Some(AxonCommand::GovernanceWorker(_)) => unreachable!(),
        _ => run_planned_invocation(cli),
    }
}

fn run_command_from_cli(
    cli: &AxonCli,
    command: &RunCommand,
    run_json_writer: Option<&mut dyn IoWrite>,
) -> CliRunResult {
    let mut command = command.clone();
    command.format = Some(command.format.unwrap_or(cli.global.format));
    if command.format == Some(OutputFormat::Text) {
        return run_cli_daemon_interaction(cli, &command, run_json_writer);
    }
    let mut result = run_cli_daemon_interaction(cli, &command, None);
    if command.format == Some(OutputFormat::Json)
        && let Some(writer) = run_json_writer
    {
        let write_result = writer
            .write_all(result.stdout.as_bytes())
            .and_then(|()| writer.flush());
        result.stdout.clear();
        if let Err(error) = write_result {
            result.exit_code = 1;
            result.stderr = format!("Daemon JSONL 输出写入失败：{error}");
        }
    }
    result
}

fn run_stop(
    command: &StopCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    match request_stop(&path, command.session.as_deref()) {
        Ok(receipt) => match format {
            OutputFormat::Text => CliRunResult {
                exit_code: 0,
                stdout: format!("{}\n- session: {}\n", receipt.message, receipt.session_id),
                stderr: String::new(),
            },
            OutputFormat::Json => json_result(&receipt),
        },
        Err(error) => cli_error(error),
    }
}

fn setup_guide_result(cli: &AxonCli) -> Option<CliRunResult> {
    setup_check::check_and_guide_setup(cli)
}

fn run_management_command(cli: &AxonCli) -> Option<CliRunResult> {
    match &cli.command {
        Some(AxonCommand::Models(command)) => Some(run_models(
            command,
            command.format.unwrap_or(cli.global.format),
            cli.global.config.as_deref(),
        )),
        Some(AxonCommand::Providers(command)) => Some(run_providers(
            command,
            command.format.unwrap_or(cli.global.format),
            cli.global.config.as_deref(),
        )),
        Some(AxonCommand::Setup(command)) => Some(run_setup(
            command,
            command.format.unwrap_or(cli.global.format),
            cli.global.config.as_deref(),
        )),
        Some(AxonCommand::Auth(command)) => Some(run_auth(
            command,
            command.format.unwrap_or(cli.global.format),
            cli.global.config.as_deref(),
        )),
        Some(AxonCommand::Mcp(command)) => Some(run_mcp(
            command,
            cli.global.format,
            cli.global.config.as_deref(),
            cli.global.cwd.as_deref(),
        )),
        Some(AxonCommand::Skills(command)) => Some(run_skills(
            command,
            cli.global.format,
            cli.global.config.as_deref(),
            cli.global.cwd.as_deref(),
        )),
        Some(AxonCommand::Config(command)) => Some(run_config(
            command,
            cli.global.format,
            cli.global.config.as_deref(),
        )),
        _ => None,
    }
}

/// 从配置初始化日志系统。
fn init_logger_from_config(cli: &AxonCli) -> axon_domain::AxonResult<()> {
    // 优先使用显式全局配置；Daemon/内部 Worker 使用自身的嵌套配置路径。
    let config_override = cli
        .global
        .config
        .as_deref()
        .or_else(|| command_config_path(cli));
    let Ok(config_path) = setup_commands::resolved_setup_config_path(config_override) else {
        // 配置路径无效，使用默认日志配置
        return init_default_logger();
    };
    init_logger_for_config_path(&config_path)
}

/// 按已解析配置路径初始化日志系统。
///
/// Windows Daemon 在通用 CLI 流程之前进入专用入口，必须显式调用本函数，
/// 否则 Daemon 日志（含 Primary Turn 拒绝原因）会整体丢失。
pub(crate) fn init_logger_for_config_path(config_path: &Path) -> axon_domain::AxonResult<()> {
    // 确定日志目录（即使配置加载失败也要创建）
    let log_dir = log_dir_for_config_path(config_path)?;
    let configured_log_level =
        ConfigLoader::load_from_path(config_path).map(|loaded| loaded.config.agent.log_level);
    axon_session_runtime::configure_first_token_telemetry(
        configured_log_level
            .as_ref()
            .is_ok_and(|level| config_log_level_enables_first_token_telemetry(level)),
    );

    // 显式环境变量是日志诊断开关，优先级高于配置文件，但不改变遥测策略。
    let log_level = env_log_level().unwrap_or_else(|| match &configured_log_level {
        Ok(level) => level.parse().unwrap_or(axon_logger::LogLevel::Warning),
        Err(_) => axon_logger::LogLevel::Error,
    });

    axon_logger::init_logger(axon_logger::LoggerConfig {
        level: log_level,
        log_dir,
        log_file: env_log_file(),
    })
}

/// config.toml 中启用完整遥测的日志级别。
const CONFIG_TRACE_LOG_LEVEL: &str = "trace";
/// 日志级别覆盖环境变量；文档与 `.env.example` 承诺的显式诊断开关。
const LOG_LEVEL_ENV: &str = "AXON_LOG_LEVEL";
/// 日志文件覆盖环境变量；设置后按该路径单文件写入，不再按日期滚动。
#[cfg(not(test))]
const LOG_FILE_ENV: &str = "AXON_LOG_FILE";

/// 判断 config.toml 日志级别是否启用首字遥测。
fn config_log_level_enables_first_token_telemetry(level: &str) -> bool {
    level.eq_ignore_ascii_case(CONFIG_TRACE_LOG_LEVEL)
}

/// 读取环境变量覆盖的日志级别；未设置或取值非法时返回 `None`。
fn env_log_level() -> Option<axon_logger::LogLevel> {
    std::env::var(LOG_LEVEL_ENV).ok()?.trim().parse().ok()
}

/// 读取环境变量覆盖的日志文件路径；未设置或为空时返回 `None`。
fn env_log_file() -> Option<PathBuf> {
    #[cfg(test)]
    {
        // 测试不得通过生产日志文件覆盖变量写入用户目录。
        None
    }

    #[cfg(not(test))]
    {
        let value = std::env::var(LOG_FILE_ENV).ok()?;
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    }
}

fn command_config_path(cli: &AxonCli) -> Option<&Path> {
    match cli.command.as_ref()? {
        AxonCommand::Daemon(command) => match &command.action {
            DaemonAction::Run(command) => Some(command.config_path.as_path()),
            _ => None,
        },
        AxonCommand::GovernanceWorker(command) => Some(command.config_path.as_path()),
        _ => None,
    }
}

/// 使用默认配置初始化日志系统。
fn init_default_logger() -> axon_domain::AxonResult<()> {
    let log_dir = default_log_dir();
    axon_session_runtime::configure_first_token_telemetry(false);

    axon_logger::init_logger(axon_logger::LoggerConfig {
        level: env_log_level().unwrap_or(axon_logger::LogLevel::Warning),
        log_dir,
        log_file: env_log_file(),
    })
}

/// 解析日志目录。
///
/// 测试一律写入隔离目录：`config_path` 会解析到真实 `~/.axon`，而单元测试完整跑通
/// CLI 初始化流程，若不隔离就会把测试内容追加进**用户日志**。
/// 生产路径保持原语义，由 [`init_logger_for_config_path`] 调用。
fn log_dir_for_config_path(config_path: &Path) -> axon_domain::AxonResult<PathBuf> {
    #[cfg(test)]
    {
        let _ = config_path;
        Ok(default_log_dir())
    }

    #[cfg(not(test))]
    {
        Ok(axon_config::runtime_axon_root_path(config_path)?
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(axon_config::AXON_RUNTIME_LOG_DIR))
    }
}

/// 测试日志目录前缀；日志目录同样并入测试线程隔离根，随线程结束自动删除。
#[cfg(test)]
const TEST_LOG_DIR_PREFIX: &str = "axon-test-logs";

fn default_log_dir() -> std::path::PathBuf {
    #[cfg(test)]
    {
        axon_test_support::test_temp_path(TEST_LOG_DIR_PREFIX)
    }

    #[cfg(not(test))]
    {
        let home_dir = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_owned());
        std::path::PathBuf::from(home_dir)
            .join(axon_config::USER_AXON_DIR)
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(axon_config::AXON_RUNTIME_LOG_DIR)
    }
}

#[cfg(test)]
mod default_log_dir_tests {
    use std::path::Path;

    use super::{
        config_log_level_enables_first_token_telemetry, default_log_dir, log_dir_for_config_path,
    };

    /// 生产日志目录，用作隔离断言的对照基准。
    fn production_log_dir() -> std::path::PathBuf {
        let user_home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .expect("测试环境应提供用户目录");
        std::path::PathBuf::from(user_home)
            .join(axon_config::USER_AXON_DIR)
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(axon_config::AXON_RUNTIME_LOG_DIR)
    }

    #[test]
    fn only_config_trace_level_enables_first_token_telemetry() {
        assert!(config_log_level_enables_first_token_telemetry("trace"));
        assert!(config_log_level_enables_first_token_telemetry("TRACE"));
        assert!(!config_log_level_enables_first_token_telemetry("debug"));
        assert!(!config_log_level_enables_first_token_telemetry(" trace "));
    }

    /// **两条**日志初始化路径都必须隔离。
    ///
    /// 本测试是补写的：原守卫只覆盖 `default_log_dir`，而实际写穿用户目录的是
    /// `init_logger_for_config_path` 这条路径，因此「已隔离」的结论当时并不成立。
    #[test]
    fn every_log_dir_resolution_is_isolated_from_user_home() {
        let production = production_log_dir();
        let resolutions = [
            ("default_log_dir", default_log_dir()),
            (
                "log_dir_for_config_path",
                log_dir_for_config_path(Path::new("isolated/config.toml"))
                    .expect("测试下必须能解析日志目录"),
            ),
        ];

        for (name, log_dir) in resolutions {
            assert_ne!(log_dir, production, "{name} 不得指向用户日志目录");
            assert!(
                log_dir.starts_with(std::env::temp_dir()),
                "{name} 必须落在临时目录：{}",
                log_dir.display()
            );
        }
    }
}

#[cfg(test)]
include!("tests/mod.rs");
