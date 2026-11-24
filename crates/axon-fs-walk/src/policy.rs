//! 目录跳过策略。

use crate::skip_modes::{CaseMode, name_matches};

/// fd 风格默认跳过目录。
pub const DEFAULT_SKIPPED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".next",
    ".turbo",
    "dist",
    "bin",
];

/// 更严格的默认跳过目录。
pub const DEFAULT_STRICT_EXTRA_SKIPPED_DIRS: &[&str] = &[".cache", ".venv", "__pycache__"];

/// 目录过滤策略。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DirPolicy {
    /// 跳过仓库中常见的大型生成目录。
    #[default]
    Default,
    /// 在默认策略上额外跳过缓存和虚拟环境目录。
    DefaultStrict,
    /// 调用方提供完整目录列表与大小写策略。
    Custom {
        /// 要跳过的目录名。
        dirs: Vec<String>,
        /// 目录名匹配模式。
        mode: CaseMode,
    },
    /// 不跳过任何目录。
    None,
}

impl DirPolicy {
    pub(crate) fn skips(&self, name: &str) -> bool {
        match self {
            Self::Default => default_skips(name),
            Self::DefaultStrict => {
                default_skips(name)
                    || DEFAULT_STRICT_EXTRA_SKIPPED_DIRS
                        .iter()
                        .any(|expected| name_matches(name, expected, CaseMode::Sensitive))
            }
            Self::Custom { dirs, mode } => dirs
                .iter()
                .any(|expected| name_matches(name, expected, *mode)),
            Self::None => false,
        }
    }
}

fn default_skips(name: &str) -> bool {
    DEFAULT_SKIPPED_DIRS
        .iter()
        .any(|expected| name_matches(name, expected, CaseMode::Sensitive))
}

#[cfg(test)]
mod tests {
    use super::{CaseMode, DirPolicy};

    #[test]
    fn default_policy_skips_generated_repository_directories() {
        assert!(DirPolicy::Default.skips(".git"));
        assert!(DirPolicy::Default.skips("node_modules"));
        assert!(!DirPolicy::Default.skips("src"));
    }

    #[test]
    fn custom_policy_can_match_case_insensitively() {
        let policy = DirPolicy::Custom {
            dirs: vec!["node_modules".to_owned()],
            mode: CaseMode::Insensitive,
        };

        assert!(policy.skips("Node_Modules"));
    }
}
