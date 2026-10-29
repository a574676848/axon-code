use super::agent_run_tool_result_wire_test_fixtures::{receipt, tool_call};
use crate::{AgentModelMessage, AgentRunToolOutputPolicy, agent_run_tool_result_messages};
use axon_domain::{OutputArchiveReference, OutputRef, OutputRefId};

fn default_output_policy() -> AgentRunToolOutputPolicy {
    AgentRunToolOutputPolicy::default()
}

#[test]
fn tool_result_message_keeps_output_source_ref() {
    let output_ref = OutputRef {
        id: OutputRefId(7),
        byte_len: 12,
        sha256: "hash".to_owned(),
        source_trace: "tool://builtin/shell_command".to_owned(),
        preview: "preview".to_owned(),
        archive_ref: Some(OutputArchiveReference {
            relative_path: "archive/hash.log".to_owned(),
        }),
    };
    let mut tool_receipt = receipt("call-1");
    tool_receipt.output_ref = Some(output_ref.clone());
    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[tool_receipt],
        None,
        default_output_policy(),
    )
    .expect("工具回执应可投影");
    assert_eq!(
        messages[1].source_ref.as_deref(),
        Some(output_ref.source_trace.as_str())
    );
}

#[test]
fn tool_result_messages_reject_invalid_pairing() {
    let orphan = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[receipt("call-2")],
        None,
        default_output_policy(),
    )
    .expect_err("orphan receipt 必须被拒绝");
    let empty = agent_run_tool_result_messages(
        &[tool_call("")],
        &[receipt("")],
        None,
        default_output_policy(),
    )
    .expect_err("空 call_id 必须被拒绝");

    assert!(orphan.contains("不匹配"));
    assert!(empty.contains("call_id 为空"));
}

#[test]
fn tool_result_messages_use_structured_call_and_result_metadata() {
    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[receipt("call-1")],
        None,
        default_output_policy(),
    )
    .expect("tool result message 应可生成");
    let call = messages[0]
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_ref())
        .and_then(|calls| calls.first())
        .expect("assistant 应携带结构化 tool call");
    let result = tool_result(&messages[1]);
    let output: serde_json::Value =
        serde_json::from_str(&result.output).expect("tool output 必须是合法 JSON");

    assert_eq!(call.id, "call-1");
    assert_eq!(call.name, "builtin.search");
    assert_eq!(result.call_id, "call-1");
    assert!(result.success);
    assert_eq!(output["status"], "completed");
    assert_eq!(output["summary"], "ok");
    assert_eq!(output["error_type"], "none");
}

