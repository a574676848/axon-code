#[path = "orchestrator_builtin.rs"]
mod orchestrator_builtin;
#[path = "orchestrator_models.rs"]
mod orchestrator_models;
#[path = "orchestrator_notes_history.rs"]
mod orchestrator_notes_history;
#[path = "orchestrator_receipt.rs"]
mod orchestrator_receipt;
#[path = "orchestrator_subagent.rs"]
mod orchestrator_subagent;
#[path = "orchestrator_subagent_job.rs"]
mod orchestrator_subagent_job;
#[path = "orchestrator_time.rs"]
mod orchestrator_time;

use axon_agent_runtime::{
    ASK_USER_QUESTION_TOOL_QUALIFIED_NAME, GET_CONTEXT_REMAINING_TOOL_QUALIFIED_NAME,
    GoalToolRuntime, HISTORY_LIST_ITEMS_TOOL_NAME, HISTORY_LIST_ITEMS_TOOL_QUALIFIED_NAME,
    HISTORY_LIST_WINDOWS_TOOL_NAME, HISTORY_LIST_WINDOWS_TOOL_QUALIFIED_NAME,
    HISTORY_READ_ITEM_TOOL_NAME, HISTORY_READ_ITEM_TOOL_QUALIFIED_NAME,
    HISTORY_SEARCH_CONTENTS_TOOL_NAME, HISTORY_SEARCH_CONTENTS_TOOL_QUALIFIED_NAME,
    NEW_CONTEXT_TOOL_QUALIFIED_NAME, NOTES_APPEND_TO_FILE_TOOL_NAME,
    NOTES_APPEND_TO_FILE_TOOL_QUALIFIED_NAME, NOTES_LIST_FILES_BY_PREFIX_TOOL_NAME,
    NOTES_LIST_FILES_BY_PREFIX_TOOL_QUALIFIED_NAME, NOTES_READ_FILE_TOOL_NAME,
    NOTES_READ_FILE_TOOL_QUALIFIED_NAME, NOTES_SEARCH_CONTENTS_TOOL_NAME,
    NOTES_SEARCH_CONTENTS_TOOL_QUALIFIED_NAME, NOTES_WRITE_FILE_TOOL_NAME,
    NOTES_WRITE_FILE_TOOL_QUALIFIED_NAME, ProviderToolCall, ProviderToolCallScheduleInput,
    ProviderToolCallScheduleRuntime, SubagentToolRuntime, ToolLedgerDecision, ToolRegistry,
    UPDATE_PLAN_TOOL_QUALIFIED_NAME, is_mcp_tool_name,
};
use axon_domain::{CellId, ExecutionRequestId, SessionId, ToolCallId, ToolExecutionPermission};

pub(crate) use orchestrator_models::{
    ToolCancelToken, ToolExecutionRequest, ToolExecutionSchedule, ToolKind,
    ToolOrchestratorContext, ToolOutputChunk, ToolOutputStream, ToolProgressInfo, ToolReceipt,
    ToolRuntimeEvent, ToolRuntimeObserver,
};
#[cfg(test)]
pub(crate) use orchestrator_receipt::record_scheduled_execution_receipts;
pub(crate) use orchestrator_receipt::{
    blocked_tool_receipt, completed_tool_receipt, failed_tool_input_receipt, failed_tool_receipt,
    failed_tool_receipt_from_axon_error, failed_tool_receipt_with_error_type,
    tool_receipt_from_runtime,
};
pub(crate) use orchestrator_time::now_ms;

use orchestrator_builtin::{
    TokenBudgetMeasurementSource, TokenBudgetSnapshot, clear_token_budget_snapshot,
    execute_get_context_remaining, execute_memory_search, execute_memory_write,
    execute_new_context, execute_search, execute_update_plan, record_token_budget_snapshot,
};
use orchestrator_notes_history::{execute_history_tool, execute_notes_tool};
use orchestrator_subagent::execute_subagent_tool;
pub(crate) use orchestrator_subagent::spawn_intent_fingerprint;

