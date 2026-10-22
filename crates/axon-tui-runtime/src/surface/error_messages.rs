//! TUI 错误文案字典与分类策略。

use std::{collections::BTreeMap, sync::OnceLock};

use serde::Deserialize;

use crate::surface::error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity};

const ERROR_MESSAGES_TOML: &str = include_str!("errors.toml");
const EMPTY_NEXT_STEP: &str = "";
const DEFAULT_ERROR_LOCALE: ErrorLocale = ErrorLocale::Zh;
const FALLBACK_ERROR_LOCALE: ErrorLocale = ErrorLocale::En;

const CLASSIFICATION_ORDER: &[ErrorMessageKey] = &[
    ErrorMessageKey::ProviderToolCallIncomplete,
    ErrorMessageKey::MemoryPersistence,
    ErrorMessageKey::ProviderAuth,
    ErrorMessageKey::ProviderRateLimit,
    ErrorMessageKey::ProviderQuota,
    ErrorMessageKey::ContextTooLong,
    ErrorMessageKey::McpCrash,
    ErrorMessageKey::SnapshotPermission,
    ErrorMessageKey::SnapshotCorrupt,
    ErrorMessageKey::RuntimeWorkerPanic,
    ErrorMessageKey::RuntimeSchemaMismatch,
    ErrorMessageKey::CellPayloadMissingField,
    ErrorMessageKey::TerminalIo,
    ErrorMessageKey::RuntimeConfiguration,
    ErrorMessageKey::RuntimeInterrupted,
    ErrorMessageKey::ProviderOffline,
    ErrorMessageKey::RuntimeGeneric,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorLocale {
    Zh,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorMessageKey {
    ProviderToolCallIncomplete,
    MemoryPersistence,
    ProviderAuth,
    ProviderRateLimit,
    ProviderOffline,
    ProviderQuota,
    ContextTooLong,
    McpCrash,
    SnapshotCorrupt,
    SnapshotPermission,
    RuntimeConfiguration,
    RuntimeSchemaMismatch,
    RuntimeInterrupted,
    RuntimeWorkerPanic,
    CellPayloadMissingField,
    TerminalIo,
    RuntimeGeneric,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ErrorMessageInput<'a> {
    pub(crate) raw_message: &'a str,
    pub(crate) error_type: Option<&'a str>,
    pub(crate) http_status: Option<u16>,
    pub(crate) error_code: Option<&'a str>,
    pub(crate) error_param: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ErrorMessageProjection {
    pub(crate) key: ErrorMessageKey,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) next_step: Option<String>,
    pub(crate) severity: TuiErrorSeverity,
    pub(crate) action: TuiErrorAction,
    pub(crate) cell_kind: TuiErrorCellKind,
}

#[derive(Debug, Deserialize)]
struct ErrorDictionary {
    #[serde(flatten)]
    entries: BTreeMap<String, ErrorDictionaryEntry>,
}

#[derive(Debug, Deserialize)]
struct ErrorDictionaryEntry {
    severity: String,
    action: String,
    cell_kind: String,
    #[serde(default)]
    use_raw_message: bool,
    #[serde(default)]
    http_statuses: Vec<u16>,
    #[serde(default)]
    markers: Vec<String>,
    zh: LocalizedErrorMessage,
    en: LocalizedErrorMessage,
}

#[derive(Debug, Deserialize)]
struct LocalizedErrorMessage {
    title: String,
    message: String,
    next_step: String,
}

impl<'a> ErrorMessageInput<'a> {
    pub(crate) const fn runtime(raw_message: &'a str) -> Self {
        Self {
            raw_message,
            error_type: None,
            http_status: None,
            error_code: None,
            error_param: None,
        }
    }
}

impl ErrorMessageKey {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderToolCallIncomplete => "provider_tool_call_incomplete",
            Self::MemoryPersistence => "memory_persistence",
            Self::ProviderAuth => "provider_auth",
            Self::ProviderRateLimit => "provider_rate_limit",
            Self::ProviderOffline => "provider_offline",
            Self::ProviderQuota => "provider_quota",
            Self::ContextTooLong => "context_too_long",
            Self::McpCrash => "mcp_crash",
            Self::SnapshotCorrupt => "snapshot_corrupt",
            Self::SnapshotPermission => "snapshot_permission",
            Self::RuntimeConfiguration => "runtime_configuration",
            Self::RuntimeSchemaMismatch => "runtime_schema_mismatch",
            Self::RuntimeInterrupted => "runtime_interrupted",
            Self::RuntimeWorkerPanic => "runtime_worker_panic",
            Self::CellPayloadMissingField => "cell_payload_missing_field",
            Self::TerminalIo => "terminal_io",
            Self::RuntimeGeneric => "runtime_generic",
            Self::Unknown => "unknown",
        }
    }
}

