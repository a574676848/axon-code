use clap::{Args, ValueEnum};

/// `axon completion` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct CompletionCommand {
    /// shell 类型。
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

/// shell 补全类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompletionShell {
    /// PowerShell。
    PowerShell,
    /// Bash。
    Bash,
    /// Zsh。
    Zsh,
    /// Fish。
    Fish,
}
