//! Commit message lint 规则实现。

use super::{
    BREAKING_CHANGE_FOOTER, CommitMessageLint, LintCode, LintIssue, LintLevel, LintPreset,
    LintResult,
};

impl Default for CommitMessageLint {
    fn default() -> Self {
        Self {
            max_subject_length: 72,
            max_body_line_length: 100,
            allow_breaking_change_mark: true,
            strict_mode: false,
        }
    }
}

impl CommitMessageLint {
    /// 创建验证器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用预设创建验证器。
    #[must_use]
    pub fn with_preset(preset: LintPreset) -> Self {
        match preset {
            LintPreset::ConventionalCommits | LintPreset::Angular => Self {
                max_subject_length: 72,
                max_body_line_length: 100,
                allow_breaking_change_mark: true,
                strict_mode: false,
            },
            LintPreset::Permissive => Self {
                max_subject_length: 100,
                max_body_line_length: 120,
                allow_breaking_change_mark: true,
                strict_mode: false,
            },
            LintPreset::Strict => Self {
                max_subject_length: 50,
                max_body_line_length: 80,
                allow_breaking_change_mark: false,
                strict_mode: true,
            },
        }
    }

    /// 设置最大主题长度。
    #[must_use]
    pub fn with_max_subject_length(mut self, len: usize) -> Self {
        self.max_subject_length = len;
        self
    }

    /// 设置最大正文行长度。
    #[must_use]
    pub fn with_max_body_line_length(mut self, len: usize) -> Self {
        self.max_body_line_length = len;
        self
    }

    /// 禁止破坏性变更标记。
    #[must_use]
    pub fn disallow_breaking_change_mark(mut self) -> Self {
        self.allow_breaking_change_mark = false;
        self
    }

    /// 启用严格模式（警告视为错误）。
    #[must_use]
    pub fn with_strict_mode(mut self) -> Self {
        self.strict_mode = true;
        self
    }