pub(crate) fn classify_tool(name: &str) -> ToolKind {
    match name {
        "builtin.search" => ToolKind::Search,
        "builtin.read_file" => ToolKind::ReadFile,
        "builtin.glob" => ToolKind::Glob,
        "builtin.memory_search" => ToolKind::MemorySearch,
        "builtin.memory_write" => ToolKind::MemoryWrite,
        _ if is_history_tool_name(name) => ToolKind::History,
        _ if is_notes_tool_name(name) => ToolKind::Notes,
        GET_CONTEXT_REMAINING_TOOL_QUALIFIED_NAME => ToolKind::GetContextRemaining,
        NEW_CONTEXT_TOOL_QUALIFIED_NAME => ToolKind::NewContext,
        "builtin.list_mcp_resources" => ToolKind::ListMcpResources,
        "builtin.read_mcp_resource" => ToolKind::ReadMcpResource,
        UPDATE_PLAN_TOOL_QUALIFIED_NAME => ToolKind::UpdatePlan,
        ASK_USER_QUESTION_TOOL_QUALIFIED_NAME | "ask_user_question" => ToolKind::AskUserQuestion,
        _ if GoalToolRuntime::is_goal_tool(name) => ToolKind::GoalTool,
        _ if SubagentToolRuntime::is_subagent_tool(name) => ToolKind::Subagent,
        "builtin.invoke_skill" => ToolKind::InvokeSkill,
        "builtin.shell_command" | "builtin.write_stdin" => ToolKind::Shell,
        "builtin.job_output" | "builtin.job_kill" => ToolKind::Job,
        "builtin.edit" => ToolKind::Edit,
        _ if is_mcp_tool_name(name) => ToolKind::Mcp,
        _ => ToolKind::Unsupported,
    }
}

/// 判定 history 命名空间工具（接受限定名或裸名）。
#[must_use]
pub(crate) fn is_history_tool_name(name: &str) -> bool {
    [
        (
            HISTORY_LIST_WINDOWS_TOOL_QUALIFIED_NAME,
            HISTORY_LIST_WINDOWS_TOOL_NAME,
        ),
        (
            HISTORY_LIST_ITEMS_TOOL_QUALIFIED_NAME,
            HISTORY_LIST_ITEMS_TOOL_NAME,
        ),
        (
            HISTORY_READ_ITEM_TOOL_QUALIFIED_NAME,
            HISTORY_READ_ITEM_TOOL_NAME,
        ),
        (
            HISTORY_SEARCH_CONTENTS_TOOL_QUALIFIED_NAME,
            HISTORY_SEARCH_CONTENTS_TOOL_NAME,
        ),
    ]
    .iter()
    .any(|(qualified, bare)| name == *qualified || name == *bare)
}

/// 判定 notes 命名空间工具（接受限定名或裸名）。
#[must_use]
pub(crate) fn is_notes_tool_name(name: &str) -> bool {
    [
        (
            NOTES_WRITE_FILE_TOOL_QUALIFIED_NAME,
            NOTES_WRITE_FILE_TOOL_NAME,
        ),
        (
            NOTES_APPEND_TO_FILE_TOOL_QUALIFIED_NAME,
            NOTES_APPEND_TO_FILE_TOOL_NAME,
        ),
        (
            NOTES_READ_FILE_TOOL_QUALIFIED_NAME,
            NOTES_READ_FILE_TOOL_NAME,
        ),
        (
            NOTES_LIST_FILES_BY_PREFIX_TOOL_QUALIFIED_NAME,
            NOTES_LIST_FILES_BY_PREFIX_TOOL_NAME,
        ),
        (
            NOTES_SEARCH_CONTENTS_TOOL_QUALIFIED_NAME,
            NOTES_SEARCH_CONTENTS_TOOL_NAME,
        ),
    ]
    .iter()
    .any(|(qualified, bare)| name == *qualified || name == *bare)
}

pub(crate) fn record_provider_token_budget_snapshot(
    session_id: &str,
    owner_id: &str,
    rollover_limit_tokens: u64,
    provider_context_tokens: Option<u64>,
    local_estimated_context_tokens: u64,
) {
    let (current_context_tokens, source) =
        provider_context_tokens.filter(|tokens| *tokens > 0).map_or(
            (
                local_estimated_context_tokens,
                TokenBudgetMeasurementSource::LocalEstimate,
            ),
            |tokens| (tokens, TokenBudgetMeasurementSource::ProviderUsage),
        );
    record_token_budget_snapshot(
        session_id,
        TokenBudgetSnapshot {
            owner_id: owner_id.to_owned(),
            rollover_limit_tokens,
            current_context_tokens,
            source,
        },
    );
}

pub(crate) struct ProviderTokenBudgetSnapshotScope {
    session_id: String,
    owner_id: String,
}