pub(crate) fn project_error_message(input: &ErrorMessageInput<'_>) -> ErrorMessageProjection {
    let key = classify_error_message(input);
    let mut projection = project_error_message_for_key(key, DEFAULT_ERROR_LOCALE);
    if dictionary_entry(key).use_raw_message {
        let raw_message = input.raw_message.trim();
        if !raw_message.is_empty() {
            projection.message = raw_message.to_owned();
        }
    }
    projection
}

pub(crate) fn project_error_message_for_key(
    key: ErrorMessageKey,
    locale: ErrorLocale,
) -> ErrorMessageProjection {
    let entry = dictionary_entry(key);
    let localized = match locale {
        ErrorLocale::Zh => &entry.zh,
        ErrorLocale::En => &entry.en,
    };
    ErrorMessageProjection {
        key,
        title: localized.title.clone(),
        message: localized.message.clone(),
        next_step: non_empty_next_step(&localized.next_step),
        severity: parse_severity(&entry.severity),
        action: parse_action(&entry.action),
        cell_kind: parse_cell_kind(&entry.cell_kind),
    }
}

fn classify_error_message(input: &ErrorMessageInput<'_>) -> ErrorMessageKey {
    input
        .error_code
        .and_then(classify_structured_value)
        .or_else(|| input.error_type.and_then(classify_structured_value))
        .or_else(|| input.http_status.and_then(classify_http_status))
        .or_else(|| classify_fallback_message(input))
        .unwrap_or(ErrorMessageKey::Unknown)
}

fn classify_structured_value(value: &str) -> Option<ErrorMessageKey> {
    let normalized = value.trim().to_ascii_lowercase();
    CLASSIFICATION_ORDER.iter().copied().find(|key| {
        key.as_str() == normalized
            || dictionary_entry(*key)
                .markers
                .iter()
                .any(|marker| marker.eq_ignore_ascii_case(&normalized))
    })
}

fn classify_http_status(status: u16) -> Option<ErrorMessageKey> {
    CLASSIFICATION_ORDER
        .iter()
        .copied()
        .find(|key| dictionary_entry(*key).http_statuses.contains(&status))
}

fn classify_fallback_message(input: &ErrorMessageInput<'_>) -> Option<ErrorMessageKey> {
    let raw_message = input.raw_message.to_ascii_lowercase();
    CLASSIFICATION_ORDER.iter().copied().find(|key| {
        *key != ErrorMessageKey::RuntimeConfiguration
            && matches_fallback_key(*key, input, &raw_message)
    })
}

fn matches_fallback_key(
    key: ErrorMessageKey,
    input: &ErrorMessageInput<'_>,
    raw_message: &str,
) -> bool {
    let marker_matches = dictionary_entry(key)
        .markers
        .iter()
        .any(|marker| contains_bounded(raw_message, &marker.to_ascii_lowercase()));
    match key {
        ErrorMessageKey::SnapshotPermission | ErrorMessageKey::SnapshotCorrupt => {
            marker_matches && (is_snapshot(input) || contains_bounded(raw_message, "snapshot"))
        }
        _ => marker_matches,
    }
}

