//! Thread 模型历史投影：注入上下文原序回放与原生工具条目恢复。

use super::*;

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
fn injected_context_survives_thread_roundtrip() {
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

    mirror_session_to_thread(&config_path, &record).expect("写入 thread 必须成功");
    let identities = super::super::super::RunIdentities::new(
        record.session_id.clone(),
        record.thread_id.clone(),
    );
    let restored = turn_record(&config_path, &identities, &record.assistant_part_id)
        .expect("读取 thread 必须成功")
        .expect("thread 轮次必须存在");

    assert_eq!(
        restored.injected_context_messages, expected,
        "注入上下文必须原样通过 thread 事件流往返"
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

/// 收口闭环：轮次落 thread 之后，轮序随侧车一起可点查。
///
/// 治理回写（反思提示、`trace:<session>:<sequence>` 标识）只查侧车，不再为了一个标量
/// 读 thread 尾窗；缺行时读侧会回落整条 thread 读，长会话上就是每轮一次 O(语料)。
#[test]
fn settled_turn_sequence_lands_in_the_sidecar() {
    use axon_session::RuntimeStore as _;

    let root = temp_root("turn-facts-sequence");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    axon_config::ConfigLoader::save_to_path(&config_path, &axon_config::AxonConfig::default())
        .unwrap();
    // 侧车只写既存的控制面库：先把库建出来，收口才会落行。
    let data_dir = crate::runtime_paths::runtime_axon_data_path_static(&config_path)
        .expect("运行态数据目录应可解析");
    std::fs::create_dir_all(&data_dir).unwrap();
    axon_session::SqliteRuntimeStore::from_root(&data_dir)
        .initialize()
        .expect("控制面库应可初始化");

    let first = session_record("第一轮问题", "第一轮回答", 100);
    mirror_session_to_thread(&config_path, &first).expect("首轮收口必须成功");
    let mut second = session_record("第二轮问题", "第二轮回答", 200);
    second.assistant_part_id = "assistant:session-1:second".to_owned();
    mirror_session_to_thread(&config_path, &second).expect("次轮收口必须成功");

    let facts = crate::run_commands::run_thread_facts::turn_facts(
        &config_path,
        &second.thread_id,
        &second.assistant_part_id,
    )
    .expect("侧车必须有一行");
    assert_eq!(
        facts.sequence,
        Some(1),
        "轮序必须随侧车落地，否则治理回写每轮都要退回整条 thread 读"
    );
    assert_eq!(facts.recorded_at_ms, 200, "收口时刻必须是本轮的时间");
    let _ = std::fs::remove_dir_all(root);
}