impl ProviderTokenBudgetSnapshotScope {
    pub(crate) fn new(session_id: &str, owner_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            owner_id: owner_id.to_owned(),
        }
    }
}

impl Drop for ProviderTokenBudgetSnapshotScope {
    fn drop(&mut self) {
        clear_token_budget_snapshot(&self.session_id, &self.owner_id);
    }
}

pub(crate) fn schedule_tool_calls(
    registry: &ToolRegistry,
    tool_calls: &[ProviderToolCall],
    provider_turn_id: u64,
    cwd: &std::path::Path,
    session_id: SessionId,
    cell_id: CellId,
    created_at_ms: u64,
) -> Result<Vec<ToolExecutionRequest>, String> {
    Ok(schedule_tool_execution(
        registry,
        tool_calls,
        provider_turn_id,
        cwd,
        session_id,
        cell_id,
        created_at_ms,
    )?
    .requests)
}

pub(crate) fn schedule_tool_execution(
    registry: &ToolRegistry,
    tool_calls: &[ProviderToolCall],
    provider_turn_id: u64,
    cwd: &std::path::Path,
    session_id: SessionId,
    cell_id: CellId,
    created_at_ms: u64,
) -> Result<ToolExecutionSchedule, String> {
    let report = ProviderToolCallScheduleRuntime::schedule(
        registry,
        ProviderToolCallScheduleInput {
            tool_calls: tool_calls.to_vec(),
            cwd: cwd.to_path_buf(),
            session_id,
            cell_id,
            first_tool_call_id: ToolCallId(1),
            first_execution_request_id: ExecutionRequestId(1),
            created_at_ms,
        },
    )
    .map_err(|error| format!("provider tool-call schedule 失败：{error:?}"))?;

    let requests = report
        .entries
        .iter()
        .map(|entry| {
            execution_request_from_permission(
                entry.tool_call.clone(),
                entry.execution_permission.clone(),
                entry.concurrency_policy,
                entry.tool_call_id,
                entry.execution_request_id,
                tool_execution_part_id(provider_turn_id, &entry.tool_call.id, entry.dispatch_order),
            )
        })
        .collect();
    Ok(ToolExecutionSchedule {
        #[cfg(test)]
        report,
        requests,
    })
}

pub(crate) fn failed_tool_schedule_receipts(
    tool_calls: &[ProviderToolCall],
    provider_turn_id: u64,
    error: &str,
) -> Vec<ToolReceipt> {
    tool_calls
        .iter()
        .enumerate()
        .map(|(index, tool_call)| {
            let sequence_id = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
            let mut receipt =
                failed_tool_receipt(tool_call, format!("工具调度失败，已拒绝执行：{error}"));
            receipt.tool_call_id = sequence_id;
            receipt.part_id = tool_execution_part_id(provider_turn_id, &tool_call.id, index);
            receipt
        })
        .collect()
}

fn execution_request_from_permission(
    tool_call: ProviderToolCall,
    execution_permission: ToolExecutionPermission,
    concurrency_policy: axon_domain::ToolConcurrencyPolicy,
    tool_call_id: ToolCallId,
    execution_request_id: ExecutionRequestId,
    part_id: String,
) -> ToolExecutionRequest {
    let kind = classify_tool(tool_call.dispatch_name());
    ToolExecutionRequest {
        tool_call,
        tool_call_id,
        execution_request_id,
        part_id,
        kind,
        execution_permission,
        concurrency_policy,
    }
}

const TOOL_EXECUTION_PART_PREFIX: &str = "tool-part:stream";
pub(crate) const NON_PROVIDER_TURN_ID: u64 = 0;

pub(crate) fn tool_execution_part_id(
    provider_turn_id: u64,
    provider_call_id: &str,
    dispatch_order: usize,
) -> String {
    format!("{TOOL_EXECUTION_PART_PREFIX}:{provider_turn_id}:{dispatch_order}:{provider_call_id}")
}

