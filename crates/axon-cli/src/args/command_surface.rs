//! CLI 命令面中与治理/LSP 相关的参数定义。

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::session_commands::SessionCommand;

/// `axon doctor` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DoctorCommand {
    /// 只检查指定模块。
    #[arg(long)]
    pub check: Option<String>,
    /// 展示运行态推导值。
    #[arg(long)]
    pub verbose: bool,
    /// 修复可安全自动修复的本地运行环境问题。
    #[arg(long)]
    pub fix: bool,
    /// 外部真实 provider 恢复验收回执 JSON 路径，仅用于 provider-recovery。
    #[arg(long)]
    pub provider_receipt: Option<PathBuf>,
    /// 生成指定 doctor 检查项的回执 JSON 模板。
    #[arg(long)]
    pub receipt_template: Option<PathBuf>,
}

/// `axon upgrade` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct UpgradeCommand {
    /// release manifest URL；默认读取 GitHub latest release。
    #[arg(long)]
    pub manifest_url: Option<String>,
    /// 只下载 release asset，不执行、不替换当前二进制。
    #[arg(long)]
    pub download: bool,
    /// 下载并发布新版本；当前会话继续运行，后续会话使用新版本。
    #[arg(long)]
    pub apply: bool,
    /// 回滚到上一个已验证版本。
    #[arg(long)]
    pub rollback: bool,
    /// 下载输出目录；省略时使用当前工作目录。
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
    /// 多 asset release 必须显式指定名称。
    #[arg(long)]
    pub asset: Option<String>,
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<super::OutputFormat>,
}

/// `axon stop` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct StopCommand {
    /// 指定正在运行的 session；省略时仅允许存在一个活动 session。
    #[arg(long)]
    pub session: Option<String>,
}

/// `axon debug` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DebugCommand {
    /// debug 子命令；省略时输出原有总览报告。
    #[command(subcommand)]
    pub command: Option<DebugSubcommand>,
}

/// `axon debug` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum DebugSubcommand {
    /// 直接触发 builtin 工具，绕过模型/provider 循环。
    Builtin(DebugBuiltinCommand),
    /// 查询本地持久化的渠道性能与缓存指标。
    ChannelMetrics(DebugChannelMetricsCommand),
    /// 查询跨轮 mailbox 的消息与投递状态。
    Mailbox(DebugMailboxCommand),
    /// 管理与诊断本地 Thread 会话。
    Session(SessionCommand),
    /// 查询实时可观测性内存滑动窗口与分钟级聚合。
    RealtimeMetrics(DebugRealtimeMetricsCommand),
}

/// `axon debug realtime-metrics` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DebugRealtimeMetricsCommand {
    /// 观察窗口时长（分钟：15, 30 或 60），默认 15。
    #[arg(long, default_value_t = 15)]
    pub window: u32,
    /// 包含最近 N 条分钟持久化聚合桶摘要，默认 10。
    #[arg(long, default_value_t = 10)]
    pub minute_limit: usize,
    /// 可选：按时间戳物理重建指定分钟桶（用于一致性自愈修复）。
    #[arg(long)]
    pub rebuild_bucket: Option<u64>,
}

/// `axon debug mailbox` 参数。
///
/// 只读查询：不投递、不重试、不改状态。默认只列待投递的消息，因为那是最需要
/// 被观察的一类（合同 §5.3.2 要求投递失败进入可观察的待投递状态）。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DebugMailboxCommand {
    /// 按根会话作用域过滤；省略表示不限作用域。
    #[arg(long)]
    pub owner: Option<String>,
    /// 按接收方 AgentPath 过滤。
    #[arg(long)]
    pub recipient: Option<String>,
    /// 按投递状态过滤；默认只列待投递的消息。
    #[arg(long, default_value = "pending")]
    pub state: DebugMailboxState,
    /// 最多输出多少条消息。
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
}

/// `axon debug mailbox --state` 取值。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum DebugMailboxState {
    /// 待投递：目标暂时不可接收，等待重试。
    #[default]
    Pending,
    /// 已投递到目标 mailbox，但尚未被读取。
    Delivered,
    /// 已被目标代理读取（读确认）；读确认与投递确认分离。
    Read,
    /// 越过保留窗口或达到尝试上限，不再重试。
    Expired,
    /// 不限状态，列出全部消息。
    Any,
}

impl DebugMailboxState {
    /// 是否限定具体状态；`Any` 表示不加状态条件。
    #[must_use]
    pub const fn is_any(self) -> bool {
        matches!(self, Self::Any)
    }

