//! Auto Changelog Generator - 自动更新日志生成器。
//!
//! 基于 Conventional Commits 规范自动生成 changelog。

use axon_process::non_interactive_command;
use std::collections::HashMap;
use std::path::Path;

/// Changelog 版本条目。
#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    /// 版本号
    pub version: String,
    /// 日期
    pub date: Option<String>,
    /// 条目列表
    pub entries: Vec<ChangelogSection>,
}

/// Changelog 区块。
#[derive(Debug, Clone)]
pub struct ChangelogSection {
    /// 分区类型
    pub section_type: SectionType,
    /// 条目列表
    pub items: Vec<ChangelogItem>,
}

/// 分区类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionType {
    /// 新功能
    Features,
    /// Bug 修复
    BugFixes,
    /// 性能优化
    Performance,
    /// 代码重构
    Refactoring,
    /// 文档更新
    Documentation,
    /// 样式调整
    Styles,
    /// 构建系统
    BuildSystem,
    /// CI/CD
    CiCd,
    /// 回滚
    Reverts,
    /// 其他
    Other,
}

impl SectionType {
    /// 转换为英文标签。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Features => "Features",
            Self::BugFixes => "Bug Fixes",
            Self::Performance => "Performance Improvements",
            Self::Refactoring => "Code Refactoring",
            Self::Documentation => "Documentation",
            Self::Styles => "Styles",
            Self::BuildSystem => "Build System",
            Self::CiCd => "Continuous Integration",
            Self::Reverts => "Reverts",
            Self::Other => "Other Changes",
        }
    }

    /// 从提交类型获取分区类型。
    #[must_use]
    pub fn from_commit_type(commit_type: &str) -> Self {
        match commit_type {
            "feat" => Self::Features,
            "fix" => Self::BugFixes,
            "perf" => Self::Performance,
            "refactor" => Self::Refactoring,
            "docs" => Self::Documentation,
            "style" => Self::Styles,
            "build" | "chore" => Self::BuildSystem,
            "ci" => Self::CiCd,
            "revert" => Self::Reverts,
            _ => Self::Other,
        }
    }
}

/// Changelog 条目。
#[derive(Debug, Clone)]
pub struct ChangelogItem {
    /// 提交范围
    pub scope: Option<String>,
    /// 提交消息
    pub message: String,
    /// 是否为破坏性变更
    pub breaking: bool,
    /// 提交 SHA
    pub sha: Option<String>,
}

/// Changelog 配置。
#[derive(Debug, Clone)]
pub struct ChangelogConfig {
    /// 版本前缀
    pub version_prefix: String,
    /// 是否包含 SHA
    pub include_sha: bool,
    /// 是否包含日期
    pub include_date: bool,
    /// 分区顺序
    pub sections_order: Vec<SectionType>,
}

impl Default for ChangelogConfig {
    fn default() -> Self {
        Self {
            version_prefix: "##".to_string(),
            include_sha: false,
            include_date: true,
            sections_order: vec![
                SectionType::Features,
                SectionType::BugFixes,
                SectionType::Performance,
                SectionType::Refactoring,
                SectionType::Documentation,
                SectionType::BuildSystem,
                SectionType::CiCd,
                SectionType::Reverts,
                SectionType::Other,
            ],
        }
    }
}

/// Changelog 生成器。
#[derive(Debug, Clone)]
pub struct ChangelogGenerator {
    config: ChangelogConfig,
}

