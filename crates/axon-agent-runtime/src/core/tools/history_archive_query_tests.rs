//! 归档历史查询引擎行为测试。
//!
//! 覆盖 unknown window、item_id 解析、tool_namespace 过滤、recent_first 顺序、
//! 归档分片损坏时的显式失败，以及 provider 可见 `[id: ...]` 标记与归档 ordinal 的一致性。

use std::path::Path;

use axon_domain::ToolCallInput;

use super::history_archive_query::{
    HistoryItemFilter, history_item_id, history_list_items, history_list_windows,
    history_read_item, history_search_contents, parse_history_item_id, tool_name_namespace,
};
use crate::{
    ARCHIVED_HISTORY_DIR_NAME, ARCHIVED_WINDOW_SCHEMA_VERSION, AgentModelMessage,
    AgentModelMessageProviderMetadata, AgentModelToolCallMetadata, ArchivedWindowRecord,
    WindowRolloverEngine, archive_shard_file_name,
};
use axon_jsonl::ShardedJsonlStore;

const WINDOW_ONE_ID: &str = "window-uuid-1";
const WINDOW_TWO_ID: &str = "window-uuid-2";

fn tool_call_message(name: &str) -> AgentModelMessage {
    let mut message = AgentModelMessage::text("assistant", format!("调用 {name}"));
    message.provider_metadata = Some(AgentModelMessageProviderMetadata {
        tool_calls: Some(vec![AgentModelToolCallMetadata {
            id: format!("call-{name}"),
            name: name.to_owned(),
            input: ToolCallInput::json(serde_json::json!({})),
        }]),
        ..Default::default()
    });
    message
}

fn archive_window(
    session_dir: &Path,
    window_number: u64,
    window_id: &str,
    messages: &[AgentModelMessage],
) {
    let records = messages
        .iter()
        .enumerate()
        .map(|(ordinal, message)| ArchivedWindowRecord {
            schema_version: ARCHIVED_WINDOW_SCHEMA_VERSION,
            lifecycle_id: format!("test-lifecycle-{window_number}"),
            checksum: String::new(),
            window_number,
            window_id: window_id.to_owned(),
            previous_window_id: None,
            archived_at_ms: 1_000,
            message: message.clone(),
            message_ordinal: u64::try_from(ordinal).unwrap_or(u64::MAX),
        })
        .collect::<Vec<_>>();
    WindowRolloverEngine::append_to_archive(session_dir, &records).expect("测试归档应写入成功");
}

fn empty_filter() -> HistoryItemFilter {
    HistoryItemFilter {
        window_id: None,
        role: None,
        tool_name: None,
        tool_namespace: None,
    }
}

#[test]
fn unknown_window_lists_and_searches_return_empty() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    archive_window(
        session_dir,
        1,
        WINDOW_ONE_ID,
        &[AgentModelMessage::text("user", "第一条请求")],
    );

    let filter = HistoryItemFilter {
        window_id: Some("unknown-window".to_owned()),
        ..empty_filter()
    };
    let listed = history_list_items(session_dir, &filter, 10, false, 100).expect("列举应成功");
    assert!(listed.items.is_empty());
    assert_eq!(listed.total_items, 0);

    let searched =
        history_search_contents(session_dir, "第一条请求", &filter, 10, false).expect("检索应成功");
    assert!(searched.matches.is_empty());

    let error = history_read_item(session_dir, "unknown-window", "unknown-window:0", 0, 100)
        .expect_err("未知窗口读取必须显式报错");
    assert!(format!("{error:?}").contains("未知窗口"));
}

#[test]
fn item_id_parsing_and_tool_namespace_mapping() {
    assert_eq!(
        parse_history_item_id("window-uuid-1:7"),
        Some(("window-uuid-1".to_owned(), 7))
    );
    assert_eq!(
        parse_history_item_id("window-uuid-1:7:9"),
        Some(("window-uuid-1:7".to_owned(), 9))
    );
    for invalid in ["no-separator", ":7", "window-uuid-1:x", "window-uuid-1:"] {
        assert_eq!(
            parse_history_item_id(invalid),
            None,
            "非法 item_id 必须被拒绝"
        );
    }

    assert_eq!(tool_name_namespace("builtin.read_file"), "builtin");
    assert_eq!(tool_name_namespace("mcp__search__query"), "mcp__search");
    assert_eq!(tool_name_namespace("bare_tool"), "bare_tool");
}

