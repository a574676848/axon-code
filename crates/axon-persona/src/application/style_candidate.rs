//! 习得风格候选的校验。
//!
//! 候选来自评估器的专用字段，不是它的判断理由；这里再做一次内容门禁，
//! 确保进入稳定段的东西既不过长，也不携带凭据或与不可违背项冲突。

use crate::constants::MAX_LEARNED_STYLE_CHARS;

/// 候选被拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleCandidateError {
    /// 空白候选。
    Empty,
    /// 超过字符上限。
    TooLong,
    /// 疑似携带凭据或私钥。
    SensitiveContent,
    /// 与不可违背项重复或直接冲突。
    ConflictsWithInvariant,
}

impl std::fmt::Display for StyleCandidateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "风格候选为空"),
            Self::TooLong => write!(formatter, "风格候选超过 {MAX_LEARNED_STYLE_CHARS} 字"),
            Self::SensitiveContent => write!(formatter, "风格候选疑似包含凭据或私钥"),
            Self::ConflictsWithInvariant => write!(formatter, "风格候选与不可违背项冲突"),
        }
    }
}

impl std::error::Error for StyleCandidateError {}

/// 校验并规范化风格候选。
///
/// # Errors
///
/// 空白、超长、疑似凭据，或与既有不可违背项语义重复时返回 [`StyleCandidateError`]。
pub fn validate_style_candidate(
    candidate: &str,
    invariants: &[String],
) -> Result<String, StyleCandidateError> {
    let text = candidate.trim();
    if text.is_empty() {
        return Err(StyleCandidateError::Empty);
    }
    if text.chars().count() > MAX_LEARNED_STYLE_CHARS {
        return Err(StyleCandidateError::TooLong);
    }
    if looks_sensitive(text) {
        return Err(StyleCandidateError::SensitiveContent);
    }
    let normalized = normalize(text);
    if invariants
        .iter()
        .map(|invariant| normalize(invariant))
        .any(|invariant| !invariant.is_empty() && invariant == normalized)
    {
        return Err(StyleCandidateError::ConflictsWithInvariant);
    }
    Ok(text.to_owned())
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn looks_sensitive(text: &str) -> bool {
    const MARKERS: [&str; 8] = [
        "sk-",
        "ghp_",
        "xoxb-",
        "-----begin",
        "api_key",
        "api key",
        "password",
        "secret",
    ];
    let lowered = text.to_lowercase();
    MARKERS.iter().any(|marker| lowered.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invariants() -> Vec<String> {
        vec!["依据事实作答".to_owned(), "不使用冗余客套话".to_owned()]
    }

    #[test]
    fn plain_style_candidate_is_accepted_and_trimmed() {
        let accepted = validate_style_candidate("  少用寒暄，结论先行  ", &invariants()).unwrap();

        assert_eq!(accepted, "少用寒暄，结论先行");
    }

    #[test]
    fn blank_candidate_is_rejected() {
        assert_eq!(
            validate_style_candidate("   ", &invariants()),
            Err(StyleCandidateError::Empty)
        );
    }

    #[test]
    fn overlong_candidate_is_rejected_at_the_character_limit() {
        let at_limit = "字".repeat(MAX_LEARNED_STYLE_CHARS);
        assert!(validate_style_candidate(&at_limit, &invariants()).is_ok());

        let over = "字".repeat(MAX_LEARNED_STYLE_CHARS + 1);
        assert_eq!(
            validate_style_candidate(&over, &invariants()),
            Err(StyleCandidateError::TooLong)
        );
    }

    #[test]
    fn credential_shaped_candidates_are_rejected() {
        for text in [
            "把我的 sk-abc123 记下来",
            "api_key=abcd1234",
            "-----BEGIN PRIVATE KEY-----",
        ] {
            assert_eq!(
                validate_style_candidate(text, &invariants()),
                Err(StyleCandidateError::SensitiveContent),
                "{text} 应被拒绝"
            );
        }
    }

    #[test]
    fn candidates_repeating_an_invariant_are_rejected() {
        assert_eq!(
            validate_style_candidate("  依据事实作答 ", &invariants()),
            Err(StyleCandidateError::ConflictsWithInvariant)
        );
        assert_eq!(
            validate_style_candidate("依据 事实  作答", &invariants()),
            Err(StyleCandidateError::ConflictsWithInvariant),
            "空白差异不应绕过冲突判定"
        );
    }
}