pub(crate) fn execute_tool_call_with_observer(
    context: &ToolOrchestratorContext,
    request: &ToolExecutionRequest,
    runtime_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    let tool_call = &request.tool_call;
    let workspace_revision = crate::run_commands::workspace_revision(&context.cwd);
    let turn_id = crate::turn_execution_ledger::turn_id(context);
    let scope_id = crate::turn_execution_ledger::scope_id(context);
    match crate::turn_execution_ledger::begin(&scope_id, request, workspace_revision, &turn_id) {
        ToolLedgerDecision::Reuse(receipt) => return receipt,
        ToolLedgerDecision::RejectUnchanged(message) => {
            return failed_tool_receipt(tool_call, message);
        }
        ToolLedgerDecision::Execute => {}
        ToolLedgerDecision::InFlight => unreachable!("begin 已等待 in-flight 完成"),
    }
    match &request.execution_permission {
        ToolExecutionPermission::Allowed => {}
        ToolExecutionPermission::Forbidden { reason } => {
            let receipt = blocked_tool_receipt(tool_call, reason);
            crate::turn_execution_ledger::settle(&scope_id, request, receipt.clone(), &turn_id);
            return receipt;
        }
    }

    let receipt = match request.kind {
        ToolKind::Search => execute_search(tool_call, context),
        ToolKind::ReadFile => crate::tool_orchestrator_fs::execute_read_file(tool_call, context),
        ToolKind::Glob => crate::tool_orchestrator_fs::execute_glob(tool_call, context),
        ToolKind::MemorySearch => execute_memory_search(tool_call, context),
        ToolKind::MemoryWrite => execute_memory_write(tool_call, context),
        ToolKind::History => execute_history_tool(tool_call, context),
        ToolKind::Notes => execute_notes_tool(tool_call, context),
        ToolKind::GetContextRemaining => execute_get_context_remaining(tool_call, context),
        ToolKind::NewContext => execute_new_context(tool_call, context),
        ToolKind::ListMcpResources => {
            crate::tool_orchestrator_runtime_capabilities::execute_list_mcp_resources(
                tool_call, context,
            )
        }
        ToolKind::ReadMcpResource => {
            crate::tool_orchestrator_runtime_capabilities::execute_read_mcp_resource(
                tool_call, context,
            )
        }
        ToolKind::UpdatePlan => execute_update_plan(tool_call),
        ToolKind::GoalTool => crate::tool_orchestrator_goal::execute_goal_tool(tool_call, context),
        ToolKind::Subagent => execute_subagent_tool(tool_call, context),
        ToolKind::Mcp => crate::tool_orchestrator_runtime_capabilities::execute_mcp(
            request,
            context,
            runtime_observer,
        ),
        ToolKind::InvokeSkill => {
            crate::tool_orchestrator_runtime_capabilities::execute_builtin_invoke_skill(
                tool_call, context,
            )
        }
        ToolKind::Shell => {
            crate::tool_orchestrator_shell::execute_shell(request, context, runtime_observer)
        }
        ToolKind::Job => crate::tool_orchestrator_job::execute_job(tool_call, context),
        ToolKind::Edit => {
            crate::tool_orchestrator_hashline::execute_edit(request, context, runtime_observer)
        }
        ToolKind::AskUserQuestion => execute_ask_user_question(tool_call, context),
        ToolKind::Unsupported => blocked_tool_receipt(
            tool_call,
            format!("当前主链尚未接入工具 {}", tool_call.dispatch_name()),
        ),
    };
    crate::turn_execution_ledger::settle(&scope_id, request, receipt.clone(), &turn_id);
    receipt
}

