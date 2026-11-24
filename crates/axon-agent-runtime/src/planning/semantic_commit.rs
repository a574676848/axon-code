//! Semantic Commit Generator - 语义化提交生成器。
//!
//! 基于 Conventional Commits 规范生成语义化提交信息。
//!
//! 核心能力：
//! - `SemanticCommitType` - 标准提交类型枚举
//! - `CommitScope` - 变更范围提取
//! - `SemanticCommit` - 完整语义化提交结构
//! - 自动范围检测与类型推断

/// Conventional Commits 标准类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticCommitType {
    /// 新功能
    Feat,
    /// Bug 修复
    Fix,
    /// 文档变更
    Docs,
    /// 样式调整（不影响代码含义）
    Style,
    /// 代码重构（不影响功能）
    Refactor,
    /// 性能优化
    Perf,
    /// 测试添加
    Test,
    /// 构建流程或辅助工具变更
    Build,
    /// CI/CD 配置变更
    Ci,
    /// 权限/角色变更
    Chore,
    /// 事务回滚
    Revert,
}

impl SemanticCommitType {
    /// 从文件变更模式推断提交类型。
    #[must_use]
    pub fn infer_from_changes(change_patterns: &[ChangePattern]) -> Self {
        if change_patterns.is_empty() {
            return SemanticCommitType::Chore;
        }

        let has_test = change_patterns
            .iter()
            .any(|p| matches!(p, ChangePattern::TestFile(_)));
        let has_docs = change_patterns
            .iter()
            .any(|p| matches!(p, ChangePattern::Documentation(_)));
        let has_build = change_patterns
            .iter()
            .any(|p| matches!(p, ChangePattern::BuildConfig(_)));
        let has_ci = change_patterns
            .iter()
            .any(|p| matches!(p, ChangePattern::CiConfig(_)));

        if has_test && !has_docs && !has_build && !has_ci {
            return SemanticCommitType::Test;
        }
        if has_docs && !has_test && !has_build && !has_ci {
            return SemanticCommitType::Docs;
        }
        if has_ci && !has_docs && !has_test && !has_build {
            return SemanticCommitType::Ci;
        }
        if has_build && !has_docs && !has_test && !has_ci {
            return SemanticCommitType::Build;
        }

        SemanticCommitType::Feat
    }

    /// 返回类型的标准前缀。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Feat => "feat",
            Self::Fix => "fix",
            Self::Docs => "docs",
            Self::Style => "style",
            Self::Refactor => "refactor",
            Self::Perf => "perf",
            Self::Test => "test",
            Self::Build => "build",
            Self::Ci => "ci",
            Self::Chore => "chore",
            Self::Revert => "revert",
        }
    }
}

impl core::fmt::Display for SemanticCommitType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 变更文件模式（用于类型推断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangePattern {
    /// 测试文件
    TestFile(String),
    /// 文档文件
    Documentation(String),
    /// 构建配置
    BuildConfig(String),
    /// CI 配置
    CiConfig(String),
    /// 普通源文件
    SourceFile(String),
}

impl ChangePattern {
    /// 从文件路径识别变更模式。
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        let path_lower = path.to_lowercase();
        if path_lower.contains("test")
            || path_lower.contains("_test")
            || path_lower.ends_with(".test.rs")
        {
            return ChangePattern::TestFile(path.to_string());
        }
        if path_lower.contains("readme")
            || path_lower.contains("changelog")
            || path_lower.ends_with(".md")
        {
            return ChangePattern::Documentation(path.to_string());
        }
        if path_lower.contains("cargo")
            || path_lower.contains("build.rs")
            || path_lower.contains("cmake")
            || path_lower.contains("package.json")
        {
            return ChangePattern::BuildConfig(path.to_string());
        }
        if path_lower.contains(".github")
            || path_lower.contains("gitlab-ci")
            || path_lower.contains("jenkins")
            || path_lower.contains(".circleci")
        {
            return ChangePattern::CiConfig(path.to_string());
        }
        ChangePattern::SourceFile(path.to_string())
    }
}

/// 提交范围（模块/组件）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitScope {
    /// 范围名称
    pub name: String,
    /// 可选的子范围
    pub subscope: Option<String>,
}

impl CommitScope {
    /// 从文件路径列表提取公共范围。
    #[must_use]
    pub fn extract_from_paths(paths: &[String]) -> Option<Self> {
        if paths.is_empty() {
            return None;
        }

        // 尝试从路径中提取模块名
        let common_prefix = Self::longest_common_prefix(paths);
        let parts: Vec<&str> = common_prefix.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return None;
        }

        let name = parts[0].to_string();
        let subscope = parts.get(1).map(std::string::ToString::to_string);

        Some(Self { name, subscope })
    }

    /// 计算最长公共前缀。
    fn longest_common_prefix(paths: &[String]) -> String {
        if paths.is_empty() {
            return String::new();
        }
        if paths.len() == 1 {
            return paths[0].clone();
        }

        let chars: Vec<Vec<char>> = paths.iter().map(|s| s.chars().collect()).collect();

        let first = &chars[0];
        let mut result = String::new();

        for (i, &c) in first.iter().enumerate() {
            if chars.iter().all(|v| v.get(i) == Some(&c)) {
                result.push(c);
            } else {
                break;
            }
        }

        result
    }
}