fn is_snapshot(input: &ErrorMessageInput<'_>) -> bool {
    [input.error_type, input.error_code]
        .into_iter()
        .flatten()
        .any(|value| value.to_ascii_lowercase().contains("snapshot"))
}

fn contains_bounded(haystack: &str, marker: &str) -> bool {
    haystack.match_indices(marker).any(|(start, value)| {
        let end = start.saturating_add(value.len());
        let left_is_boundary = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_marker_char(ch));
        let right_is_boundary = haystack[end..]
            .chars()
            .next()
            .is_none_or(|ch| !is_marker_char(ch));
        left_is_boundary && right_is_boundary
    })
}

fn is_marker_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn dictionary_entry(key: ErrorMessageKey) -> &'static ErrorDictionaryEntry {
    dictionary()
        .entries
        .get(key.as_str())
        .unwrap_or_else(|| panic!("errors.toml 缺少 {}", key.as_str()))
}

fn dictionary() -> &'static ErrorDictionary {
    static DICTIONARY: OnceLock<ErrorDictionary> = OnceLock::new();
    DICTIONARY.get_or_init(|| {
        debug_assert!(matches!(FALLBACK_ERROR_LOCALE, ErrorLocale::En));
        // Invariant: errors.toml is embedded at compile time and covered by dictionary tests.
        toml::from_str(ERROR_MESSAGES_TOML).expect("errors.toml 必须是合法 TOML")
    })
}

fn non_empty_next_step(value: &str) -> Option<String> {
    (value != EMPTY_NEXT_STEP).then(|| value.to_owned())
}

fn parse_severity(value: &str) -> TuiErrorSeverity {
    match value {
        "fatal" => TuiErrorSeverity::Fatal,
        "user_facing" => TuiErrorSeverity::UserFacing,
        "recoverable" => TuiErrorSeverity::Recoverable,
        "internal" => TuiErrorSeverity::Internal,
        "hint" => TuiErrorSeverity::Hint,
        other => panic!("未知错误级别 {other}"),
    }
}

fn parse_action(value: &str) -> TuiErrorAction {
    match value {
        "exit" => TuiErrorAction::Exit,
        "retry" => TuiErrorAction::Retry,
        "reconfigure" => TuiErrorAction::Reconfigure,
        "new_session" => TuiErrorAction::NewSession,
        "inspect_details" => TuiErrorAction::InspectDetails,
        "none" => TuiErrorAction::None,
        other => panic!("未知错误动作 {other}"),
    }
}