#[test]
fn oversized_tool_result_keeps_completion_envelope_and_valid_json() {
    const TOOL_OUTPUT_BUDGET_BYTES: usize = 4_000;
    let mut source = receipt("call-1");
    source.summary = "glob entries=300".to_owned();
    source.metadata = Some(serde_json::json!({
        "entries": (0..300)
            .map(|index| serde_json::json!({"name": format!("entry-{index:03}")}))
            .collect::<Vec<_>>(),
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        AgentRunToolOutputPolicy::with_max_output_bytes(TOOL_OUTPUT_BUDGET_BYTES),
    )
    .expect("超大工具结果应生成有界消息");
    let result = tool_result(&messages[1]);
    let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();

    assert!(result.success);
    assert_eq!(output["status"], "completed");
    assert_eq!(output["summary"], "glob entries=300");
    assert_eq!(output["metadata"]["truncated"], true);
}

#[test]
fn oversized_tool_summary_preserves_head_and_tail_within_output_budget() {
    const TOOL_OUTPUT_BUDGET_BYTES: usize = 4_000;
    const SUMMARY_HEAD: &str = "read_file path=notes.md lines=1-500\n1:开头内容";
    const SUMMARY_TAIL: &str = "500:结尾内容";
    let mut source = receipt("call-1");
    source.tool_name = "builtin.read_file".to_owned();
    source.summary = format!("{SUMMARY_HEAD}{}{SUMMARY_TAIL}", "中间内容".repeat(1_000));
    source.metadata = Some(serde_json::json!({
        "kind": "read_file",
        "path": "notes.md",
        "page": {"totalLines": 500},
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        AgentRunToolOutputPolicy::with_max_output_bytes(TOOL_OUTPUT_BUDGET_BYTES),
    )
    .expect("超长文件正文应生成有界消息");
    let result = tool_result(&messages[1]);
    let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
    let summary = output["summary"].as_str().expect("应保留正文摘要");

    assert!(result.output.len() <= TOOL_OUTPUT_BUDGET_BYTES);
    assert!(summary.starts_with(SUMMARY_HEAD));
    assert!(summary.ends_with(SUMMARY_TAIL));
    assert!(summary.contains("tool output truncated"));
    assert_eq!(output["metadata"]["truncated"], true);
}

#[test]
fn local_images_follow_their_paired_tool_result() {
    let mut source = receipt("call-1");
    source.metadata = Some(serde_json::json!({
        "localImages": [{"path": "C:\\workspace\\screen.png", "mimeType": "image/png"}]
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        default_output_policy(),
    )
    .expect("图片工具回执应可生成下一轮消息");

    assert_eq!(messages.len(), 3);
    assert_eq!(tool_result(&messages[1]).call_id, "call-1");
    assert!(messages[2].cache_excluded);
    assert_eq!(
        messages[2].local_images[0].path,
        "C:\\workspace\\screen.png"
    );
}

#[test]
fn failure_feedback_stays_inside_the_same_tool_result() {
    let call = tool_call("call-1");
    let mut failed = receipt("call-1");
    failed.status = "failed".to_owned();
    failed.summary = "builtin.search arguments 缺少 query".to_owned();
    failed.error_type = Some("execution_failed".to_owned());

    let messages =
        agent_run_tool_result_messages(&[call], &[failed], None, default_output_policy()).unwrap();
    let result = tool_result(&messages[1]);
    let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();

    assert_eq!(messages.len(), 2);
    assert!(!result.success);
    assert!(output["feedback"].get("corrected_tool").is_none());
    assert_eq!(output["feedback"]["missing_fields"][0], "query");
    assert!(output["feedback"]["retry_hint"].is_string());
}

#[test]
fn internal_model_messages_are_injected_once_and_hidden_from_tool_output() {
    let mailbox = AgentModelMessage::cache_excluded("user", "child completed");
    let mut source = receipt("call-1");
    source.metadata = Some(serde_json::json!({
        "modelMessages": [mailbox],
        "visible": "ok"
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        default_output_policy(),
    )
    .unwrap();
    let result = tool_result(&messages[1]);
    let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();

    assert_eq!(messages.len(), 3);
    assert!(output["metadata"].get("modelMessages").is_none());
    assert_eq!(output["metadata"]["visible"], "ok");
    assert_eq!(messages[2].content, "child completed");
    assert!(messages[2].cache_excluded);
}

/// 并行批次内，靠前 receipt 携带的附注不得插在同批 `tool` 结果之间。
///
/// 反例来源：`take_mailbox_model_messages` 会把 mailbox 消息挂到**第一个**子代理工具回执上；
/// 逐 receipt 内联发射会让第二条 `tool` 与 `assistant(tool_calls)` 被注记隔开，
/// OpenAI 兼容网关实测整条拒绝（`400 / 400001 invalid_request_error`）。
#[test]
fn sibling_tool_results_stay_adjacent_when_an_earlier_receipt_carries_annotations() {
    let mailbox = AgentModelMessage::cache_excluded("user", "child completed");
    let mut first = receipt("call-1");
    first.metadata = Some(serde_json::json!({
        "modelMessages": [mailbox],
        "localImages": [{"path": "C:\\workspace\\screen.png", "mimeType": "image/png"}]
    }));
    let second = receipt("call-2");

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1"), tool_call("call-2")],
        &[first, second],
        None,
        default_output_policy(),
    )
    .expect("并行批次应可生成下一轮消息");

    assert_eq!(messages.len(), 5);
    assert_eq!(tool_result(&messages[1]).call_id, "call-1");
    assert_eq!(tool_result(&messages[2]).call_id, "call-2");
    // 附注只能排在整批 tool 结果之后，且保持 receipt 内既有顺序（注记 → 图片提示）。
    assert_eq!(messages[3].content, "child completed");
    assert!(messages[3].cache_excluded);
    assert_eq!(
        messages[4].local_images[0].path,
        "C:\\workspace\\screen.png"
    );
}

#[test]
fn archive_backed_tool_image_keeps_its_durable_reference() {
    let mut source = receipt("call-1");
    source.metadata = Some(serde_json::json!({
        "localImages": [{
            "path": "C:\\workspace\\shot.png",
            "mimeType": "image/png",
            "byteLen": 68,
            "sha256": "b".repeat(64),
            "archiveRef": "blobs/bbbb.blob",
            "sourceRef": "tool://builtin.read_image/shot.png"
        }]
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        default_output_policy(),
    )
    .expect("归档图片应可生成下一轮消息");

    let image = &messages[2].local_images[0];
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.path, "C:\\workspace\\shot.png");
    assert_eq!(image.byte_len, Some(68));
    assert_eq!(image.sha256.as_deref(), Some("b".repeat(64).as_str()));
    assert_eq!(image.archive_ref.as_deref(), Some("blobs/bbbb.blob"));
    assert_eq!(
        image.source_ref.as_deref(),
        Some("tool://builtin.read_image/shot.png")
    );
}

#[test]
fn archive_only_tool_image_without_local_path_is_still_delivered() {
    let mut source = receipt("call-1");
    source.metadata = Some(serde_json::json!({
        "localImages": [{
            "mimeType": "image/jpeg",
            "archiveRef": "blobs/cccc.blob"
        }]
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        default_output_policy(),
    )
    .expect("仅带归档引用的图片应可投影");

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2].local_images[0].mime_type, "image/jpeg");
    assert_eq!(
        messages[2].local_images[0].archive_ref.as_deref(),
        Some("blobs/cccc.blob")
    );
}

#[test]
fn tool_image_without_path_or_archive_is_dropped() {
    let mut source = receipt("call-1");
    source.metadata = Some(serde_json::json!({
        "localImages": [{ "mimeType": "image/png" }]
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        default_output_policy(),
    )
    .expect("无字节来源的图片不得中断批次");

    assert_eq!(messages.len(), 2);
}

#[test]
fn low_budget_tool_result_keeps_output_ref_in_minimal_fallback() {
    const TOOL_OUTPUT_BUDGET_BYTES: usize = 1_024;
    let mut source = receipt("call-1");
    source.summary = "x".repeat(2_000);
    source.output_ref = Some(OutputRef {
        id: OutputRefId(7),
        byte_len: 2_000,
        sha256: "a".repeat(64),
        source_trace: "tool://builtin/shell_command".to_owned(),
        preview: "x".repeat(16_384),
        archive_ref: Some(OutputArchiveReference {
            relative_path: "archive/full.log".to_owned(),
        }),
    });
    source.metadata = Some(serde_json::json!({
        "archivePath": "C:\\axon\\data\\archive\\full.log"
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        AgentRunToolOutputPolicy::with_max_output_bytes(TOOL_OUTPUT_BUDGET_BYTES),
    )
    .expect("低预算仍应生成合法回执");
    let result = tool_result(&messages[1]);
    let output: serde_json::Value =
        serde_json::from_str(&result.output).expect("fallback 应为合法 JSON");

    assert!(result.output.len() <= TOOL_OUTPUT_BUDGET_BYTES);
    assert_eq!(output["output_ref"]["id"], 7);
    assert_eq!(
        output["output_ref"]["sourceTrace"],
        "tool://builtin/shell_command"
    );
    assert_eq!(output["output_ref"]["preview"], "");
    assert_eq!(
        output["output_ref"]["archiveRef"]["relativePath"],
        "archive/full.log"
    );
    assert_eq!(
        output["output_ref"]["readPath"],
        "C:\\axon\\data\\archive\\full.log"
    );
    assert!(output["archive_read_hint"].as_str().is_some());
}

#[test]
fn oversized_shell_receipt_keeps_session_id_at_top_level() {
    let mut source = receipt("call-1");
    source.tool_name = "builtin.shell_command".to_owned();
    source.summary = "cargo check --workspace ".repeat(2_000);
    source.metadata = Some(serde_json::json!({
        "kind": "exec",
        "sessionId": "session-real-123",
        "output": {"stdout": {"preview": "large"}}
    }));

    let messages = agent_run_tool_result_messages(
        &[tool_call("call-1")],
        &[source],
        None,
        AgentRunToolOutputPolicy::with_max_output_bytes(1_024),
    )
    .expect("超长 shell 回执应生成有界消息");
    let result = tool_result(&messages[1]);
    let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();

    assert_eq!(output["session_id"], "session-real-123");
    assert_eq!(output["metadata"]["truncated"], true);
}

fn tool_result(message: &AgentModelMessage) -> &crate::AgentModelToolResultMetadata {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_result.as_ref())
        .expect("应生成结构化 tool result")
}
