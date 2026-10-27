use super::*;
use crate::AgentToolRegistryRuntime;

#[test]
fn provider_tool_call_batch_schedules_stable_ids_and_requirements() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![
                provider_call(
                    "call-search",
                    "builtin_search",
                    r#"{"query":"prompt cache","intent":"检索前缀缓存相关事实"}"#,
                ),
                provider_call(
                    "call-shell",
                    "builtin_shell_command",
                    r#"{"command":"Get-Location","intent":"验证命令行为"}"#,
                ),
            ],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(10),
            first_execution_request_id: ExecutionRequestId(30),
            created_at_ms: 4,
        },
    )
    .unwrap();

    assert_eq!(report.entries.len(), 2);
    assert_eq!(report.entries[0].dispatch_order, 0);
    assert_eq!(
        report.entries[0].execution_permission,
        ToolExecutionPermission::Allowed
    );
    assert_eq!(report.entries[0].tool_call_id, ToolCallId(10));
    assert_eq!(
        report.entries[0].concurrency_policy,
        ToolConcurrencyPolicy::ParallelSafe
    );
    assert_eq!(report.entries[1].dispatch_order, 1);
    assert_eq!(
        report.entries[1].execution_permission,
        ToolExecutionPermission::Allowed
    );
    assert_eq!(report.entries[1].tool_call_id, ToolCallId(11));
    assert_eq!(
        report.entries[1].concurrency_policy,
        ToolConcurrencyPolicy::WorkspaceCoordinated
    );
    assert_eq!(
        report.entries[0].execution_request_id,
        ExecutionRequestId(30)
    );
    assert_eq!(
        report.entries[1].execution_request_id,
        ExecutionRequestId(31)
    );
    assert!(
        report
            .reasons
            .contains(&PROVIDER_TOOL_CALL_REASON_BATCH_SCHEDULED.to_owned())
    );
    assert_eq!(
        report.reasons,
        [PROVIDER_TOOL_CALL_REASON_BATCH_SCHEDULED.to_owned()]
    );
}

#[test]
fn hidden_spawn_agent_call_cannot_be_dispatched_and_stays_exclusive() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    let spawn_agent = registry
        .tool_by_qualified_name("builtin.spawn_agent")
        .expect("builtin.spawn_agent 定义必须保留在执行注册表");
    assert!(!spawn_agent.enabled);
    // PAR-1 §1.4：隐藏工具 fail-closed 为独占。
    assert_eq!(
        spawn_agent.concurrency_policy,
        ToolConcurrencyPolicy::Exclusive
    );

    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![provider_call(
                "call-spawn",
                "builtin_spawn_agent",
                r#"{"task_name":"review","task":"检查当前改动"}"#,
            )],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(1),
            first_execution_request_id: ExecutionRequestId(1),
            created_at_ms: 1,
        },
    )
    .unwrap();

    assert!(matches!(
        report.entries[0].execution_permission,
        ToolExecutionPermission::Forbidden { .. }
    ));
    assert_eq!(
        report.entries[0].concurrency_policy,
        ToolConcurrencyPolicy::Exclusive
    );
}