    /// 持久化取值；`Any` 没有对应取值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Expired => "expired",
            Self::Any => "any",
        }
    }
}

/// channel-metrics debug 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DebugChannelMetricsCommand {
    /// 按路由模型名过滤。
    #[arg(long)]
    pub model: Option<String>,
    /// 按 Provider 名称过滤。
    #[arg(long)]
    pub provider: Option<String>,
    /// 读取最近多少小时；设置为 0 表示不按时间过滤。
    #[arg(long, default_value_t = 24)]
    pub since_hours: u64,
    /// 最多输出多少个渠道。
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

/// `axon debug builtin` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DebugBuiltinCommand {
    /// builtin 工具子命令。
    #[command(subcommand)]
    pub command: DebugBuiltinSubcommand,
}

/// `axon debug builtin` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum DebugBuiltinSubcommand {
    /// 列出 debug 可直接触发的 builtin 工具。
    List,
    /// 执行单个 builtin 工具。
    Run(DebugBuiltinRunCommand),
}

/// `axon debug builtin run` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DebugBuiltinRunCommand {
    /// 工具名，必须使用精确 qualified name，例如 `builtin.search`。
    pub tool: String,
    /// 工具参数；JSON Schema 工具传 JSON object，freeform 工具传 JSON string。
    #[arg(long)]
    pub args: String,
    /// 执行超时秒数。
    #[arg(long, default_value_t = 30)]
    pub timeout_secs: u64,
}

/// `axon lsp` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct LspCommand {
    /// LSP 子命令。
    #[command(subcommand)]
    pub command: LspSubcommand,
}

/// `axon lsp` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum LspSubcommand {
    /// 查看当前构建支持的 OpenCode 对齐 LSP catalog 与工作区可启动计划。
    Servers(LspServersCommand),
    /// 执行 `workspace/symbol` 查询。
    WorkspaceSymbol(LspWorkspaceSymbolCommand),
    /// 执行 `textDocument/definition` 查询。
    Definition(LspPositionCommand),
    /// 执行 `textDocument/references` 查询。
    References(LspPositionCommand),
    /// 执行 document/workspace diagnostics 查询。
    Diagnostics(LspDiagnosticsCommand),
}

/// `axon lsp servers` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct LspServersCommand {
    /// 工作区根目录；省略时优先使用全局 `--cwd`，再回退到当前目录。
    #[arg(long)]
    pub workspace_root: Option<PathBuf>,
}

/// `axon lsp workspace-symbol` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct LspWorkspaceSymbolCommand {
    /// symbol 查询关键字。
    pub query: String,
    /// 工作区根目录；省略时优先使用全局 `--cwd`，再回退到当前目录。
    #[arg(long)]
    pub workspace_root: Option<PathBuf>,
    /// 仅展示将要调用哪些 server，不真正启动 LSP。
    #[arg(long)]
    pub plan_only: bool,
}

/// 文档位置类查询参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct LspPositionCommand {
    /// 目标文件路径。
    #[arg(long)]
    pub file: PathBuf,
    /// 0-based 行号。
    #[arg(long)]
    pub line: u32,
    /// 0-based 列号。
    #[arg(long)]
    pub character: u32,
    /// 工作区根目录；省略时优先使用全局 `--cwd`，再回退到当前目录。
    #[arg(long)]
    pub workspace_root: Option<PathBuf>,
    /// 仅展示将要调用哪些 server，不真正启动 LSP。
    #[arg(long)]
    pub plan_only: bool,
}

/// diagnostics 查询模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LspDiagnosticsModeArg {
    /// `textDocument/diagnostic`
    Document,
    /// `workspace/diagnostic`
    Workspace,
}

/// `axon lsp diagnostics` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct LspDiagnosticsCommand {
    /// diagnostics 查询模式。
    #[arg(long, value_enum, default_value_t = LspDiagnosticsModeArg::Document)]
    pub mode: LspDiagnosticsModeArg,
    /// 目标文件路径；`document` 模式必填，`workspace` 模式可省略。
    #[arg(long)]
    pub file: Option<PathBuf>,
    /// 工作区根目录；省略时优先使用全局 `--cwd`，再回退到当前目录。
    #[arg(long)]
    pub workspace_root: Option<PathBuf>,
    /// 仅展示将要调用哪些 server，不真正启动 LSP。
    #[arg(long)]
    pub plan_only: bool,
}