#[test]
fn list_items_filters_by_tool_namespace_and_supports_recent_first() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    archive_window(
        session_dir,
        1,
        WINDOW_ONE_ID,
        &[
            AgentModelMessage::text("user", "第一条请求"),
            tool_call_message("builtin.read_file"),
            tool_call_message("mcp__search__query"),
        ],
    );

    let filter = HistoryItemFilter {
        tool_namespace: Some("mcp__search".to_owned()),
        ..empty_filter()
    };
    let filtered = history_list_items(session_dir, &filter, 10, false, 100).expect("列举应成功");
    assert_eq!(filtered.total_items, 1);
    assert_eq!(
        filtered.items[0].tool_name.as_deref(),
        Some("mcp__search__query")
    );

    let forward =
        history_list_items(session_dir, &empty_filter(), 10, false, 100).expect("列举应成功");
    let ordinals = forward
        .items
        .iter()
        .map(|item| item.ordinal)
        .collect::<Vec<_>>();
    assert_eq!(ordinals, vec![0, 1, 2]);

    let recent =
        history_list_items(session_dir, &empty_filter(), 10, true, 100).expect("列举应成功");
    let ordinals = recent
        .items
        .iter()
        .map(|item| item.ordinal)
        .collect::<Vec<_>>();
    assert_eq!(ordinals, vec![2, 1, 0]);
}

#[test]
fn annotated_item_ids_resolve_to_archived_ordinals() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let messages = vec![
        AgentModelMessage::text("system", "系统指令"),
        AgentModelMessage::text("user", "目标请求"),
        tool_call_message("builtin.read_file"),
        AgentModelMessage::text("tool", "文件内容"),
    ];
    // 归档使用与 Provider 请求一致的归一化视图，对齐生产轮转路径。
    let normalized = crate::agent_model_tool_history::normalize_provider_history(&messages, false)
        .messages
        .into_owned();
    archive_window(session_dir, 2, WINDOW_TWO_ID, &normalized);

    let annotated = crate::annotate_history_item_ids(
        &crate::AgentModelHistorySnapshot::from(messages)
            .with_history_window_id(Some(WINDOW_TWO_ID.to_owned())),
        false,
    );

    let mut marked = 0_usize;
    for (ordinal, message) in annotated.iter().enumerate() {
        if matches!(message.role.as_str(), "assistant" | "system" | "developer")
            || message.content.trim().is_empty()
        {
            continue;
        }
        let item_id = history_item_id(WINDOW_TWO_ID, u64::try_from(ordinal).unwrap_or(u64::MAX));
        let marker = format!("[id: {item_id}]");
        assert!(
            message.content.ends_with(&marker),
            "非 assistant 会话条目必须带 item id 标记：{}",
            message.content
        );
        let (window_id, archived_ordinal) =
            parse_history_item_id(&item_id).expect("标记中的 item_id 应可解析");
        assert_eq!(window_id, WINDOW_TWO_ID);
        let read = history_read_item(session_dir, &window_id, &item_id, 0, 4_000)
            .expect("标记中的 item_id 必须能在归档中定位");
        assert_eq!(read.item_id, history_item_id(&window_id, archived_ordinal));
        marked += 1;
    }
    assert_eq!(marked, 2, "user 与 tool 条目都应带标记");

    for message in annotated.iter() {
        if message.role == "system" || message.role == "developer" {
            assert!(
                !message.content.contains("[id: "),
                "稳定前缀消息不得带 item id 标记"
            );
        }
    }
}

#[test]
fn annotation_does_not_mutate_stable_prefix_identity() {
    let messages = vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text("developer", "稳定开发者约束"),
        AgentModelMessage::text("user", "当前请求"),
        AgentModelMessage::text("assistant", "回答"),
    ];
    let baseline = prompt_cache_plan(&messages);
    let annotated = crate::annotate_history_item_ids(
        &crate::AgentModelHistorySnapshot::from(messages)
            .with_history_window_id(Some(WINDOW_ONE_ID.to_owned())),
        false,
    );
    let after = prompt_cache_plan(annotated.as_slice());

    assert_eq!(baseline.stable_prefix_hash, after.stable_prefix_hash);
    assert_eq!(baseline.namespace_key, after.namespace_key);
}