#[test]
fn provider_tool_call_execution_receipts_bind_to_scheduled_calls() {
    let schedule = scheduled_two_tool_calls();
    let complete_summary = "搜索完成并保留全部工具执行上下文".repeat(32);

    let report = ProviderToolCallScheduleRuntime::record_execution_receipts(
        &schedule,
        vec![
            ProviderToolCallExecutionOutcome {
                tool_call_id: ToolCallId(10),
                status: ProviderToolCallExecutionStatus::Completed,
                bounded_summary: complete_summary.clone(),
                output_ref: None,
                error_type: None,
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                completed_at_ms: 100,
            },
            ProviderToolCallExecutionOutcome {
                tool_call_id: ToolCallId(11),
                status: ProviderToolCallExecutionStatus::Failed,
                bounded_summary: "命令被拒绝".to_owned(),
                output_ref: None,
                error_type: Some("execution_failed".to_owned()),
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                completed_at_ms: 101,
            },
        ],
    )
    .unwrap();

    assert_eq!(report.completed_count, 1);
    assert_eq!(report.failed_count, 1);
    assert_eq!(report.stats.total_count, 2);
    assert_eq!(
        report.stats.failures_by_error_type.get("execution_failed"),
        Some(&1)
    );
    assert!(!report.all_completed);
    assert_eq!(report.receipts[0].provider_call_id, "call-search");
    assert_eq!(report.receipts[0].bounded_summary, complete_summary);
    assert_eq!(
        report.receipts[0].execution_request_id,
        ExecutionRequestId(30)
    );
    assert_eq!(report.receipts[1].provider_call_id, "call-shell");
    assert_eq!(
        report.receipts[1].execution_request_id,
        ExecutionRequestId(31)
    );
    assert_eq!(
        report.receipts[1].error_type.as_deref(),
        Some("execution_failed")
    );
    assert!(!report.receipts[0].remote_trigger_allowed);
    assert!(!report.receipts[0].filesystem_write_performed);
}

#[test]
fn provider_tool_call_batch_keeps_allowed_entries_in_same_schedule() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![
                provider_call(
                    "call-search",
                    "builtin_search",
                    r#"{"query":"prompt cache","intent":"检索前缀缓存相关事实"}"#,
                ),
                provider_call(
                    "call-shell",
                    "builtin_shell_command",
                    r#"{"command":"pwd","intent":"验证命令行为"}"#,
                ),
            ],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(20),
            first_execution_request_id: ExecutionRequestId(40),
            created_at_ms: 4,
        },
    )
    .unwrap();

    assert_eq!(report.entries[0].tool_call_id, ToolCallId(20));
    assert_eq!(
        report.entries[0].execution_request_id,
        ExecutionRequestId(40)
    );
    assert_eq!(
        report.entries[0].execution_permission,
        ToolExecutionPermission::Allowed
    );
    assert_eq!(report.entries[1].tool_call_id, ToolCallId(21));
    assert_eq!(
        report.entries[1].execution_request_id,
        ExecutionRequestId(41)
    );
    assert_eq!(
        report.entries[1].execution_permission,
        ToolExecutionPermission::Allowed
    );
}

#[test]
fn provider_shell_tool_calls_are_evaluated_per_invocation() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![
                provider_call(
                    "call-read",
                    "builtin_shell_command",
                    r#"{"command":"Get-Location","intent":"验证命令行为"}"#,
                ),
                provider_call(
                    "call-write",
                    "builtin_shell_command",
                    r#"{"command":"Set-Content out.txt value","intent":"验证命令行为"}"#,
                ),
                provider_call(
                    "call-forbidden",
                    "builtin_shell_command",
                    r#"{"command":"Write-Host a; pwsh -Command Get-Location","intent":"验证命令行为"}"#,
                ),
            ],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(50),
            first_execution_request_id: ExecutionRequestId(60),
            created_at_ms: 4,
        },
    )
    .unwrap();

    assert_eq!(report.entries.len(), 3);
    assert_eq!(
        report.entries[0].execution_permission,
        ToolExecutionPermission::Allowed
    );
    assert_eq!(
        report.entries[1].execution_permission,
        ToolExecutionPermission::Allowed
    );
    assert_eq!(
        report.entries[2].execution_permission,
        ToolExecutionPermission::Allowed
    );
}

