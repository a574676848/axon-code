//! Commit Message Lint - 提交消息规范化验证器。
//!
//! 验证提交消息符合 Conventional Commits 规范。
//!
//! 核心能力：
//! - `LintResult` - 验证结果
//! - `CommitMessageLint` - 验证器
//! - 自动修复建议
//! - 语义化错误类型（对标 codex GitToolingError）
//! - 严格模式与宽松模式切换
//! - 预设规则集（Angular/Karma/Conventional Commits）

use thiserror::Error;

#[path = "commit_message_lint_rules.rs"]
mod commit_message_lint_rules;

/// Conventional Commits 规范定义。
pub const CONVENTIONAL_COMMITS_PATTERN: &str =
    r"^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\(.+\))?(!)?:\s+.+";

/// 破坏性变更标记模式。
pub const BREAKING_CHANGE_FOOTER: &str = "BREAKING CHANGE:";

/// 规则预设类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LintPreset {
    /// Conventional Commits 标准
    #[default]
    ConventionalCommits,
    /// Angular 规范
    Angular,
    /// 宽松模式
    Permissive,
    /// 严格模式（所有警告视为错误）
    Strict,
}

/// Lint 错误类型，对标 codex GitToolingError
#[derive(Debug, Error)]
pub enum LintError {
    /// 空提交消息
    #[error("空提交消息")]
    EmptyMessage,
    /// 格式不符合规范
    #[error("格式不符合规范: {0}")]
    InvalidFormat(String),
    /// 无效的类型
    #[error("无效的类型 '{0}'，有效类型: {1}")]
    InvalidType(String, String),
    /// 描述缺失或过短
    #[error("描述缺失或过短")]
    MissingDescription,
    /// 主题行过长
    #[error("主题行过长 ({actual} > {max})")]
    SubjectTooLong {
        /// 实际长度
        actual: usize,
        /// 最大长度
        max: usize,
    },
    /// 正则匹配失败
    #[error("正则匹配失败: {0}")]
    RegexError(String),
}

/// 验证级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LintLevel {
    /// 提示级别（最低）
    Hint,
    /// 警告级别
    Warning,
    /// 错误级别（最高）
    Error,
}

/// 验证问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintIssue {
    /// 级别
    pub level: LintLevel,
    /// 代码
    pub code: LintCode,
    /// 消息
    pub message: String,
    /// 行号
    pub line: Option<usize>,
    /// 建议
    pub suggestion: Option<String>,
}

impl LintIssue {
    /// 是否为错误级别
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.level == LintLevel::Error
    }

    /// 是否为警告级别
    #[must_use]
    pub fn is_warning(&self) -> bool {
        self.level == LintLevel::Warning
    }

    /// 是否为提示级别
    #[must_use]
    pub fn is_hint(&self) -> bool {
        self.level == LintLevel::Hint
    }
}

/// 验证问题代码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintCode {
    /// 格式不符合规范
    InvalidFormat,
    /// 类型缺失或无效
    MissingOrInvalidType,
    /// 描述缺失或过短
    MissingOrShortDescription,
    /// 描述以小写开头
    DescriptionStartsLowercase,
    /// 描述以句号结尾
    DescriptionEndsWithPeriod,
    /// 描述过长
    DescriptionTooLong,
    /// 范围格式无效
    InvalidScope,
    /// 破坏性变更标记不一致
    BreakingChangeInconsistency,
    /// 正文与描述相同
    BodySameAsSubject,
    /// 多余的空行
    ExtraBlankLine,
}

/// 验证结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintResult {
    /// 是否有效
    pub is_valid: bool,
    /// 问题列表
    pub issues: Vec<LintIssue>,
    /// 修复后的消息
    pub fixed_message: Option<String>,
}

impl Default for LintResult {
    fn default() -> Self {
        Self {
            is_valid: true,
            issues: Vec::new(),
            fixed_message: None,
        }
    }
}

/// 提交消息验证器。
#[derive(Debug, Clone)]
pub struct CommitMessageLint {
    pub(super) max_subject_length: usize,
    pub(super) max_body_line_length: usize,
    pub(super) allow_breaking_change_mark: bool,
    /// 严格模式：警告视为错误
    pub(super) strict_mode: bool,
}

#[cfg(test)]
mod commit_message_lint_tests;
