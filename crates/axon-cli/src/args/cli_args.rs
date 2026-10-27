//! CLI 参数类型与调用计划。

use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

use super::{
    AuthCommand, CompletionCommand, ConfigCommand, CronCommand, DaemonCommand, DebugCommand,
    DoctorCommand, ExecCommand, ExportCommand, LspCommand, McpCommand, MemoryCommand,
    ModelsCommand, PackageCommand, PersonaCommand, ProvidersCommand, RemoteCommand, SetupCommand,
    SkillsCommand, StopCommand, UpgradeCommand, UsageCommand,
};

/// Axon CLI 顶层参数。
#[derive(Debug, Clone, Parser, PartialEq, Eq)]
#[command(
    author,
    version,
    bin_name = "axon",
    subcommand_negates_reqs = true,
    override_usage = "axon [OPTIONS] [PROMPT]\n       axon <COMMAND> [ARGS]"
)]
pub struct AxonCli {
    /// 全局选项。
    #[command(flatten)]
    pub global: GlobalOptions,

    /// 初始 prompt；无子命令时作为本轮 Agent 请求执行。
    pub prompt: Option<String>,

    /// 子命令。
    #[command(subcommand)]
    pub command: Option<AxonCommand>,
}

impl AxonCli {
    /// 解析命令行参数。
    #[must_use]
    pub fn parse_from_args<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        Self::parse_from(args)
    }
}

/// 全局选项。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct GlobalOptions {
    /// 配置文件路径。
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// 工作目录。
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// 日志级别。
    #[arg(long, value_enum, default_value_t = LogLevel::Warn)]
    pub log_level: LogLevel,
    /// 日志文件路径。
    #[arg(long)]
    pub log_file: Option<PathBuf>,
    /// 结构化输出格式。
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
}

/// 日志级别。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    /// trace。
    Trace,
    /// debug。
    Debug,
    /// info。
    Info,
    /// warn。
    #[default]
    Warn,
    /// error。
    Error,
}

/// 输出格式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// 人类可读文本。
    #[default]
    Text,
    /// JSON 或 JSONL。
    Json,
}

/// 持久化 post-run governance worker 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct GovernanceWorkerCommand {
    /// 原始 run 使用的配置文件路径。
    #[arg(long = "config-path")]
    pub config_path: PathBuf,
}

/// Axon 子命令注册表。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum AxonCommand {
    /// 执行持久化 post-run governance 工作单元。
    #[command(name = "__governance-worker", hide = true)]
    GovernanceWorker(GovernanceWorkerCommand),
    /// 执行一次 Coding Agent 任务。
    Run(RunCommand),
    /// 停止正在运行的会话。
    Stop(StopCommand),
    /// 启动 headless HTTP server。
    Serve(ServeCommand),
    /// 执行本机命令。
    #[command(visible_alias = "e")]
    Exec(ExecCommand),
    /// 列出可用模型。
    Models(ModelsCommand),
    /// 管理 provider 配置。
    Providers(ProvidersCommand),
    /// 初始化本机配置。
    Setup(SetupCommand),
    /// 管理本机认证状态。
    Auth(AuthCommand),
    /// MCP server 管理。
    Mcp(McpCommand),
    /// 查看已加载 skill。
    Skills(SkillsCommand),
    /// 配置管理。
    Config(ConfigCommand),
    /// 诊断与健康检查。
    Doctor(DoctorCommand),
    /// 生成 shell 补全脚本。
    Completion(CompletionCommand),
    /// 检查更新。
    Upgrade(UpgradeCommand),
    /// 生成 portable release 包。
    Package(PackageCommand),
    /// 导出数据。
    Export(ExportCommand),
    /// 人格系统：校验与持久化状态。
    Persona(PersonaCommand),
    /// 记忆管理。
    Memory(MemoryCommand),
    /// 通用后台 Daemon Host 与 Session IPC。
    Daemon(DaemonCommand),
    /// 管理用户级 Agent Cron。
    Cron(CronCommand),
    /// 查询本机使用统计看板数据。
    Usage(UsageCommand),
    /// 管理与核算模型费率规则。
    Pricing(crate::usage_statistics::PricingCommand),
    /// 远控治理与诊断。
    Remote(RemoteCommand),
    /// 调试工具。
    Debug(DebugCommand),
    /// LSP / diagnostics 查询入口。
    Lsp(LspCommand),
}

