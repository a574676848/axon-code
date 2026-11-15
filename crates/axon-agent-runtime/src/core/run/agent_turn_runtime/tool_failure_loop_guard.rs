//! 连续同构工具调用的有界止损。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult};

use crate::{
    AgentModelMessage, GLOB_TOOL_NAME, HASHLINE_NOOP_CHANGE_MARKER, JOB_OUTPUT_TOOL_NAME,
    MEMORY_SEARCH_TOOL_NAME, READ_FILE_TOOL_NAME, SEARCH_TOOL_NAME, TOOL_INTENT_FIELD,
    WRITE_STDIN_TOOL_CHARS_FIELD, WRITE_STDIN_TOOL_CTRL_C_FIELD, WRITE_STDIN_TOOL_NAME,
};

const MAX_IDENTICAL_TOOL_LOOP_TURNS: u8 = 3;
/// 同一批次每次达到重复阈值都会先提示 Agent；第 4 次才硬止损。
const MAX_IDENTICAL_TOOL_LOOP_PROMPTS: u8 = 3;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

#[derive(Debug, Default)]
pub(super) struct ToolLoopGuard {
    previous_fingerprint: Option<u64>,
    consecutive_turns: u8,
    prompted_fingerprint: Option<u64>,
    prompt_count: u8,
}

impl ToolLoopGuard {
    pub(super) fn observe(&mut self, messages: &[AgentModelMessage]) -> AxonResult<Option<String>> {
        let Some(batch) = tool_loop_batch(messages) else {
            self.reset();
            return Ok(None);
        };
        if self.previous_fingerprint == Some(batch.fingerprint) {
            self.consecutive_turns = self.consecutive_turns.saturating_add(1);
        } else {
            self.previous_fingerprint = Some(batch.fingerprint);
            self.consecutive_turns = 1;
            if self.prompted_fingerprint != Some(batch.fingerprint) {
                self.prompted_fingerprint = None;
                self.prompt_count = 0;
            }
        }
        if self.consecutive_turns < MAX_IDENTICAL_TOOL_LOOP_TURNS {
            return Ok(None);
        }
        let reason = match batch.kind {
            ToolLoopKind::Failure => "相同工具失败",
            ToolLoopKind::ReadOnlyNoProgress => "无进展只读工具调用",
            ToolLoopKind::NoOpWrite => "重复提交无变化的编辑",
        };
        if self.prompted_fingerprint == Some(batch.fingerprint) {
            self.prompt_count = self.prompt_count.saturating_add(1);
        } else {
            self.prompted_fingerprint = Some(batch.fingerprint);
            self.prompt_count = 1;
        }
        self.previous_fingerprint = None;
        self.consecutive_turns = 0;
        if self.prompt_count > MAX_IDENTICAL_TOOL_LOOP_PROMPTS {
            return Err(AxonError::runtime_failure(format!(
                "检测到第 4 次连续 {} 轮{}仍无变化，已达到重复调用硬止损上限：{}。此前已要求 Agent 停止工具调用并向用户反馈。",
                MAX_IDENTICAL_TOOL_LOOP_TURNS,
                reason,
                batch.tool_names.join(", ")
            )));
        }
        let warning = match self.prompt_count {
            1 => format!(
                "第 1 次检测到连续 {} 轮{}：{}。请立即检查失败原因并改变调用参数或执行路径，禁止原样重试。",
                MAX_IDENTICAL_TOOL_LOOP_TURNS,
                reason,
                batch.tool_names.join(", ")
            ),
            2 => format!(
                "第 2 次检测到连续 {} 轮{}仍无进展：{}。必须更换工具、参数或解决方案；不要继续重复当前调用。",
                MAX_IDENTICAL_TOOL_LOOP_TURNS,
                reason,
                batch.tool_names.join(", ")
            ),
            _ => format!(
                "第 3 次检测到连续 {} 轮{}仍无变化：{}。请立即停止工具调用，直接向用户反馈已尝试的路径、当前阻塞和需要的输入；下一次仍无变化将硬停止。",
                MAX_IDENTICAL_TOOL_LOOP_TURNS,
                reason,
                batch.tool_names.join(", ")
            ),
        };
        Ok(Some(warning))
    }

