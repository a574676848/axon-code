//! Agent 上下文敏感信息脱敏。

use crate::CommandErrorSpan;

/// Agent 上下文敏感值替换标记。
pub const AGENT_CONTEXT_REDACTION_PLACEHOLDER: &str = "[REDACTED]";

const SENSITIVE_FIELD_NAMES: [&str; 14] = [
    "authorization",
    "api_key",
    "apikey",
    "api-key",
    "access_token",
    "refresh_token",
    "id_token",
    "token",
    "secret",
    "credential",
    "password",
    "signature",
    "sig",
    "x-api-key",
];
const SECRET_PREFIXES: [&str; 8] = [
    "sk-", "sk-ant-", "AIza", "AKIA", "ASIA", "ghp_", "gho_", "ghs_",
];
const MIN_SECRET_TOKEN_CHARS: usize = 16;

/// Agent 上下文脱敏策略。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentContextRedactionPolicy {
    extra_sensitive_field_names: Vec<String>,
    extra_secret_prefixes: Vec<String>,
}

impl AgentContextRedactionPolicy {
    /// 创建只追加规则的脱敏策略；内置敏感字段和 token 前缀始终生效。
    #[must_use]
    pub fn new(
        extra_sensitive_field_names: Vec<String>,
        extra_secret_prefixes: Vec<String>,
    ) -> Self {
        Self {
            extra_sensitive_field_names: normalize_field_names(extra_sensitive_field_names),
            extra_secret_prefixes: normalize_prefixes(extra_secret_prefixes),
        }
    }

    fn sensitive_field_names(&self) -> impl Iterator<Item = &str> {
        SENSITIVE_FIELD_NAMES
            .iter()
            .copied()
            .chain(self.extra_sensitive_field_names.iter().map(String::as_str))
    }

    fn secret_prefixes(&self) -> impl Iterator<Item = &str> {
        SECRET_PREFIXES
            .iter()
            .copied()
            .chain(self.extra_secret_prefixes.iter().map(String::as_str))
    }
}

/// 将会进入模型上下文、Agent Pane 或本机历史投影的文本脱敏。
#[must_use]
pub fn redact_agent_context_text(value: &str) -> String {
    redact_agent_context_text_with_policy(value, &AgentContextRedactionPolicy::default())
}

/// 按指定策略脱敏会进入模型上下文、Agent Pane 或本机历史投影的文本。
#[must_use]
pub fn redact_agent_context_text_with_policy(
    value: &str,
    policy: &AgentContextRedactionPolicy,
) -> String {
    redact_sensitive_assignments(&redact_secret_tokens(value, policy), policy)
}

/// 脱敏结构化错误中的 message 字段，保留 kind/file/line/column 定位信息。
#[must_use]
pub fn redact_command_errors(errors: Vec<CommandErrorSpan>) -> Vec<CommandErrorSpan> {
    redact_command_errors_with_policy(errors, &AgentContextRedactionPolicy::default())
}

/// 按指定策略脱敏结构化错误中的 message 字段，保留 kind/file/line/column 定位信息。
#[must_use]
pub fn redact_command_errors_with_policy(
    errors: Vec<CommandErrorSpan>,
    policy: &AgentContextRedactionPolicy,
) -> Vec<CommandErrorSpan> {
    errors
        .into_iter()
        .map(|mut error| {
            error.message = redact_agent_context_text_with_policy(&error.message, policy);
            error
        })
        .collect()
}

fn redact_secret_tokens(value: &str, policy: &AgentContextRedactionPolicy) -> String {
    let mut output = String::with_capacity(value.len());
    let mut token = String::new();
    for character in value.chars() {
        if is_token_boundary(character) {
            push_redacted_token(&mut output, &token, policy);
            token.clear();
            output.push(character);
        } else {
            token.push(character);
        }
    }
    push_redacted_token(&mut output, &token, policy);
    output
}

fn push_redacted_token(output: &mut String, token: &str, policy: &AgentContextRedactionPolicy) {
    if token.is_empty() {
        return;
    }
    if is_secret_token(token, policy) {
        output.push_str(AGENT_CONTEXT_REDACTION_PLACEHOLDER);
    } else {
        output.push_str(token);
    }
}

fn is_secret_token(token: &str, policy: &AgentContextRedactionPolicy) -> bool {
    let trimmed = token.trim_matches(|character: char| {
        matches!(character, '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}')
    });
    trimmed.chars().count() >= MIN_SECRET_TOKEN_CHARS
        && policy
            .secret_prefixes()
            .any(|prefix| trimmed.starts_with(prefix))
}