impl core::fmt::Display for CommitScope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.name)?;
        if let Some(sub) = &self.subscope {
            write!(f, "({sub})")?;
        }
        Ok(())
    }
}

/// 语义化提交消息结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCommit {
    /// 提交类型
    pub commit_type: SemanticCommitType,
    /// 变更范围（可选）
    pub scope: Option<CommitScope>,
    /// 是否为破坏性变更
    pub breaking: bool,
    /// 简短描述
    pub short_description: String,
    /// 详细正文（可选）
    pub body: Option<String>,
    /// 页脚（可选，用于 BREAKING CHANGE 和 issue 引用）
    pub footer: Option<String>,
    /// 关联的 issue 列表
    pub issues: Vec<String>,
}

impl SemanticCommit {
    /// 从文件变更生成语义化提交。
    pub fn from_changes(changed_files: &[String], short_description: impl Into<String>) -> Self {
        let patterns: Vec<ChangePattern> = changed_files
            .iter()
            .map(|p| ChangePattern::from_path(p))
            .collect();

        let commit_type = SemanticCommitType::infer_from_changes(&patterns);
        let scope = CommitScope::extract_from_paths(changed_files);

        Self {
            commit_type,
            scope,
            breaking: false,
            short_description: short_description.into(),
            body: None,
            footer: None,
            issues: Vec::new(),
        }
    }

    /// 标记为破坏性变更。
    #[must_use]
    pub fn with_breaking(mut self) -> Self {
        self.breaking = true;
        self
    }

    /// 添加正文内容。
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// 添加页脚。
    pub fn with_footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    /// 添加关联 issue。
    pub fn with_issues(mut self, issues: impl IntoIterator<Item = String>) -> Self {
        self.issues.extend(issues);
        self
    }
}

impl core::fmt::Display for SemanticCommit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // 类型
        write!(f, "{}", self.commit_type)?;

        // 范围
        if let Some(scope) = &self.scope {
            write!(
                f,
                "({}{})",
                scope.name,
                scope
                    .subscope
                    .as_ref()
                    .map(|s| format!("/{s}"))
                    .unwrap_or_default()
            )?;
        }

        // 破坏性标记
        if self.breaking {
            write!(f, "!")?;
        }

        // 分隔符和描述
        write!(f, ": {}", self.short_description)?;

        // 正文
        if let Some(body) = &self.body {
            write!(f, "\n\n{body}")?;
        }

        // 页脚
        if let Some(footer) = &self.footer {
            write!(f, "\n\n{footer}")?;
        }

        Ok(())
    }
}

/// 生成 Conventional Commits 格式的提交消息。
pub fn generate_semantic_commit(
    changed_files: &[String],
    short_description: impl Into<String>,
) -> SemanticCommit {
    SemanticCommit::from_changes(changed_files, short_description)
}

/// 生成短格式提交消息（仅类型: 描述）。
#[must_use]
pub fn generate_short_commit(
    commit_type: SemanticCommitType,
    scope: Option<&str>,
    description: &str,
) -> String {
    match scope {
        Some(s) => format!("{commit_type}!({s}): {description}"),
        None => format!("{commit_type}: {description}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commit_type_inference() {
        let test_files = ["tests/test_foo.rs".to_string()];
        let patterns: Vec<ChangePattern> = test_files
            .iter()
            .map(|p| ChangePattern::from_path(p))
            .collect();
        assert_eq!(
            SemanticCommitType::infer_from_changes(&patterns),
            SemanticCommitType::Test
        );
    }

    #[test]
    fn test_change_pattern_detection() {
        assert!(matches!(
            ChangePattern::from_path("README.md"),
            ChangePattern::Documentation(_)
        ));
        assert!(matches!(
            ChangePattern::from_path(".github/workflows/ci.yml"),
            ChangePattern::CiConfig(_)
        ));
        assert!(matches!(
            ChangePattern::from_path("src/lib.rs"),
            ChangePattern::SourceFile(_)
        ));
    }

    #[test]
    fn test_scope_extraction() {
        let paths = vec![
            "crates/axon-agent/src/lib.rs".to_string(),
            "crates/axon-agent/src/main.rs".to_string(),
        ];
        let scope = CommitScope::extract_from_paths(&paths);
        assert!(scope.is_some());
        assert_eq!(scope.unwrap().name, "crates");
    }

    #[test]
    fn test_full_semantic_commit() {
        let commit =
            SemanticCommit::from_changes(&["src/foo/bar.rs".to_string()], "add new feature")
                .with_breaking()
                .with_body("This commit adds a new feature to the foo module.")
                .with_footer("BREAKING CHANGE: The old API has been removed.");

        let output = format!("{commit}");
        assert!(output.contains("feat(src/foo)!: add new feature"));
        assert!(output.contains("add new feature"));
        assert!(output.contains("BREAKING CHANGE"));
    }
}