/// `axon run` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RunCommand {
    /// 发送给 Agent 的提示文本。
    pub prompt: String,
    /// 模型路由键或模型 ID。
    #[arg(long)]
    pub model: Option<String>,
    /// 本轮模型思考强度，覆盖模型配置中的 reasoning_effort。
    #[arg(long = "reasoning-effort")]
    pub reasoning_effort: Option<String>,
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    /// 继续最近一次会话。
    #[arg(long = "continue")]
    pub continue_last: bool,
    /// 指定会话 ID。
    #[arg(long)]
    pub session: Option<String>,
    /// 附带文件路径。
    #[arg(long)]
    pub file: Vec<PathBuf>,
    /// Provider 流式响应空闲超时秒数。
    #[arg(long)]
    pub timeout: Option<u64>,
    /// 限制可用工具。
    #[arg(long)]
    pub tool: Vec<String>,
}

/// `axon serve` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ServeCommand {
    /// 绑定地址。
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// 端口。
    #[arg(long, default_value_t = 3000)]
    pub port: u16,
    /// Bearer token 文件路径。
    #[arg(long)]
    pub auth_token_file: Option<PathBuf>,
    /// 测试用：处理指定请求数后退出。
    #[arg(long, hide = true)]
    pub max_requests: Option<usize>,
}

/// 已注册的子命令名称。
#[must_use]
pub fn registered_command_names() -> Vec<String> {
    AxonCli::command()
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .map(|command| command.get_name().to_owned())
        .collect()
}

/// CLI 调用计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliInvocation {
    /// 无子命令时进入主入口。
    NoCommand {
        /// 初始 prompt。
        prompt: Option<String>,
    },
    /// 执行已注册子命令。
    Command {
        /// 子命令名称。
        name: &'static str,
    },
}

/// 将解析结果转成运行计划。
#[must_use]
pub fn plan_invocation(cli: &AxonCli) -> CliInvocation {
    match &cli.command {
        None => CliInvocation::NoCommand {
            prompt: cli.prompt.clone(),
        },
        Some(AxonCommand::Run(_)) => CliInvocation::Command { name: "run" },
        Some(AxonCommand::Stop(_)) => CliInvocation::Command { name: "stop" },
        Some(AxonCommand::Serve(_)) => CliInvocation::Command { name: "serve" },
        Some(AxonCommand::Exec(_)) => CliInvocation::Command { name: "exec" },
        Some(AxonCommand::Models(_)) => CliInvocation::Command { name: "models" },
        Some(AxonCommand::Providers(_)) => CliInvocation::Command { name: "providers" },
        Some(AxonCommand::Setup(_)) => CliInvocation::Command { name: "setup" },
        Some(AxonCommand::Auth(_)) => CliInvocation::Command { name: "auth" },
        Some(AxonCommand::Mcp(_)) => CliInvocation::Command { name: "mcp" },
        Some(AxonCommand::Skills(_)) => CliInvocation::Command { name: "skills" },
        Some(AxonCommand::Config(_)) => CliInvocation::Command { name: "config" },
        Some(AxonCommand::Doctor(_)) => CliInvocation::Command { name: "doctor" },
        Some(AxonCommand::Completion(_)) => CliInvocation::Command { name: "completion" },
        Some(AxonCommand::Upgrade(_)) => CliInvocation::Command { name: "upgrade" },
        Some(AxonCommand::Package(_)) => CliInvocation::Command { name: "package" },
        Some(AxonCommand::Export(_)) => CliInvocation::Command { name: "export" },
        Some(AxonCommand::Persona(_)) => CliInvocation::Command { name: "persona" },
        Some(AxonCommand::Memory(_)) => CliInvocation::Command { name: "memory" },
        Some(AxonCommand::Daemon(_)) => CliInvocation::Command { name: "daemon" },
        Some(AxonCommand::Cron(_)) => CliInvocation::Command { name: "cron" },
        Some(AxonCommand::Usage(_)) => CliInvocation::Command { name: "usage" },
        Some(AxonCommand::Pricing(_)) => CliInvocation::Command { name: "pricing" },
        Some(AxonCommand::Remote(_)) => CliInvocation::Command { name: "remote" },
        Some(AxonCommand::Debug(_)) => CliInvocation::Command { name: "debug" },
        Some(AxonCommand::Lsp(_)) => CliInvocation::Command { name: "lsp" },
        Some(AxonCommand::GovernanceWorker(_)) => CliInvocation::Command {
            name: "__governance-worker",
        },
    }
}

/// CLI 运行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliRunResult {
    /// 进程退出码。
    pub exit_code: u8,
    /// stdout 内容。
    pub stdout: String,
    /// stderr 内容。
    pub stderr: String,
}