    fn reset(&mut self) {
        self.previous_fingerprint = None;
        self.consecutive_turns = 0;
        self.prompted_fingerprint = None;
        self.prompt_count = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolLoopKind {
    Failure,
    ReadOnlyNoProgress,
    /// 编辑重复提交字节级等价的空改动（回执携带 NOOP_CHANGE 标记）。
    ///
    /// 这类结果在传输层是「失败」，但它与语法/上下文失败不同：模型往往只是把同一份
    /// 改动重复提交，或该改动此前的调用已经落地。分开归类才能给出正确指引。
    NoOpWrite,
}

struct ToolLoopBatch {
    kind: ToolLoopKind,
    fingerprint: u64,
    tool_names: Vec<String>,
}

fn tool_loop_batch(messages: &[AgentModelMessage]) -> Option<ToolLoopBatch> {
    let calls = tool_calls_by_id(messages);
    let results = messages
        .iter()
        .filter_map(|message| message.provider_metadata.as_ref())
        .filter_map(|metadata| metadata.tool_result.as_ref())
        .collect::<Vec<_>>();
    if results.is_empty() {
        return None;
    }
    let kind = if results.iter().all(|result| !result.success) {
        if results
            .iter()
            .all(|result| result.output.contains(HASHLINE_NOOP_CHANGE_MARKER))
        {
            ToolLoopKind::NoOpWrite
        } else {
            ToolLoopKind::Failure
        }
    } else if results.iter().all(|result| result.success)
        && results.iter().all(|result| {
            calls
                .get(result.call_id.as_str())
                .is_some_and(|(name, input)| is_read_only_no_progress_call(name, input))
        })
    {
        ToolLoopKind::ReadOnlyNoProgress
    } else {
        return None;
    };

    let mut parts = Vec::with_capacity(results.len());
    let mut tool_names = Vec::with_capacity(results.len());
    for result in results {
        let (tool_name, input) = calls
            .get(result.call_id.as_str())
            .map_or(("unknown", String::new()), |(name, input)| {
                (*name, input.clone())
            });
        tool_names.push(tool_name.to_owned());
        let normalized_input = normalized_input_for_fingerprint(tool_name, &input);
        let normalized_output = normalized_output_for_fingerprint(tool_name, &result.output);
        parts.push(format!(
            "{tool_name}\0{normalized_input}\0{normalized_output}"
        ));
    }
    parts.sort_unstable();
    tool_names.sort_unstable();
    tool_names.dedup();
    let kind_tag = match kind {
        ToolLoopKind::Failure => "failure",
        ToolLoopKind::ReadOnlyNoProgress => "read_only_no_progress",
        ToolLoopKind::NoOpWrite => "noop_write",
    };
    Some(ToolLoopBatch {
        kind,
        fingerprint: stable_hash(format!("{kind_tag}\0{}", parts.join("\u{1f}")).as_bytes()),
        tool_names,
    })
}

fn is_read_only_no_progress_call(name: &str, canonical_input: &str) -> bool {
    let Some(bare) = name.strip_prefix("builtin.") else {
        return false;
    };
    match bare {
        SEARCH_TOOL_NAME
        | READ_FILE_TOOL_NAME
        | GLOB_TOOL_NAME
        | MEMORY_SEARCH_TOOL_NAME
        | JOB_OUTPUT_TOOL_NAME => true,
        WRITE_STDIN_TOOL_NAME => is_pure_wait_write_stdin_input(canonical_input),
        _ => false,
    }
}

fn is_pure_wait_write_stdin_input(canonical_input: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(canonical_input) else {
        return false;
    };
    let has_chars = value
        .get(WRITE_STDIN_TOOL_CHARS_FIELD)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|chars| !chars.is_empty());
    let has_ctrl_c = value
        .get(WRITE_STDIN_TOOL_CTRL_C_FIELD)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    !has_chars && !has_ctrl_c
}

fn normalized_input_for_fingerprint(name: &str, canonical_input: &str) -> String {
    if name.strip_prefix("builtin.") != Some(WRITE_STDIN_TOOL_NAME) {
        return canonical_input.to_owned();
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(canonical_input) else {
        return canonical_input.to_owned();
    };
    if let Some(object) = value.as_object_mut() {
        object.remove(TOOL_INTENT_FIELD);
    }
    value.to_string()
}

fn normalized_output_for_fingerprint(name: &str, output: &str) -> String {
    let trimmed = output.trim();
    if !matches!(
        name.strip_prefix("builtin."),
        Some(JOB_OUTPUT_TOOL_NAME | WRITE_STDIN_TOOL_NAME)
    ) {
        return trimmed.to_owned();
    }
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(root) = value.as_object_mut()
    {
        if let Some(normalized) = root
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .map(strip_elapsed_ms_markers)
        {
            root.insert("summary".to_owned(), serde_json::Value::String(normalized));
        }
        if let Some(metadata) = root
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
        {
            metadata.remove("elapsedMs");
            metadata.remove("durationMs");
        }
        return value.to_string();
    }
    strip_elapsed_ms_markers(trimmed)
}

fn strip_elapsed_ms_markers(text: &str) -> String {
    let mut normalized = strip_numeric_marker(text, "elapsed_ms=");
    normalized = strip_numeric_marker(&normalized, "\"elapsedMs\":");
    strip_numeric_marker(&normalized, "\"durationMs\":")
}

fn strip_numeric_marker(text: &str, prefix: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(prefix) {
        let split_at = pos + prefix.len();
        result.push_str(&rest[..split_at]);
        let after = &rest[split_at..];
        let digits_len = after
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits_len > 0 {
            result.push_str("<n>");
            rest = &after[digits_len..];
        } else {
            rest = after;
        }
    }
    result.push_str(rest);
    result
}

fn tool_calls_by_id(messages: &[AgentModelMessage]) -> BTreeMap<&str, (&str, String)> {
    messages
        .iter()
        .filter_map(|message| message.provider_metadata.as_ref())
        .filter_map(|metadata| metadata.tool_calls.as_deref())
        .flatten()
        .map(|call| {
            (
                call.id.as_str(),
                (call.name.as_str(), call.input.canonical_text()),
            )
        })
        .collect()
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentModelToolCallMetadata;
    use axon_domain::ToolCallInput;

    #[test]
    fn repeated_identical_failed_batches_warn_then_hard_stop_on_fourth_detection() {
        let messages = tool_messages(
            "builtin.edit",
            r#"{"path":"a.rs"}"#,
            "invalid arguments",
            false,
        );
        let mut guard = ToolLoopGuard::default();

        for expected in ["第 1 次", "第 2 次", "第 3 次"] {
            assert!(guard.observe(&messages).unwrap().is_none());
            assert!(guard.observe(&messages).unwrap().is_none());
            let warning = guard
                .observe(&messages)
                .unwrap()
                .expect("重复检测应继续提示 Agent");
            assert!(warning.contains(expected));
        }
        assert!(guard.observe(&messages).unwrap().is_none());
        assert!(guard.observe(&messages).unwrap().is_none());
        let error = guard.observe(&messages).unwrap_err();
        assert!(format!("{error:?}").contains("第 4 次"));
    }

    #[test]
    fn repeated_identical_read_only_success_warns_on_third_detection() {
        let messages = tool_messages(
            "builtin.search",
            r#"{"query":"same"}"#,
            "same search result",
            true,
        );
        let mut guard = ToolLoopGuard::default();

        assert!(guard.observe(&messages).unwrap().is_none());
        assert!(guard.observe(&messages).unwrap().is_none());
        let warning = guard
            .observe(&messages)
            .unwrap()
            .expect("第一次检测应提示 Agent");
        assert!(warning.contains("无进展只读工具调用"));
        assert!(guard.observe(&messages).unwrap().is_none());
        assert!(guard.observe(&messages).unwrap().is_none());
        let warning = guard
            .observe(&messages)
            .unwrap()
            .expect("第二次检测应提示 Agent");
        assert!(warning.contains("第 2 次"));
        assert!(guard.observe(&messages).unwrap().is_none());
        assert!(guard.observe(&messages).unwrap().is_none());
        let warning = guard
            .observe(&messages)
            .unwrap()
            .expect("第三次检测应要求停止工具调用");
        assert!(warning.contains("第 3 次") && warning.contains("停止工具调用"));
    }

    #[test]
    fn changed_input_or_output_resets_repetition() {
        let first = tool_messages("builtin.search", r#"{"query":"a"}"#, "same result", true);
        let changed_input =
            tool_messages("builtin.search", r#"{"query":"b"}"#, "same result", true);
        let changed_output =
            tool_messages("builtin.search", r#"{"query":"a"}"#, "new result", true);
        let mut guard = ToolLoopGuard::default();

        assert!(guard.observe(&first).unwrap().is_none());
        assert!(guard.observe(&first).unwrap().is_none());
        assert!(guard.observe(&first).unwrap().is_some());
        assert!(guard.observe(&changed_input).is_ok());
        assert!(guard.observe(&changed_output).is_ok());
        assert!(guard.observe(&first).unwrap().is_none());
        assert!(guard.observe(&first).unwrap().is_none());
        let warning = guard
            .observe(&first)
            .unwrap()
            .expect("输入或输出变化后应从第一次提示重新计数");
        assert!(warning.contains("第 1 次"));
    }

    #[test]
    fn successful_result_breaks_failure_chain_and_restarts_threshold() {
        let failed = tool_messages(
            "builtin.edit",
            r#"{"path":"a.rs"}"#,
            "invalid arguments",
            false,
        );
        let recovered = tool_messages("builtin.edit", r#"{"path":"a.rs"}"#, "completed", true);
        let mut guard = ToolLoopGuard::default();

        assert!(guard.observe(&failed).unwrap().is_none());
        assert!(guard.observe(&failed).unwrap().is_none());
        assert!(guard.observe(&failed).unwrap().is_some());

        // 一次成功结果会清断失败链；后续同样的失败必须从第 1 次重新计数。
        assert!(guard.observe(&recovered).unwrap().is_none());
        assert!(guard.observe(&failed).unwrap().is_none());
        assert!(guard.observe(&failed).unwrap().is_none());
        let warning = guard
            .observe(&failed)
            .unwrap()
            .expect("成功后再次失败应重新从第一次提示");
        assert!(warning.contains("第 1 次"));
    }

    #[test]
    fn successful_write_tool_is_not_treated_as_no_progress() {
        let messages = tool_messages(
            "builtin.edit",
            "*** Begin Patch\n*** End Patch",
            "completed",
            true,
        );
        let mut guard = ToolLoopGuard::default();

        for _ in 0..(MAX_IDENTICAL_TOOL_LOOP_TURNS * (MAX_IDENTICAL_TOOL_LOOP_PROMPTS + 1)) {
            assert!(guard.observe(&messages).is_ok());
        }
    }

    #[test]
    fn repeated_identical_noop_edits_get_noop_guidance() {
        let messages = tool_messages(
            "builtin.edit",
            "*** Begin Patch\n*** Update File: a.rs\n@@\n-same\n+same\n*** End Patch",
            "[builtin.edit 执行失败: NOOP_CHANGE]\n目标文件: a.rs",
            false,
        );
        let mut guard = ToolLoopGuard::default();

        assert!(guard.observe(&messages).unwrap().is_none());
        assert!(guard.observe(&messages).unwrap().is_none());
        let warning = guard
            .observe(&messages)
            .unwrap()
            .expect("重复空改动应在第三次检测时提示 Agent");
        assert!(warning.contains("重复提交无变化的编辑"));
        assert!(!warning.contains("相同工具失败"));
    }

    #[test]
    fn generic_edit_failure_still_uses_failure_guidance() {
        let messages = tool_messages(
            "builtin.edit",
            "*** Begin Patch\n*** Update File: a.rs\n@@\n-context\n+new\n*** End Patch",
            "[builtin.edit 执行失败: HUNK_CONTEXT_MISMATCH]",
            false,
        );
        let mut guard = ToolLoopGuard::default();

        assert!(guard.observe(&messages).unwrap().is_none());
        assert!(guard.observe(&messages).unwrap().is_none());
        let warning = guard
            .observe(&messages)
            .unwrap()
            .expect("普通失败仍应按失败归类");
        assert!(warning.contains("相同工具失败"));
    }

    #[test]
    fn job_output_polling_with_changing_elapsed_ms_still_triggers_no_progress_guard() {
        let mut guard = ToolLoopGuard::default();
        for elapsed in [100_u64, 2500, 5000] {
            let payload = serde_json::json!({
                "tool": "builtin.job_output",
                "status": "completed",
                "summary": format!("job output job_id=7 status=running elapsed_ms={elapsed} exit_code=none stdout= stderr="),
                "metadata": { "kind": "job", "jobId": "7", "jobStatus": "running", "elapsedMs": elapsed }
            })
            .to_string();
            let messages = tool_messages("builtin.job_output", r#"{"job_id":"7"}"#, &payload, true);
            let observed = guard.observe(&messages).unwrap();
            if elapsed == 5000 {
                let warning = observed.expect("第 3 次连续空轮询应命中无进展警告");
                assert!(warning.contains("无进展只读工具调用"));
            } else {
                assert!(observed.is_none());
            }
        }

        // 新输出到达后必须重置计数，不得误判。
        let progress_payload = "job output job_id=7 status=running elapsed_ms=7000 exit_code=none stdout=compiling stderr=";
        let progress_messages = tool_messages(
            "builtin.job_output",
            r#"{"job_id":"7"}"#,
            progress_payload,
            true,
        );
        assert!(guard.observe(&progress_messages).unwrap().is_none());
    }

    #[test]
    fn write_stdin_pure_wait_triggers_guard_while_real_input_does_not() {
        let mut guard = ToolLoopGuard::default();
        for intent in ["查看输出1", "查看输出2", "查看输出3"] {
            let input = serde_json::json!({
                "session_id": "sess-1",
                "intent": intent
            })
            .to_string();
            let payload = serde_json::json!({
                "tool": "builtin.write_stdin",
                "status": "running",
                "summary": "shell command=sleep exit_code=none stdout= stderr=",
                "metadata": { "kind": "exec", "sessionId": "sess-1", "durationMs": intent.len() * 100 }
            })
            .to_string();
            let messages = tool_messages("builtin.write_stdin", &input, &payload, true);
            let observed = guard.observe(&messages).unwrap();
            if intent == "查看输出3" {
                let warning = observed.expect("连续 3 轮无进展纯等待 write_stdin 必须触发警告");
                assert!(warning.contains("无进展只读工具调用"));
            } else {
                assert!(observed.is_none());
            }
        }

        // 真正写入 chars 或发送 ctrl_c 不属于只读无进展。
        let mut input_guard = ToolLoopGuard::default();
        let interactive_messages = tool_messages(
            "builtin.write_stdin",
            r#"{"session_id":"sess-1","chars":"y\n","intent":"确认继续"}"#,
            "shell command=prompt exit_code=none stdout= stderr=",
            true,
        );
        for _ in 0..5 {
            assert!(
                input_guard
                    .observe(&interactive_messages)
                    .unwrap()
                    .is_none()
            );
        }
    }

    fn tool_messages(
        name: &str,
        input: &str,
        output: &str,
        success: bool,
    ) -> Vec<AgentModelMessage> {
        vec![
            AgentModelMessage::text("assistant", "").with_tool_calls(vec![
                AgentModelToolCallMetadata {
                    id: "call-tool".to_owned(),
                    name: name.to_owned(),
                    input: ToolCallInput::from(input),
                },
            ]),
            AgentModelMessage::tool_result("call-tool", output, success),
        ]
    }
}
