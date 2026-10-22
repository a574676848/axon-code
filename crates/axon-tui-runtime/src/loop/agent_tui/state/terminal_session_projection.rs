//! 后台终端会话的 TUI 聚合状态。

use std::collections::HashMap;

use serde_json::Value;

const MAX_ACTIVE_TERMINAL_SESSIONS: usize = 64;
const TOOL_WRITE_STDIN: &str = "write_stdin";
const ARGUMENT_SESSION_ID: &str = "session_id";
const ARGUMENT_CHARS: &str = "chars";
const ARGUMENT_CTRL_C: &str = "ctrl_c";
const METADATA_SESSION_ID: &str = "sessionId";

#[derive(Debug, Clone)]
struct TerminalWaitBinding {
    session_id: String,
    projection_part_id: String,
}

/// 将同一 terminal session 的空轮询聚合到一个 transcript cell。
#[derive(Debug, Clone, Default)]
pub(super) struct TerminalSessionProjectionState {
    sessions: HashMap<String, String>,
    wait_calls: HashMap<String, TerminalWaitBinding>,
}

impl TerminalSessionProjectionState {
    pub(super) fn begin_wait_call(
        &mut self,
        source_part_id: &str,
        tool_name: &str,
        arguments: &str,
    ) -> Option<String> {
        let session_id = terminal_wait_session_id(tool_name, arguments)?;
        let projection_part_id = self.sessions.get(&session_id)?.clone();
        if self.wait_calls.len() >= MAX_ACTIVE_TERMINAL_SESSIONS {
            return None;
        }
        self.wait_calls.insert(
            source_part_id.to_owned(),
            TerminalWaitBinding {
                session_id,
                projection_part_id: projection_part_id.clone(),
            },
        );
        Some(projection_part_id)
    }

    pub(super) fn cancel_wait_call(&mut self, source_part_id: &str) {
        self.wait_calls.remove(source_part_id);
    }

    pub(super) fn bind_wait_call_to_projection(
        &mut self,
        source_part_id: &str,
        projection_part_id: &str,
    ) {
        self.wait_calls.insert(
            source_part_id.to_owned(),
            TerminalWaitBinding {
                session_id: String::new(),
                projection_part_id: projection_part_id.to_owned(),
            },
        );
    }

    pub(super) fn projection_part_id(&self, source_part_id: &str) -> Option<&str> {
        self.wait_calls
            .get(source_part_id)
            .map(|binding| binding.projection_part_id.as_str())
    }

    pub(super) fn observe_result(
        &mut self,
        source_part_id: &str,
        projection_part_id: &str,
        tool_name: &str,
        status: &str,
        metadata: Option<&Value>,
    ) {
        if is_write_stdin_tool(tool_name) {
            self.finish_wait_call(source_part_id, status);
            return;
        }
        let Some(session_id) = metadata_session_id(metadata) else {
            return;
        };
        if self.sessions.len() >= MAX_ACTIVE_TERMINAL_SESSIONS
            && !self.sessions.contains_key(session_id)
        {
            return;
        }
        self.sessions
            .insert(session_id.to_owned(), projection_part_id.to_owned());
    }

    pub(super) fn clear(&mut self) {
        self.sessions.clear();
        self.wait_calls.clear();
    }

    fn finish_wait_call(&mut self, source_part_id: &str, status: &str) {
        let Some(binding) = self.wait_calls.remove(source_part_id) else {
            return;
        };
        if terminal_status_is_active(status) {
            return;
        }
        if self.sessions.get(&binding.session_id) == Some(&binding.projection_part_id) {
            self.sessions.remove(&binding.session_id);
        }
    }
}

fn terminal_wait_session_id(tool_name: &str, arguments: &str) -> Option<String> {
    if !is_write_stdin_tool(tool_name) {
        return None;
    }
    let value = serde_json::from_str::<Value>(arguments).ok()?;
    let ctrl_c = value
        .get(ARGUMENT_CTRL_C)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_stdin = value
        .get(ARGUMENT_CHARS)
        .and_then(Value::as_str)
        .is_some_and(|chars| !chars.is_empty());
    if ctrl_c || has_stdin {
        return None;
    }
    value
        .get(ARGUMENT_SESSION_ID)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .map(str::to_owned)
}

fn metadata_session_id(metadata: Option<&Value>) -> Option<&str> {
    metadata?
        .get(METADATA_SESSION_ID)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
}

fn is_write_stdin_tool(tool_name: &str) -> bool {
    tool_name
        .rsplit(['.', ':', '/'])
        .next()
        .is_some_and(|name| name == TOOL_WRITE_STDIN)
}

fn terminal_status_is_active(status: &str) -> bool {
    matches!(
        status,
        "started" | "running" | "waiting" | "calling" | "pending" | "queued"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_wait_calls_reuse_active_terminal_projection() {
        let mut state = TerminalSessionProjectionState::default();
        state.observe_result(
            "tool-part:shell",
            "tool-part:shell",
            "builtin.shell_command",
            "running",
            Some(&serde_json::json!({"sessionId": "terminal-1"})),
        );

        assert_eq!(
            state.begin_wait_call(
                "tool-part:wait",
                "builtin.write_stdin",
                r#"{"session_id":"terminal-1"}"#,
            ),
            Some("tool-part:shell".to_owned())
        );
        assert_eq!(
            state.projection_part_id("tool-part:wait"),
            Some("tool-part:shell")
        );
    }

    #[test]
    fn stdin_and_interrupt_are_not_folded_into_wait_projection() {
        let mut state = TerminalSessionProjectionState::default();
        state.observe_result(
            "tool-part:shell",
            "tool-part:shell",
            "builtin.shell_command",
            "running",
            Some(&serde_json::json!({"sessionId": "terminal-1"})),
        );

        assert_eq!(
            state.begin_wait_call(
                "tool-part:input",
                "builtin.write_stdin",
                r#"{"session_id":"terminal-1","chars":"yes\n"}"#,
            ),
            None
        );
        assert_eq!(
            state.begin_wait_call(
                "tool-part:interrupt",
                "builtin.write_stdin",
                r#"{"session_id":"terminal-1","ctrl_c":true}"#,
            ),
            None
        );
    }

    #[test]
    fn terminal_completion_releases_session_projection() {
        let mut state = TerminalSessionProjectionState::default();
        state.observe_result(
            "tool-part:shell",
            "tool-part:shell",
            "builtin.shell_command",
            "running",
            Some(&serde_json::json!({"sessionId": "terminal-1"})),
        );
        state.begin_wait_call(
            "tool-part:wait",
            "builtin.write_stdin",
            r#"{"session_id":"terminal-1"}"#,
        );
        state.observe_result(
            "tool-part:wait",
            "tool-part:shell",
            "builtin.write_stdin",
            "completed",
            None,
        );

        assert_eq!(
            state.begin_wait_call(
                "tool-part:late-wait",
                "builtin.write_stdin",
                r#"{"session_id":"terminal-1"}"#,
            ),
            None
        );
    }
}
