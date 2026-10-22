//! Thread 模型历史投影：注入上下文原序回放与原生工具条目恢复。

use super::*;
use crate::run_commands::{RolloutSqliteStore, resolved_rollout_db_path};

#[test]
fn injected_context_replays_verbatim_before_turn_user_message() {
    let mut record = session_record("用户问题", "最终回答", 100);
    record.injected_context_messages = vec![
        AgentModelMessage::cache_excluded(
            "developer",
            "<environment-context>local_time=10:00</environment-context>",
        ),
        AgentModelMessage::context(
            "developer",
            "<agent-context-snapshot>ledger=v1</agent-context-snapshot>",
        ),
    ];
    let expected = record.injected_context_messages.clone();

    let detail = thread_detail_from_records(&[record]).unwrap();
    let messages = parsed_model_history(&detail);

    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages[..2],
        expected[..],
        "注入上下文必须逐字段回放，否则整段会话历史失去前缀缓存"
    );
    assert_eq!(messages[2].content, "用户问题");
    assert_eq!(messages[3].content, "最终回答");
}

#[test]
fn injected_context_survives_sqlite_record_roundtrip() {
    let root = temp_root("injected-record-roundtrip");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    axon_config::ConfigLoader::save_to_path(&config_path, &axon_config::AxonConfig::default())
        .unwrap();
    let mut record = session_record("用户问题", "最终回答", 100);
    record.injected_context_messages = vec![
        AgentModelMessage::cache_excluded(
            "developer",
            "<environment-context>local_time=10:00</environment-context>",
        ),
        AgentModelMessage::context(
            "developer",
            "<prompt-context>selected_skill=none</prompt-context>",
        ),
    ];
    let expected = record.injected_context_messages.clone();

    let db_path = resolved_rollout_db_path(&config_path).unwrap();
    let store = RolloutSqliteStore::open(&db_path).unwrap();
    store
        .record_session(&record)
        .expect("写入 rollout 必须成功");
    let restored = store
        .load_by_id(&record.session_id)
        .expect("读取 rollout 必须成功")
        .expect("rollout 记录必须存在");

    assert_eq!(
        restored.injected_context_messages, expected,
        "注入上下文必须原样通过 SQLite 记录往返"
    );
    let detail = thread_detail_from_records(&[restored]).unwrap();
    let messages = parsed_model_history(&detail);
    assert_eq!(
        messages[..2],
        expected[..],
        "恢复后的模型历史必须把注入上下文放回用户输入之前"
    );
    assert_eq!(messages[2].content, "用户问题");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn model_history_projects_native_tool_call_and_result_from_same_items() {
    let detail = thread_detail_from_records(&[session_record_with_tools()]).unwrap();

    let messages = parsed_model_history(&detail);
    let calls = model_tool_calls(&messages[1]);
    let result = model_tool_result(&messages[2]).expect("工具 result 必须使用强类型 metadata");

    assert_eq!(messages.len(), 4);
    assert_eq!(calls[0].id, "call-search");
    assert_eq!(calls[0].input.canonical_text(), r#"{"query":"完整参数"}"#);
    assert_eq!(result.call_id, "call-search");
    assert!(result.output.contains("找到结果"));
    assert!(result.success);
}