fn execute_ask_user_question(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let input_value = match &tool_call.input {
        axon_domain::ToolCallInput::Json(val) => val.clone(),
        axon_domain::ToolCallInput::Text(text) => {
            match serde_json::from_str::<serde_json::Value>(text) {
                Ok(val) => val,
                Err(e) => {
                    return failed_tool_receipt(tool_call, format!("工具输入不是合法 JSON: {e}"));
                }
            }
        }
    };

    let input = match axon_agent_runtime::parse_and_validate_ask_user_question_input(&input_value) {
        Ok(input) => input,
        Err(error) => return failed_tool_receipt(tool_call, format!("{error:?}")),
    };

    let Some(port) = &context.user_question_port else {
        return failed_tool_receipt(tool_call, "当前运行时环境未装配用户提问端口");
    };

    let turn_id = crate::turn_execution_ledger::turn_id(context);
    let questions_json = match serde_json::to_string(&input.questions) {
        Ok(json) => json,
        Err(error) => return failed_tool_receipt(tool_call, format!("问题序列化失败: {error}")),
    };

    let now = now_ms() as i64;
    let record = axon_domain::UserQuestionRecord {
        question_id: format!("uq-{}", uuid::Uuid::new_v4()),
        idempotency_key: format!("idem-{}-{}", context.session_id.0, tool_call.id),
        // 车道身份取自本轮 Interaction 的路由上下文：答案消息必须能按它回到同一条车道，
        // 客户端的车道对账查询也按它归属。
        lane_id: crate::run_commands::run_lane_id(
            context.goal_transport_context.as_ref(),
            &context.thread_session_id,
        ),
        session_id: context.session_id.0.to_string(),
        interaction_id: context.cell_id.0.to_string(),
        turn_id,
        tool_call_id: tool_call.id.clone(),
        questions_json,
        handoff_text: input.handoff.clone(),
        state: axon_domain::UserQuestionState::Pending,
        deadline_at_ms: now + axon_agent_runtime::DEFAULT_WAIT_WINDOW_MS as i64,
        answer_json: None,
        answered_at_ms: None,
        answered_channel: None,
        delivered: false,
        delivery_mode: None,
        created_at_ms: now,
        expires_at_ms: now + axon_agent_runtime::DEFAULT_EXPIRES_AFTER_MS,
    };

    let cancel_token = context.run_cancel_token.clone().unwrap_or_default();
    let budget = axon_agent_runtime::QuestionWaitBudget::default();

    match port.wait_for_answer(&record, budget, &cancel_token) {
        Ok(axon_agent_runtime::QuestionWaitResult::Answered { answer_json, .. }) => {
            match ask_user_question_receipt(tool_call, "answered", answer_json, None, None, None) {
                Ok(summary) => completed_tool_receipt(tool_call, summary, None, false),
                Err(error) => failed_tool_receipt(tool_call, error),
            }
        }
        Ok(axon_agent_runtime::QuestionWaitResult::Suspended { handoff_text }) => {
            let receipt = ask_user_question_receipt(
                tool_call,
                "suspended",
                String::new(),
                Some(true),
                Some(true),
                Some(handoff_text),
            );
            match receipt {
                Ok(summary) => completed_tool_receipt(tool_call, summary, None, false),
                Err(error) => failed_tool_receipt(tool_call, error),
            }
        }
        Ok(axon_agent_runtime::QuestionWaitResult::Aborted { reason }) => {
            failed_tool_receipt(tool_call, format!("提问等待已中止: {reason}"))
        }
        Err(error) => failed_tool_receipt(tool_call, format!("提问执行失败: {error}")),
    }
}