impl Default for ChangelogGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangelogGenerator {
    /// 创建新实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ChangelogConfig::default(),
        }
    }

    /// 使用自定义配置创建实例。
    #[must_use]
    pub fn with_config(mut self, config: ChangelogConfig) -> Self {
        self.config = config;
        self
    }

    /// 从版本 tag 生成 changelog。
    pub fn generate_from_tag(
        &self,
        cwd: &Path,
        from_tag: &str,
        to_tag: &str,
    ) -> Result<String, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args([
                "log",
                &format!("{from_tag}..{to_tag}"),
                "--pretty=format:%H%x1f%ct%x1f%s",
            ])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        let entries = self.parse_commits(&String::from_utf8_lossy(&output.stdout));
        self.generate_changelog(&entries, to_tag, None)
    }

    /// 从日期范围生成 changelog。
    pub fn generate_from_date(
        &self,
        cwd: &Path,
        from_date: &str,
        to_date: &str,
    ) -> Result<String, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args([
                "log",
                &format!("--since={from_date}"),
                &format!("--until={to_date}"),
                "--pretty=format:%H%x1f%ct%x1f%s",
            ])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        let entries = self.parse_commits(&String::from_utf8_lossy(&output.stdout));
        self.generate_changelog(&entries, "", Some(to_date))
    }

    /// 从最新提交生成 changelog。
    pub fn generate_from_head(&self, cwd: &Path, base_tag: Option<&str>) -> Result<String, String> {
        let args: Vec<String> = if let Some(tag) = base_tag {
            vec![
                "log".to_string(),
                format!("{}..HEAD", tag),
                "--pretty=format:%H%x1f%ct%x1f%s".to_string(),
            ]
        } else {
            vec![
                "log".to_string(),
                "HEAD".to_string(),
                "-100".to_string(),
                "--pretty=format:%H%x1f%ct%x1f%s".to_string(),
            ]
        };

        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(&args)
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        let entries = self.parse_commits(&String::from_utf8_lossy(&output.stdout));
        let version = base_tag.unwrap_or("Unreleased").to_string();
        self.generate_changelog(&entries, &version, None)
    }

    fn parse_commits(&self, output: &str) -> Vec<(String, i64, String)> {
        output
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('\u{001f}').collect();
                if parts.len() >= 3 {
                    Some((
                        parts[0].to_string(),
                        parts[1].parse().unwrap_or(0),
                        parts[2].to_string(),
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    fn generate_changelog(
        &self,
        commits: &[(String, i64, String)],
        version: &str,
        date: Option<&str>,
    ) -> Result<String, String> {
        let mut sections: HashMap<SectionType, Vec<ChangelogItem>> = HashMap::new();

        for (sha, _timestamp, message) in commits {
            let header = message.lines().next().unwrap_or(message);
            let (commit_type, scope, desc, breaking) = self.parse_header(header);

            let item = ChangelogItem {
                scope,
                message: desc,
                breaking,
                sha: if self.config.include_sha {
                    Some(sha[..7].to_string())
                } else {
                    None
                },
            };

            let section_type = SectionType::from_commit_type(&commit_type);
            sections.entry(section_type).or_default().push(item);
        }

        let mut changelog = String::new();
        let version_line = if version.is_empty() {
            "## Unreleased".to_string()
        } else {
            format!("{} {}", self.config.version_prefix, version)
        };

        changelog.push_str(&version_line);
        if let Some(d) = date {
            changelog.push_str(&format!(" ({d})"));
        }
        changelog.push_str("\n\n");

        for section_type in &self.config.sections_order {
            if let Some(items) = sections.get(section_type)
                && !items.is_empty()
            {
                changelog.push_str(&format!("### {}\n\n", section_type.as_str()));
                for item in items {
                    changelog.push_str(&self.format_item(item));
                }
                changelog.push('\n');
            }
        }

        Ok(changelog)
    }

    fn parse_header(&self, header: &str) -> (String, Option<String>, String, bool) {
        let header = header.trim();
        let breaking = header.contains("!:") || header.contains("!):");

        let parts: Vec<&str> = header.splitn(3, [':', '!']).collect();

        if parts.len() >= 2 {
            let type_scope = parts[0].replace('!', "");
            let (commit_type, scope) = if let Some(paren_pos) = type_scope.find('(') {
                let t = type_scope[..paren_pos].trim().to_string();
                let s = type_scope[paren_pos..]
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .to_string();
                (t, Some(s))
            } else {
                (type_scope.trim().to_string(), None)
            };

            let desc = parts.get(1).map_or_else(
                || header.to_string(),
                |s| {
                    let d = s.trim();
                    if d.is_empty() && parts.len() > 2 {
                        parts[2].trim().to_string()
                    } else {
                        d.to_string()
                    }
                },
            );

            (commit_type, scope, desc, breaking)
        } else {
            (header.to_string(), None, header.to_string(), false)
        }
    }

    fn format_item(&self, item: &ChangelogItem) -> String {
        let mut line = String::from("- ");

        if item.breaking {
            line.push_str("**BREAKING** ");
        }

        if let Some(scope) = &item.scope {
            line.push_str(&format!("**{scope}**: "));
        }

        line.push_str(&item.message);

        if let Some(sha) = &item.sha {
            line.push_str(&format!(" ({sha})"));
        }

        line.push('\n');
        line
    }

    /// 追加到现有 changelog 文件。
    pub fn append_to_file(&self, changelog_path: &Path, content: &str) -> Result<(), String> {
        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(changelog_path)
            .map_err(|e| format!("打开文件失败: {e}"))?;

        writeln!(file, "{content}\n").map_err(|e| format!("写入文件失败: {e}"))
    }

    /// 生成完整 changelog。
    pub fn generate_full_changelog(&self, cwd: &Path) -> Result<String, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["tag", "--sort=-v:refname"])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        let tag_output = String::from_utf8_lossy(&output.stdout).to_string();
        let tags: Vec<&str> = tag_output
            .lines()
            .filter(|t| !t.starts_with('v') || t.parse::<f32>().is_ok())
            .collect();

        let mut full_changelog = String::new();
        full_changelog.push_str("# Changelog\n\n");
        full_changelog
            .push_str("All notable changes to this project will be documented in this file.\n\n");
        full_changelog.push_str("The format is based on [Conventional Commits](https://www.conventionalcommits.org/).\n\n");

        if tags.is_empty() {
            return self.generate_from_head(cwd, None);
        }

        for (i, tag) in tags.iter().enumerate() {
            let prev_tag = tags.get(i + 1);
            let section = if let Some(prev) = prev_tag {
                self.generate_from_tag(cwd, prev, tag)?
            } else {
                self.generate_from_tag(cwd, "HEAD", tag)?
            };

            if !section.is_empty() && !section.contains("## Unreleased") {
                full_changelog.push_str(&section);
            }
        }

        Ok(full_changelog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_header_features_and_breaking() {
        let generator = ChangelogGenerator::new();
        let (t, s, d, b) = generator.parse_header("feat(core): add new feature");
        assert_eq!(t, "feat");
        assert_eq!(s, Some("core".to_string()));
        assert_eq!(d, "add new feature");
        assert!(!b);

        let (_, _, _, breaking) = generator.parse_header("feat!: breaking change");
        assert!(breaking);
    }
}