    /// 获取有效类型列表。
    #[must_use]
    pub fn valid_types() -> &'static [&'static str] {
        &[
            "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
            "revert",
        ]
    }

    /// 验证提交消息。
    #[must_use]
    pub fn lint(&self, message: &str) -> LintResult {
        let mut result = LintResult::default();
        let lines: Vec<&str> = message.lines().collect();

        if lines.is_empty() {
            result.is_valid = false;
            result.issues.push(LintIssue {
                level: LintLevel::Error,
                code: LintCode::MissingOrShortDescription,
                message: "提交消息不能为空".to_string(),
                line: None,
                suggestion: Some("请输入有效的提交消息".to_string()),
            });
            return result;
        }

        let subject = lines[0];
        let body: Vec<&str> = if lines.len() > 1 {
            lines[1..].to_vec()
        } else {
            Vec::new()
        };

        self.lint_subject(&mut result, subject);

        if !body.is_empty() {
            self.lint_body(&mut result, &body, subject);
        }

        self.lint_breaking_change(&mut result, &lines);

        let has_error = result.issues.iter().any(|i| i.level == LintLevel::Error);
        let has_warning = result.issues.iter().any(|i| i.level == LintLevel::Warning);
        result.is_valid = !(has_error || self.strict_mode && has_warning);

        result.fixed_message = self.try_fix(&result, &lines);

        result
    }

    /// 检查消息是否符合规范。
    #[must_use]
    pub fn is_valid(&self, message: &str) -> bool {
        self.lint(message).is_valid
    }

    fn lint_subject(&self, result: &mut LintResult, subject: &str) {
        if subject.is_empty() {
            result.issues.push(LintIssue {
                level: LintLevel::Error,
                code: LintCode::MissingOrShortDescription,
                message: "主题行不能为空".to_string(),
                line: Some(0),
                suggestion: Some("添加简短的描述".to_string()),
            });
            return;
        }

        if subject.len() > self.max_subject_length {
            result.issues.push(LintIssue {
                level: LintLevel::Warning,
                code: LintCode::DescriptionTooLong,
                message: format!(
                    "主题行长度 {} 超过建议的最大长度 {}",
                    subject.len(),
                    self.max_subject_length
                ),
                line: Some(0),
                suggestion: Some(format!(
                    "将主题行缩短至 {} 个字符以内",
                    self.max_subject_length
                )),
            });
        }

        if !subject.contains(':') {
            result.issues.push(LintIssue {
                level: LintLevel::Error,
                code: LintCode::InvalidFormat,
                message: "主题行必须包含 ':' 分隔符".to_string(),
                line: Some(0),
                suggestion: Some("使用 'type(scope): description' 格式".to_string()),
            });
            return;
        }

        let parts: Vec<&str> = subject.split(':').collect();
        if parts.is_empty() {
            result.issues.push(LintIssue {
                level: LintLevel::Error,
                code: LintCode::InvalidFormat,
                message: "主题行格式无效".to_string(),
                line: Some(0),
                suggestion: Some("使用 'type(scope): description' 格式".to_string()),
            });
            return;
        }

        let type_scope = parts[0].trim();
        let description = parts.get(1).map(|s| s.trim()).unwrap_or_default();

        self.lint_type_and_scope(result, type_scope, 0);
        self.lint_description(result, description, 0);
    }

    fn lint_type_and_scope(&self, result: &mut LintResult, type_scope: &str, line: usize) {
        let valid_types = Self::valid_types();

        let (type_part, scope_part) = if let Some(paren_pos) = type_scope.find('(') {
            let t = &type_scope[..paren_pos];
            let scope = type_scope[paren_pos..]
                .trim_start_matches('(')
                .trim_end_matches(')');
            (t, Some(scope))
        } else {
            (type_scope, None)
        };

        if !valid_types.contains(&type_part) {
            result.issues.push(LintIssue {
                level: LintLevel::Error,
                code: LintCode::MissingOrInvalidType,
                message: format!(
                    "无效的类型 '{}'，有效的类型: {}",
                    type_part,
                    valid_types.join(", ")
                ),
                line: Some(line),
                suggestion: Some(format!("使用以下类型之一: {}", valid_types.join(", "))),
            });
        }

        if let Some(scope) = scope_part {
            if scope.is_empty() {
                result.issues.push(LintIssue {
                    level: LintLevel::Warning,
                    code: LintCode::InvalidScope,
                    message: "范围不能为空".to_string(),
                    line: Some(line),
                    suggestion: Some("移除空括号或填入有效的范围".to_string()),
                });
            } else if scope.chars().any(char::is_uppercase) {
                result.issues.push(LintIssue {
                    level: LintLevel::Hint,
                    code: LintCode::InvalidScope,
                    message: "范围建议使用小写字母".to_string(),
                    line: Some(line),
                    suggestion: Some("将范围改为小写".to_string()),
                });
            }
        }
    }

    fn lint_description(&self, result: &mut LintResult, description: &str, line: usize) {
        if description.is_empty() {
            result.issues.push(LintIssue {
                level: LintLevel::Error,
                code: LintCode::MissingOrShortDescription,
                message: "描述不能为空".to_string(),
                line: Some(line),
                suggestion: Some("添加简短的描述".to_string()),
            });
            return;
        }

        if description.len() < 3 {
            result.issues.push(LintIssue {
                level: LintLevel::Warning,
                code: LintCode::MissingOrShortDescription,
                message: "描述过短（建议至少3个字符）".to_string(),
                line: Some(line),
                suggestion: Some("提供更详细的描述".to_string()),
            });
        }

        if let Some(first) = description.chars().next()
            && first.is_lowercase()
        {
            result.issues.push(LintIssue {
                level: LintLevel::Warning,
                code: LintCode::DescriptionStartsLowercase,
                message: "描述应以大写字母开头".to_string(),
                line: Some(line),
                suggestion: Some("将首字母大写".to_string()),
            });
        }

        if description.ends_with('.') {
            result.issues.push(LintIssue {
                level: LintLevel::Hint,
                code: LintCode::DescriptionEndsWithPeriod,
                message: "描述不应以句号结尾".to_string(),
                line: Some(line),
                suggestion: Some("移除末尾的句号".to_string()),
            });
        }
    }

    fn lint_body(&self, result: &mut LintResult, body: &[&str], subject: &str) {
        for (idx, line) in body.iter().enumerate() {
            if line.len() > self.max_body_line_length {
                result.issues.push(LintIssue {
                    level: LintLevel::Hint,
                    code: LintCode::DescriptionTooLong,
                    message: format!(
                        "第 {} 行长度 {} 超过建议的最大长度 {}",
                        idx + 2,
                        line.len(),
                        self.max_body_line_length
                    ),
                    line: Some(idx + 2),
                    suggestion: Some(format!(
                        "将行缩短至 {} 个字符以内",
                        self.max_body_line_length
                    )),
                });
            }

            if line.trim().is_empty() && idx > 0 && body[idx - 1].trim().is_empty() {
                result.issues.push(LintIssue {
                    level: LintLevel::Hint,
                    code: LintCode::ExtraBlankLine,
                    message: "发现多余空行".to_string(),
                    line: Some(idx + 2),
                    suggestion: Some("移除多余空行".to_string()),
                });
            }
        }

        let description = subject.split(':').nth(1).map(str::trim).unwrap_or_default();
        let body_start = body.first().map(|s| s.trim()).unwrap_or_default();
        if !body_start.is_empty() && description == body_start {
            result.issues.push(LintIssue {
                level: LintLevel::Warning,
                code: LintCode::BodySameAsSubject,
                message: "正文与主题描述相同".to_string(),
                line: Some(1),
                suggestion: Some("正文应提供更多信息，而非重复主题".to_string()),
            });
        }
    }

    fn lint_breaking_change(&self, result: &mut LintResult, lines: &[&str]) {
        let has_bang = lines[0].contains('!');
        let has_breaking_footer = lines.iter().any(|l| l.contains(BREAKING_CHANGE_FOOTER));

        if has_bang && !has_breaking_footer {
            result.issues.push(LintIssue {
                level: LintLevel::Hint,
                code: LintCode::BreakingChangeInconsistency,
                message: "建议添加 BREAKING CHANGE 页脚".to_string(),
                line: None,
                suggestion: Some(format!(
                    "在页脚添加 '{BREAKING_CHANGE_FOOTER}:' 后跟破坏性变更说明"
                )),
            });
        }

        if has_breaking_footer && !has_bang {
            result.issues.push(LintIssue {
                level: LintLevel::Hint,
                code: LintCode::BreakingChangeInconsistency,
                message: "建议在类型后添加 '!' 标记".to_string(),
                line: None,
                suggestion: Some("在冒号前添加 '!'，如 'feat!: description'".to_string()),
            });
        }

        if !self.allow_breaking_change_mark && (has_bang || has_breaking_footer) {
            result.issues.push(LintIssue {
                level: LintLevel::Error,
                code: LintCode::BreakingChangeInconsistency,
                message: "不允许破坏性变更标记".to_string(),
                line: None,
                suggestion: Some("移除 '!' 标记和 BREAKING CHANGE 页脚".to_string()),
            });
        }
    }

    fn try_fix(&self, result: &LintResult, lines: &[&str]) -> Option<String> {
        if result.issues.is_empty() {
            return None;
        }

        let mut fixed = Vec::new();
        fixed.push(lines[0].to_string());

        let first_issue = result.issues.first()?;
        if first_issue.code == LintCode::DescriptionStartsLowercase
            && let Some(rest) = lines[0].split(':').nth(1)
        {
            let Some(prefix) = lines[0].split(':').next() else {
                return Some(fixed.join("\n"));
            };
            let desc = rest.trim();
            if let Some(first) = desc.chars().next()
                && first.is_lowercase()
            {
                let remainder_start = first.len_utf8();
                let remainder = desc.get(remainder_start..).unwrap_or_default();
                let capitalized = format!("{}: {}{}", prefix, first.to_uppercase(), remainder);
                fixed[0] = capitalized;
            }
        }

        if first_issue.code == LintCode::DescriptionEndsWithPeriod
            && let Some((prefix, desc)) = lines[0].rsplit_once(": ")
            && desc.ends_with('.')
        {
            fixed[0] = format!("{}: {}", prefix, desc.trim_end_matches('.'));
        }

        for line in &lines[1..] {
            fixed.push(line.to_string());
        }

        Some(fixed.join("\n"))
    }
}
