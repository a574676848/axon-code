//! `builtin.turn_trace` 的执行侧：按引用回取单轮工具回执原文。
//!
//! 轮次记录只保存回执摘要与 `output_ref` 指针，完整原文在归档文件里；本文件负责
//! 定位轮次、按引用挑选回执、按 `max_chars` 截断并如实上报。

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Mutex;

use axon_agent_runtime::ProviderToolCall;

use super::{
    ToolOrchestratorContext, ToolReceipt, completed_tool_receipt, failed_tool_input_receipt,
    failed_tool_receipt,
};

/// 按引用回取单轮工具回执原文。
///
/// 轮次记录只保存回执摘要与 `output_ref` 指针，完整原文在归档文件里；
/// 本函数负责定位轮次、按引用挑选回执、按 `max_chars` 截断并如实上报。
pub(super) fn execute_turn_trace(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let Some(config_path) = context.config_path.as_deref() else {
        return failed_tool_receipt(tool_call, "builtin.turn_trace 缺少本地配置路径");
    };
    let value = match tool_call.input.as_json() {
        Some(value) => value,
        None => {
            return failed_tool_input_receipt(tool_call, "builtin.turn_trace 参数必须是 JSON");
        }
    };
    let read_text = |field: &str| {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let receipt_ref = read_text(axon_agent_runtime::TURN_TRACE_RECEIPT_REF_FIELD);
    let explicit_session = read_text(axon_agent_runtime::TURN_TRACE_SESSION_ID_FIELD);
    let trace_id = read_text(axon_agent_runtime::TURN_TRACE_TRACE_ID_FIELD);
    let explicit_sequence = match value.get(axon_agent_runtime::TURN_TRACE_SEQUENCE_FIELD) {
        None | Some(serde_json::Value::Null) => None,
        Some(raw) => match raw.as_u64() {
            Some(sequence) => Some(sequence),
            None => {
                return failed_tool_input_receipt(
                    tool_call,
                    "builtin.turn_trace sequence 必须是非负整数",
                );
            }
        },
    };
    if explicit_sequence.is_some() && explicit_session.is_none() {
        return failed_tool_input_receipt(
            tool_call,
            "builtin.turn_trace sequence 必须与 session_id 同时给出",
        );
    }
    let max_chars = match value.get(axon_agent_runtime::TURN_TRACE_MAX_CHARS_FIELD) {
        None | Some(serde_json::Value::Null) => axon_agent_runtime::TURN_TRACE_DEFAULT_MAX_CHARS,
        Some(raw) => match raw.as_u64() {
            Some(chars)
                if (1..=axon_agent_runtime::TURN_TRACE_MAX_CHARS_LIMIT).contains(&chars) =>
            {
                chars
            }
            _ => {
                return failed_tool_input_receipt(
                    tool_call,
                    "builtin.turn_trace max_chars 必须在 1 到 32000 之间",
                );
            }
        },
    };
    if receipt_ref.is_none() && trace_id.is_none() && explicit_session.is_none() {
        return failed_tool_input_receipt(
            tool_call,
            "builtin.turn_trace 必须给出 receipt_ref、trace_id 或 session_id 加 sequence 之一",
        );
    }

    // `receipt:<n>` 只是轮内位置下标，不唯一：脱离 trace_id 去猜轮次，会把别的轮的
    // 第 n 条回执当成这一条返回，且不报错。只给 receipt_ref 时直接拒绝，不猜。
    if receipt_ref.is_some() && trace_id.is_none() && explicit_session.is_none() {
        return failed_tool_input_receipt(
            tool_call,
            "builtin.turn_trace receipt_ref 必须与 trace_id 或 session_id 加 sequence 同给",
        );
    }

    let (session_id, sequence) = match trace_id.as_deref() {
        Some(trace_id) => match parse_trace_id(trace_id) {
            Some(parsed) => parsed,
            None => {
                return failed_tool_input_receipt(
                    tool_call,
                    "builtin.turn_trace trace_id 必须形如 trace:<session>:<sequence>",
                );
            }
        },
        None => (
            explicit_session.unwrap_or_else(|| context.session_id.0.to_string()),
            explicit_sequence,
        ),
    };

    // 跨会话一律拒绝：只允许读当前会话自己的轮次。daemon 与远端通道共用同一份归档，
    // 不加归属校验就等于把别人的工具原文开放给任意调用方。
    if session_id != context.session_id.0.to_string() {
        return failed_tool_receipt(
            tool_call,
            format!("builtin.turn_trace 拒绝跨会话引用：{session_id} 不是当前会话"),
        );
    }
    let (record, resolved_sequence) = match load_turn_record(config_path, &session_id, sequence) {
        Ok(record) => record,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    let archive_root = crate::run_commands::run_session_store::resolved_axon_root(config_path)
        .map(|root| root.join("data"))
        .ok();
    let receipts = match collect_turn_receipts(
        &record,
        receipt_ref.as_deref(),
        max_chars,
        archive_root.as_deref(),
    ) {
        Ok(receipts) => receipts,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let is_truncated = |receipt: &serde_json::Value| {
        receipt
            .get("truncated")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    };
    let truncated_count = receipts
        .iter()
        .filter(|receipt| is_truncated(receipt))
        .count();
    // 总字节按归档声明值累加：这是「这一轮工具输出共有多少」的准确来源，
    // 不依赖本次读了多少。调用方据此判断还有多少没看到。
    let total_bytes: u64 = record
        .tool_receipts
        .iter()
        .filter_map(|receipt| receipt.output_ref.as_ref())
        .map(|reference| u64::try_from(reference.byte_len).unwrap_or(u64::MAX))
        .sum();
    let payload = serde_json::json!({
        "traceId": format!("trace:{session_id}:{resolved_sequence}"),
        "turnId": record.assistant_part_id,
        "matchedReceipts": receipts.len(),
        "truncated": truncated_count > 0,
        "truncatedReceipts": truncated_count,
        "totalBytes": total_bytes,
        "receipts": receipts,
    });
    completed_tool_receipt(tool_call, payload.to_string(), None, false)
}

/// 解析 `trace:<session>:<sequence>`；非约定格式返回 `None`，不做猜测。
fn parse_trace_id(trace_id: &str) -> Option<(String, Option<u64>)> {
    let mut parts = trace_id.split(':');
    if parts.next()? != "trace" {
        return None;
    }
    let session = parts.next().filter(|session| !session.is_empty())?;
    let sequence = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((session.to_owned(), Some(sequence)))
}

/// 载入指定会话的某一轮；未给 `sequence` 时取该会话最新一轮。
///
/// `sequence` 是 `rollout_turns.sequence`，即会话内轮序，与 `replay_chain`
/// 的会话祖先链下标是两套编号；用后者当轮序会把祖先会话的记录当成这一轮。
fn load_turn_record(
    config_path: &std::path::Path,
    session_id: &str,
    sequence: Option<u64>,
) -> Result<
    (
        crate::run_commands::run_session_store::RunSessionRecord,
        u64,
    ),
    String,
> {
    let db_path = crate::run_commands::run_rollout_store::resolved_rollout_db_path(config_path)?;
    if !db_path.exists() {
        return Err(format!(
            "builtin.turn_trace 没有可用的轮次记录库：{session_id} 尚未落盘"
        ));
    }
    let store = crate::run_commands::run_rollout_store::RolloutSqliteStore::open(&db_path)?;
    match sequence {
        Some(sequence) => {
            let record = store
                .load_turn_by_sequence(session_id, sequence)?
                .ok_or_else(|| {
                    format!("builtin.turn_trace 会话 {session_id} 没有第 {sequence} 条轮次记录")
                })?;
            Ok((record, sequence))
        }
        None => {
            let record = store
                .load_by_id(session_id)?
                .ok_or_else(|| format!("builtin.turn_trace 未找到会话 {session_id} 的轮次记录"))?;
            let resolved = store
                .turn_sequence(session_id, &record.assistant_part_id)?
                .unwrap_or(0);
            Ok((record, resolved))
        }
    }
}

/// 从轮次回执里挑出目标回执，并按上限读取归档原文。
fn collect_turn_receipts(
    record: &crate::run_commands::run_session_store::RunSessionRecord,
    receipt_ref: Option<&str>,
    max_chars: u64,
    archive_root: Option<&std::path::Path>,
) -> Result<Vec<serde_json::Value>, String> {
    let target_index = match receipt_ref {
        None => None,
        Some(reference) => Some(parse_receipt_ref(reference)?),
    };
    let mut collected = Vec::new();
    for (index, receipt) in record.tool_receipts.iter().enumerate() {
        if let Some(target) = target_index
            && target != index
        {
            continue;
        }
        let (output, truncated) =
            read_archived_output(archive_root, receipt.output_ref.as_ref(), max_chars)?;
        let output_chars = u64::try_from(output.chars().count()).unwrap_or(u64::MAX);
        collected.push(serde_json::json!({
            "receiptRef": format!("receipt:{}", index + 1),
            "toolName": receipt.tool_name,
            "status": receipt.status,
            "outputRef": receipt.output_ref,
            "outputChars": output_chars,
            "truncated": truncated,
            "output": output,
        }));
    }
    if collected.is_empty()
        && let Some(target) = target_index
    {
        return Err(format!(
            "builtin.turn_trace 引用不存在：该轮没有第 {} 条回执",
            target + 1
        ));
    }
    Ok(collected)
}

/// 解析 `receipt:<n>` 为 0 基下标。
fn parse_receipt_ref(reference: &str) -> Result<usize, String> {
    let index = reference
        .strip_prefix("receipt:")
        .and_then(|index| index.parse::<u64>().ok())
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index >= 1)
        .ok_or_else(|| "builtin.turn_trace receipt_ref 必须形如 receipt:<n>".to_owned())?;
    Ok(index - 1)
}

/// 读取归档原文并按上限截断；没有归档时返回空正文而不是报错。
fn read_archived_output(
    archive_root: Option<&std::path::Path>,
    output_ref: Option<&axon_domain::OutputRef>,
    max_chars: u64,
) -> Result<(String, bool), String> {
    let Some(output_ref) = output_ref else {
        return Ok((String::new(), false));
    };
    let Some(archive_root) = archive_root else {
        return Err("builtin.turn_trace 无法解析归档根目录".to_owned());
    };
    let bytes = axon_agent_runtime::ContentArchiveRuntime::default()
        .read_output(archive_root, output_ref)
        .map_err(|error| format!("builtin.turn_trace 归档不可读或已过保留窗口：{error}"))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let limit = usize::try_from(max_chars).unwrap_or(usize::MAX);
    if text.chars().count() <= limit {
        return Ok((text, false));
    }
    Ok((text.chars().take(limit).collect(), true))
}

#[cfg(test)]
mod turn_trace_tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    use axon_agent_runtime::{AgentPath, ToolRegistry};
    use axon_domain::{CellId, ModelProfileId, PermissionSnapshotId, SessionId};

    fn trace_context(config_path: Option<PathBuf>) -> ToolOrchestratorContext {
        ToolOrchestratorContext {
            config_path,
            cwd: PathBuf::from("."),
            session_id: SessionId(1),
            thread_session_id: "thread-1".to_owned(),
            cell_id: CellId(1),
            model_messages: Vec::new(),
            execution_tools: Arc::new(RwLock::new(ToolRegistry::new())),
            memory_write_runtime: Arc::new(crate::run_commands::AgentMemoryWriteRuntime::default()),
            workspace_files: Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
            timeout: std::time::Duration::from_secs(10),
            goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
            goal_transport_context: None,
            cancel_token: None,
            run_cancel_token: None,
            model_profile_id: ModelProfileId(0),
            supports_vision: true,
            permission_snapshot_id: PermissionSnapshotId(0),
            current_agent_path: AgentPath::root(),
            agent_control: Arc::new(Mutex::new(
                axon_agent_runtime::AgentControlRuntime::default(),
            )),
            agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
            subagent_sessions: Arc::new(Mutex::new(
                axon_agent_runtime::SubagentSessionRuntime::default(),
            )),
            subagent_executor: Arc::new(
                crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
            ),
            subagent_cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
            user_question_port: None,
        }
    }

    fn trace_call(input: serde_json::Value) -> ProviderToolCall {
        ProviderToolCall::new("call-trace", "builtin.turn_trace", input.to_string())
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-turn-trace-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn missing_config_path_reports_local_only_contract() {
        let receipt = execute_turn_trace(&trace_call(serde_json::json!({})), &trace_context(None));

        assert_eq!(receipt.status, "failed");
        assert!(receipt.summary.contains("缺少本地配置路径"));
    }

    #[test]
    fn non_json_input_is_rejected_before_any_lookup() {
        let root = temp_root("non-json");
        let call = ProviderToolCall::new("call-trace", "builtin.turn_trace", "not-json");

        let receipt = execute_turn_trace(
            &call,
            &trace_context(Some(root.join("runtime").join("config.toml"))),
        );

        assert_eq!(receipt.status, "failed");
        assert!(receipt.summary.contains("参数必须是 JSON"));
    }

    #[test]
    fn empty_input_requires_one_of_three_addresses() {
        let root = temp_root("no-address");

        let receipt = execute_turn_trace(
            &trace_call(serde_json::json!({})),
            &trace_context(Some(root.join("runtime").join("config.toml"))),
        );

        assert_eq!(receipt.status, "failed");
        assert!(receipt.summary.contains("receipt_ref"));
        assert!(receipt.summary.contains("trace_id"));
        assert!(receipt.summary.contains("session_id"));
    }

    #[test]
    fn sequence_without_session_id_is_rejected() {
        let root = temp_root("sequence-only");

        let receipt = execute_turn_trace(
            &trace_call(serde_json::json!({ "sequence": 1 })),
            &trace_context(Some(root.join("runtime").join("config.toml"))),
        );

        assert_eq!(receipt.status, "failed");
        assert!(
            receipt
                .summary
                .contains("sequence 必须与 session_id 同时给出")
        );
    }

    #[test]
    fn trace_id_must_follow_trace_pattern() {
        let root = temp_root("bad-trace-id");

        let receipt = execute_turn_trace(
            &trace_call(serde_json::json!({ "trace_id": "trace:only-two" })),
            &trace_context(Some(root.join("runtime").join("config.toml"))),
        );

        assert_eq!(receipt.status, "failed");
        assert!(receipt.summary.contains("trace:<session>:<sequence>"));
    }

    #[test]
    fn max_chars_out_of_range_is_rejected() {
        let root = temp_root("max-chars");

        let receipt = execute_turn_trace(
            &trace_call(serde_json::json!({
                "session_id": "session-1",
                "receipt_ref": "receipt:1",
                "max_chars": 0
            })),
            &trace_context(Some(root.join("runtime").join("config.toml"))),
        );

        assert_eq!(receipt.status, "failed");
        assert!(receipt.summary.contains("1 到 32000"));
    }

    #[test]
    fn missing_rollout_store_reports_reference_invalidation() {
        let root = temp_root("no-store");

        let receipt = execute_turn_trace(
            &trace_call(serde_json::json!({
                "session_id": "session-missing",
                "receipt_ref": "receipt:1"
            })),
            &trace_context(Some(root.join("runtime").join("config.toml"))),
        );

        assert_eq!(receipt.status, "failed");
        assert!(receipt.summary.contains("builtin.turn_trace"));
    }

    #[test]
    fn receipt_ref_must_be_positive_index() {
        let root = temp_root("bad-receipt-ref");

        let receipt = execute_turn_trace(
            &trace_call(serde_json::json!({
                "session_id": "session-1",
                "receipt_ref": "receipt:0"
            })),
            &trace_context(Some(root.join("runtime").join("config.toml"))),
        );

        assert_eq!(receipt.status, "failed");
    }

    #[test]
    fn archive_root_lives_under_axon_data() {
        let root = temp_root("archive-root");
        let config_path = root.join("runtime").join("config.toml");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();

        let resolved = crate::run_commands::run_session_store::resolved_axon_root(&config_path)
            .map(|root| root.join("data"));

        assert!(resolved.is_ok(), "归档根必须能由 config_path 推出");
        let _ = std::fs::remove_dir_all(root);
    }
}