fn redact_sensitive_assignments(value: &str, policy: &AgentContextRedactionPolicy) -> String {
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if let Some((_, _, value_start)) = find_sensitive_assignment(&value[index..], policy) {
            let absolute_value_start = index + value_start;
            output.push_str(&value[index..absolute_value_start]);
            output.push_str(AGENT_CONTEXT_REDACTION_PLACEHOLDER);
            index = assignment_value_end(value, absolute_value_start);
        } else {
            output.push_str(&value[index..]);
            break;
        }
    }
    output
}

fn find_sensitive_assignment(
    value: &str,
    policy: &AgentContextRedactionPolicy,
) -> Option<(usize, usize, usize)> {
    let lower = value.to_ascii_lowercase();
    policy
        .sensitive_field_names()
        .filter_map(|name| {
            let start = lower.find(name)?;
            let key_end = start + name.len();
            let value_start = assignment_value_start(value, key_end)?;
            Some((start, key_end, value_start))
        })
        .min_by_key(|(start, _, _)| *start)
}

fn assignment_value_start(value: &str, key_end: usize) -> Option<usize> {
    let mut cursor = key_end;
    cursor = skip_ascii_spaces(value, cursor);
    let separator = value.get(cursor..)?.chars().next()?;
    if !matches!(separator, '=' | ':') {
        return None;
    }
    cursor += separator.len_utf8();
    Some(skip_ascii_spaces(value, cursor))
}

fn assignment_value_end(value: &str, value_start: usize) -> usize {
    let mut quote: Option<char> = None;
    let mut has_value = false;
    for (offset, character) in value[value_start..].char_indices() {
        if quote.is_none() && matches!(character, '"' | '\'') {
            quote = Some(character);
            has_value = true;
            continue;
        }
        if quote.is_some_and(|quote| character == quote) && has_value {
            return value_start + offset;
        }
        if quote.is_none() && is_assignment_value_boundary(character) {
            return value_start + offset;
        }
        has_value = true;
    }
    value.len()
}

fn skip_ascii_spaces(value: &str, start: usize) -> usize {
    let mut cursor = start;
    while let Some(character) = value.get(cursor..).and_then(|tail| tail.chars().next()) {
        if !character.is_ascii_whitespace() {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}

fn is_token_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '"' | '\'' | '`' | '<' | '>' | '(' | ')')
}

fn is_assignment_value_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, ',' | ';' | '&')
}

fn normalize_field_names(values: Vec<String>) -> Vec<String> {
    normalize_values(values, str::to_ascii_lowercase)
}

fn normalize_prefixes(values: Vec<String>) -> Vec<String> {
    normalize_values(values, ToOwned::to_owned)
}

fn normalize_values(values: Vec<String>, normalize: impl Fn(&str) -> String) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let candidate = normalize(trimmed);
        if !normalized.iter().any(|existing| existing == &candidate) {
            normalized.push(candidate);
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_context_redaction_masks_known_token_prefixes() {
        let text = "Authorization: Bearer sk-ant-test-fixture-invalid-0001";

        let redacted = redact_agent_context_text(text);

        assert!(redacted.contains(AGENT_CONTEXT_REDACTION_PLACEHOLDER));
        assert!(!redacted.contains("sk-ant-test-fixture-invalid-0001"));
    }

    #[test]
    fn agent_context_redaction_masks_sensitive_assignments() {
        let text = "curl https://api.example.test?api_key=secret-value token: ghp_test_fixture_invalid_0001";

        let redacted = redact_agent_context_text(text);

        assert!(redacted.contains("api_key=[REDACTED]"));
        assert!(redacted.contains("token: [REDACTED]"));
        assert!(!redacted.contains("secret-value"));
        assert!(!redacted.contains("ghp_test_fixture_invalid_0001"));
    }

    #[test]
    fn agent_context_redaction_keeps_non_secret_output() {
        let text = "error[E0308]: mismatched types";

        let redacted = redact_agent_context_text(text);

        assert_eq!(redacted, text);
    }

    #[test]
    fn agent_context_redaction_policy_adds_fields_and_prefixes() {
        let policy = AgentContextRedactionPolicy::new(
            vec!["workspace_ticket".to_owned()],
            vec!["ovk_".to_owned()],
        );
        let text = "workspace_ticket=local-secret token=ghp_test_builtin_fixture_0001 ovk_test_custom_fixture_0001";

        let redacted = redact_agent_context_text_with_policy(text, &policy);

        assert!(redacted.contains("workspace_ticket=[REDACTED]"));
        assert!(redacted.contains("token=[REDACTED]"));
        assert!(!redacted.contains("local-secret"));
        assert!(!redacted.contains("ghp_test_builtin_fixture_0001"));
        assert!(!redacted.contains("ovk_test_custom_fixture_0001"));
    }
}
