//! 目录名匹配大小写策略。

/// 目录名匹配大小写策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseMode {
    /// 精确大小写匹配。
    Sensitive,
    /// 忽略 ASCII 大小写匹配。
    Insensitive,
}

pub(crate) fn name_matches(actual: &str, expected: &str, mode: CaseMode) -> bool {
    match mode {
        CaseMode::Sensitive => actual == expected,
        CaseMode::Insensitive => actual.eq_ignore_ascii_case(expected),
    }
}

#[cfg(test)]
mod tests {
    use super::{CaseMode, name_matches};

    #[test]
    fn case_mode_insensitive_matches_ascii_directory_names() {
        assert!(name_matches(
            "Node_Modules",
            "node_modules",
            CaseMode::Insensitive
        ));
        assert!(!name_matches(
            "Node_Modules",
            "node_modules",
            CaseMode::Sensitive
        ));
    }
}