/// 作业工具必须有调用级判定：`CallEvaluated` 落到兜底分支会被整体禁止，
/// 那样模型即便拿到 `job_id` 也无法读取或终止自己的作业。
#[test]
fn provider_job_tool_calls_are_evaluated_without_blocking() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![
                provider_call("call-job-output", "builtin.job_output", r#"{"job_id":"1"}"#),
                provider_call("call-job-kill", "builtin.job_kill", r#"{"job_id":"1"}"#),
            ],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(90),
            first_execution_request_id: ExecutionRequestId(90),
            created_at_ms: 4,
        },
    )
    .unwrap();

    for entry in &report.entries {
        assert_eq!(
            entry.execution_permission,
            ToolExecutionPermission::Allowed,
            "{} 不得被「缺少调用级权限判定器」的兜底分支禁止",
            entry.tool_call.name
        );
        let expected = if entry.tool_call.name == "builtin.job_output" {
            ToolConcurrencyPolicy::ParallelSafe
        } else {
            ToolConcurrencyPolicy::Exclusive
        };
        assert_eq!(
            entry.concurrency_policy, expected,
            "{} 的并发声明必须与 PAR-1 第 2 节一致",
            entry.tool_call.name
        );
    }
}

#[test]
fn unregistered_provider_tool_is_forbidden() {
    let registry = ToolRegistry::new();
    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![provider_call("call-unknown", "unknown_tool", "{}")],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(70),
            first_execution_request_id: ExecutionRequestId(80),
            created_at_ms: 4,
        },
    )
    .unwrap();

    assert!(matches!(
        report.entries[0].execution_permission,
        ToolExecutionPermission::Forbidden { .. }
    ));
}

#[test]
fn provider_tool_call_execution_receipts_reject_missing_duplicate_or_unknown_outcomes() {
    let schedule = scheduled_two_tool_calls();

    let missing = ProviderToolCallScheduleRuntime::record_execution_receipts(
        &schedule,
        vec![ProviderToolCallExecutionOutcome {
            tool_call_id: ToolCallId(10),
            status: ProviderToolCallExecutionStatus::Completed,
            bounded_summary: "ok".to_owned(),
            output_ref: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            completed_at_ms: 100,
        }],
    );
    let duplicate = ProviderToolCallScheduleRuntime::record_execution_receipts(
        &schedule,
        vec![
            ProviderToolCallExecutionOutcome {
                tool_call_id: ToolCallId(10),
                status: ProviderToolCallExecutionStatus::Completed,
                bounded_summary: "ok".to_owned(),
                output_ref: None,
                error_type: None,
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                completed_at_ms: 100,
            },
            ProviderToolCallExecutionOutcome {
                tool_call_id: ToolCallId(10),
                status: ProviderToolCallExecutionStatus::Completed,
                bounded_summary: "ok".to_owned(),
                output_ref: None,
                error_type: None,
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                completed_at_ms: 101,
            },
        ],
    );
    let unknown = ProviderToolCallScheduleRuntime::record_execution_receipts(
        &schedule,
        vec![
            ProviderToolCallExecutionOutcome {
                tool_call_id: ToolCallId(10),
                status: ProviderToolCallExecutionStatus::Completed,
                bounded_summary: "ok".to_owned(),
                output_ref: None,
                error_type: None,
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                completed_at_ms: 100,
            },
            ProviderToolCallExecutionOutcome {
                tool_call_id: ToolCallId(12),
                status: ProviderToolCallExecutionStatus::Completed,
                bounded_summary: "ok".to_owned(),
                output_ref: None,
                error_type: None,
                remote_trigger_allowed: false,
                filesystem_write_performed: false,
                completed_at_ms: 101,
            },
        ],
    );

    assert!(missing.is_err());
    assert!(duplicate.is_err());
    assert!(unknown.is_err());
}

#[test]
fn provider_tool_call_with_recoverable_argument_error_is_forbidden() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    let arguments = serde_json::json!({
        crate::PROVIDER_TOOL_CALL_PARSE_ERROR_FIELD: "malformed_arguments",
        crate::PROVIDER_TOOL_CALL_RAW_ARGUMENTS_FIELD: "{\"path\":",
    })
    .to_string();

    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![provider_call(
                "call-invalid",
                "builtin_edit",
                arguments.as_str(),
            )],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(10),
            first_execution_request_id: ExecutionRequestId(30),
            created_at_ms: 4,
        },
    )
    .unwrap();

    assert_eq!(
        report.entries[0].execution_permission,
        ToolExecutionPermission::Forbidden {
            reason: "invalid_arguments: provider 工具参数无法解析；builtin.edit 在 JSON function-call 协议下必须传对象，并将原始文本放入 `input` 字段"
                .to_owned(),
        }
    );
}

