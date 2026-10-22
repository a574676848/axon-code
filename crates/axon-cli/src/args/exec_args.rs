use clap::{Args, ValueEnum};

/// `axon exec` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ExecCommand {
    /// 超时时间，单位毫秒。
    #[arg(long)]
    pub timeout_ms: Option<u64>,
    /// 执行类型。test 会在回执中标记测试编排结果。
    #[arg(long, value_enum, default_value_t = ExecKind::Command)]
    pub kind: ExecKind,
    /// 需要执行的命令。
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// `axon exec` 执行类型。
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ExecKind {
    /// 普通本机命令。
    Command,
    /// 测试命令，回执会携带 testPassed。
    Test,
}
