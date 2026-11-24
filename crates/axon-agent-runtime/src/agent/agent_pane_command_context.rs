//! Agent Pane 命令上下文类型。

use serde::Serialize;

/// 最近命令状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecentCommandState {
    /// 命令文本。
    pub command: String,
    /// 退出码。
    pub exit_code: Option<i32>,
    /// 输出尾部，有界保存。
    pub output_tail: String,
    /// 执行耗时。
    pub duration_ms: Option<u64>,
    /// 检测到的命令错误。
    pub detected_errors: Vec<CommandErrorSpan>,
}

impl RecentCommandState {
    /// 构造只包含命令和退出码的最近命令。
    #[must_use]
    pub fn new(command: impl Into<String>, exit_code: Option<i32>) -> Self {
        Self {
            command: command.into(),
            exit_code,
            output_tail: String::new(),
            duration_ms: None,
            detected_errors: Vec::new(),
        }
    }
}

/// 命令错误类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CommandErrorKind {
    /// Rust 编译器错误。
    Rustc,
    /// Clang 编译器错误。
    Clang,
    /// Go 编译器错误。
    GoCompiler,
    /// PyTest 失败。
    PyTest,
    /// ESLint 失败。
    Eslint,
    /// TypeScript 编译器错误。
    TypeScript,
    /// JavaScript / TypeScript 测试失败。
    JavaScriptTest,
    /// JavaScript / TypeScript 运行时错误。
    JavaScriptRuntime,
    /// 通用错误。
    Generic,
}

/// 命令错误片段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandErrorSpan {
    /// 错误类型。
    pub kind: CommandErrorKind,
    /// 人类可读错误摘要。
    pub message: String,
    /// 出错文件路径。
    pub file: Option<String>,
    /// 出错行号。
    pub line: Option<u32>,
    /// 出错列号。
    pub column: Option<u32>,
}
