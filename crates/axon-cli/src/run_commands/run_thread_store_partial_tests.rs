//! 中途落盘回归：历史按身份增量合并，用户消息在重复落盘下保持唯一。

use super::*;

/// 回执在工具执行后到达，item 生成顺序不是「只追加尾部」。
///
/// 中途落盘若按位置切片，会把回执插进错误位置并丢掉第一条调用的结果；崩溃/失败路径
/// 不会再写终态，错位历史会永久留在模型可见历史里。
#[test]
fn partial_write_with_settled_receipts_keeps_history_consistent() {
    let root = temp_root("partial-interleaved");
    let config_path = root.join("config.toml");
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut started = session_record("用户问题", "", 300);
    started.tool_calls = vec![
        ProviderToolCall::new(
            "call-one",
            "builtin.shell_command",
            r#"{"command":"cargo check"}"#,
        ),
        ProviderToolCall::new("call-two", "builtin.read_file", r#"{"path":"Cargo.toml"}"#),
    ];
    started.turn_events = vec![
        tool_started_event("tool-part:stream:1:0:call-one", "call-one"),
        tool_started_event("tool-part:stream:1:1:call-two", "call-two"),
    ];
    mirror_session_started(&config_path, &started).unwrap();

    let mut settled = started.clone();
    settled.tool_receipts = vec![
        crate::run_commands::run_execution::RunToolReceipt {
            tool_call_id: 1,
            part_id: "tool-part:stream:1:0:call-one".to_owned(),
            provider_call_id: "call-one".to_owned(),
            tool_name: "builtin.shell_command".to_owned(),
            status: "completed".to_owned(),
            summary: "check 通过".to_owned(),
            metadata: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: Some(3),
        },
        crate::run_commands::run_execution::RunToolReceipt {
            tool_call_id: 2,
            part_id: "tool-part:stream:1:1:call-two".to_owned(),
            provider_call_id: "call-two".to_owned(),
            tool_name: "builtin.read_file".to_owned(),
            status: "completed".to_owned(),
            summary: "读到清单".to_owned(),
            metadata: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: Some(1),
        },
    ];
    settled.response = "两个工具都完成".to_owned();
    crate::run_commands::run_thread_partial::mirror_session_partial(&config_path, &settled)
        .unwrap();

    let detail = service.read_thread(&root, &settled.session_id).unwrap();
    let items = &detail.turns[0].history_items;
    let mut identities = items.iter().map(|item| item.identity()).collect::<Vec<_>>();
    let total = identities.len();
    identities.sort_unstable();
    identities.dedup();
    assert_eq!(identities.len(), total, "同一身份的历史 item 不得重复出现");

    let calls = items
        .iter()
        .filter_map(|item| match item {
            ThreadHistoryItem::ToolCall {
                provider_call_id,
                status,
                ..
            } => Some((provider_call_id.as_str(), *status)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let results = items
        .iter()
        .filter_map(|item| match item {
            ThreadHistoryItem::ToolResult {
                provider_call_id, ..
            } => Some(provider_call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2, "两条调用都必须保留");
    assert_eq!(results.len(), 2, "两条结果都必须写进本轮历史");
    assert!(
        calls
            .iter()
            .all(|(_, status)| *status == ThreadHistoryStatus::Completed),
        "回执到达后调用状态必须更新成已完成"
    );
    for (call_id, _) in &calls {
        assert!(results.contains(call_id), "结果必须能对上调用：{call_id}");
    }

    let history_json = thread_model_history_json(&detail).unwrap();
    let messages: Vec<axon_agent_runtime::AgentModelMessage> =
        serde_json::from_str(&history_json).unwrap();
    let serialized = history_json.as_str();
    assert_eq!(
        serialized.matches("读到清单").count(),
        1,
        "工具结果正文必须进入模型可见历史且只出现一次"
    );
    assert_eq!(
        serialized.matches("check 通过").count(),
        1,
        "第二条工具结果同样必须进入模型可见历史"
    );
    let _ = messages;
    let _ = std::fs::remove_dir_all(root);
}

/// 增量落盘不得重复追加用户消息：同一轮的 part_id 必须与落盘时间无关。
#[test]
fn repeated_partial_writes_keep_a_single_user_message() {
    let root = temp_root("partial-user-singleton");
    let config_path = root.join("config.toml");
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut first = session_record("用户问题", "", 300);
    first.tool_calls = vec![ProviderToolCall::new(
        "call-one",
        "builtin.shell_command",
        r#"{"command":"cargo check"}"#,
    )];
    first.turn_events = vec![tool_started_event(
        "tool-part:stream:1:0:call-one",
        "call-one",
    )];
    mirror_session_started(&config_path, &first).unwrap();

    let mut second = first.clone();
    // 生产里每次落盘都是新构造的记录：created_at_ms 会变化，但轮次身份不变。
    second.created_at_ms = 301;
    crate::run_commands::run_thread_partial::mirror_session_partial(&config_path, &second).unwrap();
    let mut third = second.clone();
    third.created_at_ms = 302;
    crate::run_commands::run_thread_partial::mirror_session_partial(&config_path, &third).unwrap();

    let detail = service.read_thread(&root, &third.session_id).unwrap();
    let users = detail.turns[0]
        .history_items
        .iter()
        .filter(|item| matches!(item, ThreadHistoryItem::UserMessage { .. }))
        .count();
    assert_eq!(users, 1, "同一轮只能有一条用户消息");
    let _ = std::fs::remove_dir_all(root);
}