fn scheduled_two_tool_calls() -> ProviderToolCallScheduleReport {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![
                provider_call(
                    "call-search",
                    "builtin_search",
                    r#"{"query":"prompt cache","intent":"检索前缀缓存相关事实"}"#,
                ),
                provider_call(
                    "call-shell",
                    "builtin_shell_command",
                    r#"{"command":"Get-Location","intent":"验证命令行为"}"#,
                ),
            ],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(10),
            first_execution_request_id: ExecutionRequestId(30),
            created_at_ms: 4,
        },
    )
    .unwrap()
}

#[test]
fn provider_tool_call_batch_rejects_empty_and_accepts_large_schedule() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let empty = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: Vec::new(),
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(10),
            first_execution_request_id: ExecutionRequestId(30),
            created_at_ms: 4,
        },
    );
    let large = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: (0..17)
                .map(|index| {
                    provider_call(
                        format!("call-{index}"),
                        "builtin_search",
                        r#"{"query":"hello","intent":"检索 hello"}"#,
                    )
                })
                .collect(),
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(10),
            first_execution_request_id: ExecutionRequestId(30),
            created_at_ms: 4,
        },
    );

    assert!(empty.is_err());
    assert_eq!(large.unwrap().entries.len(), 17);
}

#[test]
fn provider_mcp_tool_schedule_allows_governed_write() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    registry
        .register(axon_domain::ToolDefinition {
            id: axon_domain::ToolId(900),
            namespace: "mcp__team_tools".to_owned(),
            name: "query_db".to_owned(),
            description: "query db".to_owned(),
            schema: axon_domain::ToolSchema::new(
                1,
                serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
            ),
            enabled: true,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            output_policy: axon_domain::ToolOutputPolicy::RefOnOverflow,
            permission_policy: ToolPermissionPolicy::GovernedWrite,
        })
        .unwrap();
    let resolution = resolve_provider_tool_name(&registry, "mcp__team_tools__query_db");
    assert_eq!(resolution.canonical_name, "mcp__team_tools__query_db");

    let report = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![provider_call(
                "call-mcp",
                "mcp__team_tools__query_db",
                r#"{"query":"ORBIT-9"}"#,
            )],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(10),
            first_execution_request_id: ExecutionRequestId(30),
            created_at_ms: 4,
        },
    )
    .unwrap();

    assert_eq!(
        report.entries[0].execution_permission,
        ToolExecutionPermission::Allowed
    );
}

#[test]
fn provider_tool_call_batch_rejects_zero_ids_or_time() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

    let invalid_ids = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![provider_call(
                "call-search",
                "builtin_search",
                r#"{"query":"hello","intent":"检索 hello"}"#,
            )],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(0),
            first_execution_request_id: ExecutionRequestId(0),
            created_at_ms: 4,
        },
    );
    let invalid_time = ProviderToolCallScheduleRuntime::schedule(
        &registry,
        ProviderToolCallScheduleInput {
            tool_calls: vec![provider_call(
                "call-search",
                "builtin_search",
                r#"{"query":"hello","intent":"检索 hello"}"#,
            )],
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            cell_id: CellId(2),
            first_tool_call_id: ToolCallId(1),
            first_execution_request_id: ExecutionRequestId(1),
            created_at_ms: 0,
        },
    );

    assert!(invalid_ids.is_err());
    assert!(invalid_time.is_err());
}

fn provider_call(
    id: impl Into<String>,
    name: impl Into<String>,
    input: impl Into<axon_domain::ToolCallInput>,
) -> ProviderToolCall {
    ProviderToolCall::new(id, name, input)
}