#[test]
fn tool_outputs_and_call_arguments_are_searchable_and_readable() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();

    let mut call_message = AgentModelMessage::text("assistant", "");
    call_message.provider_metadata = Some(AgentModelMessageProviderMetadata {
        tool_calls: Some(vec![AgentModelToolCallMetadata {
            id: "call-1".to_owned(),
            name: "builtin.shell_command".to_owned(),
            input: ToolCallInput::json(serde_json::json!({"command": "cargo test -p axon-cli"})),
        }]),
        ..Default::default()
    });
    let tool_message = AgentModelMessage::tool_result("call-1", "全仓编译成功 100%", true);
    archive_window(session_dir, 1, WINDOW_ONE_ID, &[call_message, tool_message]);

    let filter = empty_filter();
    let searched_calls =
        history_search_contents(session_dir, "cargo test -p axon-cli", &filter, 10, false)
            .expect("检索应成功");
    assert_eq!(searched_calls.matches.len(), 1);
    assert_eq!(
        searched_calls.matches[0].item_id,
        history_item_id(WINDOW_ONE_ID, 0)
    );
    assert!(
        searched_calls.matches[0]
            .snippet
            .contains("[Tool Call: builtin.shell_command]")
    );

    let searched_output = history_search_contents(session_dir, "全仓编译成功", &filter, 10, false)
        .expect("检索应成功");
    assert_eq!(searched_output.matches.len(), 1);
    assert_eq!(
        searched_output.matches[0].item_id,
        history_item_id(WINDOW_ONE_ID, 1)
    );
    assert_eq!(searched_output.matches[0].role, "tool");

    let read = history_read_item(
        session_dir,
        WINDOW_ONE_ID,
        &history_item_id(WINDOW_ONE_ID, 1),
        0,
        4_000,
    )
    .expect("回读应成功");
    assert!(read.text.contains("[Tool Result]: 全仓编译成功 100%"));
    assert_eq!(read.tool_name, None);
}

#[test]
fn history_queries_reject_blank_query_and_zero_limits() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let filter = empty_filter();

    for query in ["", "   ", "\t\n"] {
        let error = history_search_contents(session_dir, query, &filter, 1, false)
            .expect_err("空白 query 必须 fail-closed");
        assert!(format!("{error:?}").contains("不能为空"));
    }

    let error = history_list_windows(session_dir, 0, false)
        .expect_err("history_list_windows 的零 limit 必须被拒绝");
    assert!(format!("{error:?}").contains("limit"));

    let error = history_list_items(session_dir, &filter, 0, false, 100)
        .expect_err("history_list_items 的零 limit 必须被拒绝");
    assert!(format!("{error:?}").contains("limit"));

    let error = history_list_items(session_dir, &filter, 1, false, 0)
        .expect_err("max_chars_per_item=0 必须被拒绝");
    assert!(format!("{error:?}").contains("max_chars_per_item"));

    let error = history_read_item(session_dir, "missing", "missing:0", 0, 0)
        .expect_err("limit_chars=0 必须被拒绝");
    assert!(format!("{error:?}").contains("limit_chars"));
}

#[test]
fn corrupted_shard_reports_explicit_failure_instead_of_unknown_window() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let archive_dir = session_dir.join(ARCHIVED_HISTORY_DIR_NAME);
    std::fs::create_dir_all(&archive_dir).expect("归档目录应可创建");
    // 模拟崩溃中断的尾部半行：分片存在，但无法解析出任何有效记录。
    let logical_shard = archive_dir.join(archive_shard_file_name(1));
    std::fs::write(
        ShardedJsonlStore::physical_shard_path(&logical_shard, 0),
        r#"{"schema_version":1,"window_id":"window-uuid-1""#,
    )
    .expect("损坏分片应可写入");

    let filter = HistoryItemFilter {
        window_id: Some(WINDOW_ONE_ID.to_owned()),
        ..empty_filter()
    };
    let error =
        history_list_items(session_dir, &filter, 10, false, 100).expect_err("归档损坏必须显式失败");
    assert!(format!("{error:?}").contains("归档分片损坏"));
}

fn prompt_cache_plan(messages: &[AgentModelMessage]) -> crate::ProviderPromptCachePlan {
    crate::ProviderPromptCachePlanRuntime
        .plan(crate::ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_OPENAI,
            ),
            format: crate::PromptCacheProviderWireFormat::OpenAiResponses,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: crate::AgentModelHistorySnapshot::from(messages.to_vec()),
            model_tools: Vec::new(),
            context_section_count: 1,
        })
        .expect("prompt cache plan 应生成成功")
}

/// 列举条目必须严格受 limit 约束，同时精确统计命中总数。
#[test]
fn list_items_truncates_to_limit_and_keeps_exact_total() {
    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    archive_window(
        session_dir,
        1,
        WINDOW_ONE_ID,
        &[
            AgentModelMessage::text("user", "第一条请求"),
            AgentModelMessage::text("assistant", "第一条答复"),
            AgentModelMessage::text("user", "第二条请求"),
        ],
    );

    let listed =
        history_list_items(session_dir, &empty_filter(), 2, false, 100).expect("列举应成功");
    assert_eq!(listed.items.len(), 2);
    assert_eq!(listed.total_items, 3);

    let recent =
        history_list_items(session_dir, &empty_filter(), 2, true, 100).expect("列举应成功");
    assert_eq!(
        recent
            .items
            .iter()
            .map(|item| item.ordinal)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert_eq!(recent.total_items, 3);
}