/// 构造 ask_user_question 的结构化回执 JSON（合同形状的唯一构造点）。
///
/// `answered` 分支必须带上解析后的答案列表：模型据此判断用户到底选了什么，
/// 而不是从一段自由文本里猜。答案载荷不可解析时如实失败，不静默降级成空答案。
fn ask_user_question_receipt(
    tool_call: &ProviderToolCall,
    status: &str,
    answer_json: String,
    pending: Option<bool>,
    suspended: Option<bool>,
    notice: Option<String>,
) -> Result<String, String> {
    let answers = if status == "answered" {
        let parsed: Vec<axon_domain::UserAnswerItem> = serde_json::from_str(&answer_json)
            .map_err(|error| format!("提问回执答案不可解析: {error}"))?;
        Some(parsed)
    } else {
        None
    };
    serde_json::to_string(&axon_agent_runtime::AskUserQuestionToolReceipt {
        schema_version: axon_agent_runtime::ASK_USER_QUESTION_TOOL_SCHEMA_VERSION,
        kind: axon_agent_runtime::ASK_USER_QUESTION_RECEIPT_KIND.to_string(),
        status: status.to_string(),
        pending,
        suspended,
        call_id: Some(tool_call.id.clone()),
        notice,
        answers,
    })
    .map_err(|error| format!("提问回执序列化失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    const TEST_PROVIDER_TURN_ID: u64 = 7;

    #[test]
    fn execution_receipt_recording_reuses_schedule_report() {
        let registry = ToolRegistry::new();
        let scheduled = schedule_tool_execution(
            &registry,
            &[ProviderToolCall::new("call-1", "builtin.search", "{}")],
            TEST_PROVIDER_TURN_ID,
            Path::new("."),
            SessionId(1),
            CellId(1),
            1,
        )
        .unwrap();
        assert_eq!(scheduled.requests[0].part_id, "tool-part:stream:7:0:call-1");
        let receipts = vec![completed_tool_receipt(
            &scheduled.requests[0].tool_call,
            "search ok",
            None,
            false,
        )];

        let report = record_scheduled_execution_receipts(&scheduled.report, &receipts, 2).unwrap();

        assert_eq!(report.receipts[0].provider_call_id, "call-1");
        assert_eq!(report.receipts[0].tool_call_id, ToolCallId(1));
        assert_eq!(report.receipts[0].completed_at_ms, 2);
    }

    #[test]
    fn repeated_provider_call_id_keeps_distinct_execution_parts() {
        let registry = ToolRegistry::new();
        let scheduled = schedule_tool_execution(
            &registry,
            &[
                ProviderToolCall::new("shared-call", "builtin.search", r#"{"query":"a"}"#),
                ProviderToolCall::new(
                    "shared-call",
                    "builtin.read_file",
                    r#"{"path":"Cargo.toml"}"#,
                ),
            ],
            TEST_PROVIDER_TURN_ID,
            Path::new("."),
            SessionId(1),
            CellId(1),
            1,
        )
        .unwrap();

        assert_eq!(scheduled.requests.len(), 2);
        assert_eq!(
            scheduled.requests[0].part_id,
            "tool-part:stream:7:0:shared-call"
        );
        assert_eq!(
            scheduled.requests[1].part_id,
            "tool-part:stream:7:1:shared-call"
        );
        assert_ne!(scheduled.requests[0].part_id, scheduled.requests[1].part_id);
    }

    #[test]
    fn ask_user_question_classification_and_validation() {
        assert_eq!(
            classify_tool("builtin.ask_user_question"),
            ToolKind::AskUserQuestion
        );
        assert_eq!(
            classify_tool("ask_user_question"),
            ToolKind::AskUserQuestion
        );

        let invalid_call = ProviderToolCall::new("call-inv", "builtin.ask_user_question", "{}");
        let context = ToolOrchestratorContext {
            config_path: None,
            cwd: std::path::PathBuf::from("."),
            session_id: SessionId(1),
            thread_session_id: "thread-1".to_string(),
            cell_id: CellId(1),
            model_messages: Vec::new(),
            execution_tools: Arc::new(std::sync::RwLock::new(ToolRegistry::new())),
            memory_write_runtime: Arc::new(crate::run_commands::AgentMemoryWriteRuntime::default()),
            workspace_files: Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
            timeout: std::time::Duration::from_secs(10),
            goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
            goal_transport_context: None,
            cancel_token: None,
            run_cancel_token: None,
            model_profile_id: axon_domain::ModelProfileId(0),
            permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
            current_agent_path: axon_agent_runtime::AgentPath::root(),
            agent_control: Arc::new(std::sync::Mutex::new(
                axon_agent_runtime::AgentControlRuntime::default(),
            )),
            agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
            subagent_sessions: Arc::new(std::sync::Mutex::new(
                axon_agent_runtime::SubagentSessionRuntime::default(),
            )),
            subagent_executor: Arc::new(
                crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
            ),
            subagent_cancel_tokens: Arc::new(std::sync::Mutex::new(HashMap::new())),
            user_question_port: None,
        };

        let receipt = execute_ask_user_question(&invalid_call, &context);
        assert_eq!(receipt.status, "failed");
        assert!(
            receipt.summary.contains("missing field")
                || receipt.summary.contains("提问参数无法解析")
        );

        let empty_call = ProviderToolCall::new(
            "call-empty",
            "builtin.ask_user_question",
            r#"{"summary":"确认部署分支","questions": []}"#,
        );
        let receipt_empty = execute_ask_user_question(&empty_call, &context);
        assert_eq!(receipt_empty.status, "failed");
        // 面向模型的是可读因果，不是内部错误码：码只用于日志与稳定判定。
        assert!(receipt_empty.summary.contains("至少要提出一个问题"));

        // 摘要缺失：schema 必填项缺失必须在解析阶段被拦下，而不是放到等待之后
        let missing_summary = ProviderToolCall::new(
            "call-no-summary",
            "builtin.ask_user_question",
            r#"{"questions":[{"id":"q1","question":"继续吗？"}]}"#,
        );
        let receipt_missing = execute_ask_user_question(&missing_summary, &context);
        assert_eq!(receipt_missing.status, "failed");
        assert!(
            receipt_missing.summary.contains("summary")
                || receipt_missing.summary.contains("提问参数无法解析")
        );
    }
}
