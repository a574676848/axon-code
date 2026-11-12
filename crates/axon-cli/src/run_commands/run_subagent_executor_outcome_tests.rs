use super::*;

#[test]
fn memory_write_count_only_accepts_completed_receipts_with_persisted_ids() {
    let receipt = |tool_name: &str, status: &str, memory_id: serde_json::Value| {
        crate::run_commands::run_tool_loop::RunToolReceipt {
            tool_call_id: 0,
            part_id: "tool-part:memory-write".to_owned(),
            provider_call_id: format!("{tool_name}:{status}"),
            tool_name: tool_name.to_owned(),
            status: status.to_owned(),
            summary: "memory receipt".to_owned(),
            metadata: Some(serde_json::json!({ "memoryId": memory_id })),
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: None,
        }
    };
    let response = crate::run_commands::run_execution::RunResponse {
        content: "done".to_owned(),
        assistant_part_id: "assistant-part:memory-write".to_owned(),
        http_status: 200,
        token_usage: None,
        tool_receipts: vec![
            receipt("builtin.memory_write", "completed", serde_json::json!(7)),
            receipt("builtin.memory_write", "completed", serde_json::Value::Null),
            receipt("builtin.memory_write", "failed", serde_json::json!(8)),
            receipt("builtin.search", "completed", serde_json::json!(9)),
        ],
        tool_calls: Vec::new(),
        provider: "fixture".to_owned(),
        model: "fixture-chat".to_owned(),
        channel_attempts: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
        max_tokens_truncated: false,
    };

    assert_eq!(persisted_memory_write_count(&response), 1);
}

#[test]
fn empty_result_child_turn_is_marked_errored() {
    fn empty_provider_turn(
        request: &RunRequest,
        _progress: Option<&RunProgressSink<'_>>,
    ) -> Result<RunResponse, Box<RunExecutionError>> {
        Ok(RunResponse {
            content: String::new(),
            assistant_part_id: MOCK_ASSISTANT_PART_ID.to_owned(),
            http_status: MOCK_HTTP_STATUS,
            token_usage: None,
            tool_receipts: Vec::new(),
            tool_calls: Vec::new(),
            provider: request.provider.name.clone(),
            model: request.model.name.clone(),
            channel_attempts: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
            max_tokens_truncated: false,
        })
    }

    let config_path = temp_config_path("empty-result");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        config_path.parent().unwrap().join("AGENTS.md"),
        "子代理空结果规则",
    )
    .unwrap();
    let executor = ProviderRunSubagentExecutor::with_provider_turn_runner(
        fixture_environment(config_path.clone()),
        empty_provider_turn,
    );
    let input = fixture_execution_input("/root/review");
    let output = executor
        .execute(input)
        .expect("provider executor 应返回子代理执行结果");

    assert_eq!(output.status, RunSubagentExecutionStatus::Errored);
    assert!(
        output.summary.contains("未产生任何结果"),
        "空结果摘要应说明未产生任何结果: {}",
        output.summary
    );
    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}