fn parse_cell_kind(value: &str) -> TuiErrorCellKind {
    match value {
        "runtime" => TuiErrorCellKind::Runtime,
        "snapshot" => TuiErrorCellKind::Snapshot,
        "cell_payload" => TuiErrorCellKind::CellPayload,
        "ui" => TuiErrorCellKind::Ui,
        "terminal" => TuiErrorCellKind::Terminal,
        other => panic!("未知错误 cell 类型 {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_401_uses_chinese_auth_recovery() {
        let projection = project_error_message(&ErrorMessageInput {
            raw_message: "Incorrect API key provided",
            error_type: Some("provider_transport"),
            http_status: Some(401),
            error_code: Some("invalid_api_key"),
            error_param: None,
        });

        assert_eq!(projection.key, ErrorMessageKey::ProviderAuth);
        assert_eq!(projection.title, "认证失败");
        assert_eq!(projection.action, TuiErrorAction::Reconfigure);
    }

    #[test]
    fn provider_429_uses_retry_action() {
        let projection = project_error_message(&ErrorMessageInput {
            raw_message: "Too many requests",
            error_type: Some("provider_transport"),
            http_status: Some(429),
            error_code: Some("rate_limit"),
            error_param: None,
        });

        assert_eq!(projection.key, ErrorMessageKey::ProviderRateLimit);
        assert_eq!(projection.action, TuiErrorAction::Retry);
        assert_eq!(projection.severity, TuiErrorSeverity::Recoverable);
    }

    #[test]
    fn mcp_crash_code_uses_retry_action() {
        let projection = project_error_message(&ErrorMessageInput {
            raw_message: "mcp server closed: transport closed",
            error_type: Some("runtime"),
            http_status: None,
            error_code: Some("mcp_crash"),
            error_param: None,
        });

        assert_eq!(projection.key, ErrorMessageKey::McpCrash);
        assert_eq!(projection.action, TuiErrorAction::Retry);
    }

    #[test]
    fn snapshot_permission_uses_reconfigure_action() {
        let projection = project_error_message(&ErrorMessageInput {
            raw_message: "access is denied",
            error_type: Some("snapshot"),
            http_status: None,
            error_code: Some("snapshot_permission"),
            error_param: None,
        });

        assert_eq!(projection.key, ErrorMessageKey::SnapshotPermission);
        assert_eq!(projection.cell_kind, TuiErrorCellKind::Snapshot);
        assert_eq!(projection.action, TuiErrorAction::Reconfigure);
    }

    #[test]
    fn unknown_error_hides_raw_debug_message() {
        let projection = project_error_message(&ErrorMessageInput::runtime(
            "provider.rs:88 token=secret debug dump",
        ));

        assert_eq!(projection.key, ErrorMessageKey::Unknown);
        assert!(!projection.message.contains("provider.rs"));
        assert!(!projection.message.contains("secret"));
    }

    #[test]
    fn english_locale_uses_english_copy() {
        let projection =
            project_error_message_for_key(ErrorMessageKey::ProviderAuth, ErrorLocale::En);

        assert_eq!(projection.title, "Authentication failed");
        assert_eq!(projection.action, TuiErrorAction::Reconfigure);
    }

    #[test]
    fn provider_tool_call_incomplete_uses_dedicated_projection() {
        let projection = project_error_message(&ErrorMessageInput {
            raw_message: "Config response payload ended",
            error_type: Some("provider_protocol"),
            http_status: Some(200),
            error_code: Some("provider_tool_call_incomplete"),
            error_param: Some("malformed_arguments"),
        });

        assert_eq!(projection.key, ErrorMessageKey::ProviderToolCallIncomplete);
        assert_eq!(projection.title, "模型响应不完整");
        assert_eq!(projection.action, TuiErrorAction::Retry);
    }

    #[test]
    fn raw_config_and_setup_words_never_classify_as_configuration() {
        for raw_message in ["Config(...) response ended", "setup payload incomplete"] {
            let projection = project_error_message(&ErrorMessageInput::runtime(raw_message));
            assert_ne!(projection.key, ErrorMessageKey::RuntimeConfiguration);
        }
    }

    #[test]
    fn structured_type_precedes_http_status_and_raw_marker() {
        let projection = project_error_message(&ErrorMessageInput {
            raw_message: "invalid api key",
            error_type: Some("configuration"),
            http_status: Some(429),
            error_code: None,
            error_param: None,
        });

        assert_eq!(projection.key, ErrorMessageKey::RuntimeConfiguration);
    }

    #[test]
    fn memory_persistence_uses_dedicated_projection_and_raw_message() {
        let projection = project_error_message(&ErrorMessageInput {
            raw_message: "Memory SQLite 记录写入失败",
            error_type: Some("memory_persistence"),
            http_status: None,
            error_code: None,
            error_param: None,
        });

        assert_eq!(projection.key, ErrorMessageKey::MemoryPersistence);
        assert_eq!(projection.title, "Memory 持久化失败");
        assert_eq!(projection.message, "Memory SQLite 记录写入失败");
        assert_eq!(projection.action, TuiErrorAction::Retry);
    }
}
