//! Provider tool-call 本机调度计划。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use axon_domain::{
    AxonError, AxonResult, CellId, ExecutionRequestId, OutputRef, SessionId, ToolCallId,
    ToolConcurrencyPolicy, ToolExecutionPermission, ToolInputFormat, ToolPermissionPolicy,
};

use crate::{
    JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME, ProviderToolCall, SHELL_COMMAND_TOOL_NAME,
    ShellCommandPolicyEvaluator, ToolRegistry, WRITE_STDIN_TOOL_NAME,
    is_recoverable_tool_call_input, resolve_provider_tool_name,
};

/// provider tool-call 批量调度完成的审计原因。
pub const PROVIDER_TOOL_CALL_REASON_BATCH_SCHEDULED: &str = "provider_tool_call:batch_scheduled";
/// provider tool-call 执行回执已汇聚的审计原因。
pub const PROVIDER_TOOL_CALL_REASON_EXECUTION_RECEIPTS_RECORDED: &str =
    "provider_tool_call:execution_receipts_recorded";

/// provider tool-call 批量调度输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolCallScheduleInput {
    /// provider 归一化后的工具调用，顺序由 provider stream index 决定。
    pub tool_calls: Vec<ProviderToolCall>,
    /// 当前工具工作目录。
    pub cwd: PathBuf,
    /// 所属 session。
    pub session_id: SessionId,
    /// 所属 RuntimeCell。
    pub cell_id: CellId,
    /// 首个工具调用运行 ID。
    pub first_tool_call_id: ToolCallId,
    /// 首个审批请求 ID。
    pub first_execution_request_id: ExecutionRequestId,
    /// 当前时间。
    pub created_at_ms: u64,
}

/// provider tool-call 批量调度项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolCallScheduleEntry {
    /// 调度顺序。
    pub dispatch_order: usize,
    /// provider tool-call。
    pub tool_call: ProviderToolCall,
    /// 本机运行时工具调用 ID。
    pub tool_call_id: ToolCallId,
    /// 审批请求 ID。
    pub execution_request_id: ExecutionRequestId,
    /// 本次调用的最终执行权限。
    pub execution_permission: ToolExecutionPermission,
    /// 工具定义声明的并发执行策略。
    pub concurrency_policy: ToolConcurrencyPolicy,
}

/// provider tool-call 批量调度报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolCallScheduleReport {
    /// 已调度的工具调用。
    pub entries: Vec<ProviderToolCallScheduleEntry>,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// provider tool-call 执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProviderToolCallExecutionStatus {
    /// 工具调用已完成。
    Completed,
    /// 工具调用失败。
    Failed,
}

/// provider tool-call 执行 outcome。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolCallExecutionOutcome {
    /// 本机运行时工具调用 ID。
    pub tool_call_id: ToolCallId,
    /// 执行状态。
    pub status: ProviderToolCallExecutionStatus,
    /// 有界摘要；不得包含 provider raw payload。
    pub bounded_summary: String,
    /// 结构化错误类型。
    pub error_type: Option<String>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否发生文件写入。
    pub filesystem_write_performed: bool,
    /// 完成时间。
    pub completed_at_ms: u64,
    /// 完整输出引用。
    pub output_ref: Option<OutputRef>,
}

/// provider tool-call 执行回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolCallExecutionReceipt {
    /// 调度顺序。
    pub dispatch_order: usize,
    /// 本机运行时工具调用 ID。
    pub tool_call_id: ToolCallId,
    /// provider call ID。
    pub provider_call_id: String,
    /// provider tool name。
    pub tool_name: String,
    /// 本机审批请求 ID。
    pub execution_request_id: ExecutionRequestId,
    /// 执行状态。
    pub status: ProviderToolCallExecutionStatus,
    /// 有界摘要。
    pub bounded_summary: String,
    /// 结构化错误类型。
    pub error_type: Option<String>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否发生文件写入。
    pub filesystem_write_performed: bool,
    /// 完成时间。
    pub completed_at_ms: u64,
    /// 完整输出引用。
    pub output_ref: Option<OutputRef>,
}

/// provider tool-call 执行统计。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProviderToolCallExecutionStats {
    /// 执行总数。
    pub total_count: usize,
    /// 按工具名聚合的调用数。
    pub by_tool_name: BTreeMap<String, usize>,
    /// 按错误类型聚合的失败数。
    pub failures_by_error_type: BTreeMap<String, usize>,
}

impl Serialize for ProviderToolCallExecutionReceipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ProviderToolCallExecutionReceipt", 12)?;
        state.serialize_field("dispatchOrder", &self.dispatch_order)?;
        state.serialize_field("toolCallId", &self.tool_call_id.0)?;
        state.serialize_field("providerCallId", &self.provider_call_id)?;
        state.serialize_field("toolName", &self.tool_name)?;
        state.serialize_field("executionRequestId", &self.execution_request_id.0)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("boundedSummary", &self.bounded_summary)?;
        state.serialize_field("outputRef", &self.output_ref)?;
        state.serialize_field("errorType", &self.error_type)?;
        state.serialize_field("remoteTriggerAllowed", &self.remote_trigger_allowed)?;
        state.serialize_field("filesystemWritePerformed", &self.filesystem_write_performed)?;
        state.serialize_field("completedAtMs", &self.completed_at_ms)?;
        state.end()
    }
}

/// provider tool-call 执行回执报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderToolCallExecutionReport {
    /// 已汇聚的执行回执。
    pub receipts: Vec<ProviderToolCallExecutionReceipt>,
    /// 完成数量。
    pub completed_count: usize,
    /// 失败数量。
    pub failed_count: usize,
    /// 是否全部完成。
    pub all_completed: bool,
    /// 工具调用统计。
    pub stats: ProviderToolCallExecutionStats,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Provider tool-call 调度 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderToolCallScheduleRuntime;

impl ProviderToolCallScheduleRuntime {
    /// 批量调度 provider tool-call；只生成稳定编号与执行边界，不直接执行副作用。
    pub fn schedule(
        registry: &ToolRegistry,
        input: ProviderToolCallScheduleInput,
    ) -> AxonResult<ProviderToolCallScheduleReport> {
        validate_schedule(&input)?;
        let mut entries = Vec::with_capacity(input.tool_calls.len());
        for (dispatch_order, mut tool_call) in input.tool_calls.into_iter().enumerate() {
            let execution_request_id =
                offset_execution_request_id(input.first_execution_request_id, dispatch_order)?;
            let tool_call_id = offset_tool_call_id(input.first_tool_call_id, dispatch_order)?;
            tool_call.name = resolve_provider_tool_name(registry, &tool_call.name).canonical_name;
            let execution_permission = execution_permission_for_call(
                registry,
                tool_call.dispatch_name(),
                &tool_call.input,
                &input.cwd,
            );
            let concurrency_policy = registry
                .tool_by_qualified_name(tool_call.dispatch_name())
                .map_or(ToolConcurrencyPolicy::Exclusive, |tool| {
                    tool.concurrency_policy
                });
            entries.push(ProviderToolCallScheduleEntry {
                dispatch_order,
                tool_call,
                tool_call_id,
                execution_request_id,
                execution_permission,
                concurrency_policy,
            });
        }

        Ok(ProviderToolCallScheduleReport {
            entries,
            reasons: vec![PROVIDER_TOOL_CALL_REASON_BATCH_SCHEDULED.to_owned()],
        })
    }

    /// 汇聚 provider tool-call 执行结果；只记录 bounded receipt，不执行工具副作用。
    pub fn record_execution_receipts(
        schedule: &ProviderToolCallScheduleReport,
        outcomes: Vec<ProviderToolCallExecutionOutcome>,
    ) -> AxonResult<ProviderToolCallExecutionReport> {
        validate_execution_inputs(schedule, &outcomes)?;
        let outcomes_by_id = outcomes
            .into_iter()
            .map(|outcome| (outcome.tool_call_id, outcome))
            .collect::<BTreeMap<_, _>>();
        let mut receipts = Vec::with_capacity(schedule.entries.len());
        let mut completed_count = 0;
        let mut failed_count = 0;
        let mut stats = ProviderToolCallExecutionStats {
            total_count: schedule.entries.len(),
            ..ProviderToolCallExecutionStats::default()
        };

        for entry in &schedule.entries {
            let outcome = outcomes_by_id.get(&entry.tool_call_id).ok_or(
                AxonError::tool_invalid_definition("provider tool-call 执行回执缺少调度项"),
            )?;
            match outcome.status {
                ProviderToolCallExecutionStatus::Completed => completed_count += 1,
                ProviderToolCallExecutionStatus::Failed => {
                    failed_count += 1;
                    let error_type = outcome
                        .error_type
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned());
                    *stats.failures_by_error_type.entry(error_type).or_default() += 1;
                }
            }
            *stats
                .by_tool_name
                .entry(entry.tool_call.name.clone())
                .or_default() += 1;
            receipts.push(ProviderToolCallExecutionReceipt {
                dispatch_order: entry.dispatch_order,
                tool_call_id: entry.tool_call_id,
                provider_call_id: entry.tool_call.id.clone(),
                tool_name: entry.tool_call.name.clone(),
                execution_request_id: entry.execution_request_id,
                status: outcome.status,
                bounded_summary: outcome.bounded_summary.clone(),
                output_ref: outcome.output_ref.clone(),
                error_type: outcome.error_type.clone(),
                remote_trigger_allowed: outcome.remote_trigger_allowed,
                filesystem_write_performed: outcome.filesystem_write_performed,
                completed_at_ms: outcome.completed_at_ms,
            });
        }

        Ok(ProviderToolCallExecutionReport {
            receipts,
            completed_count,
            failed_count,
            all_completed: failed_count == 0,
            stats,
            reasons: vec![PROVIDER_TOOL_CALL_REASON_EXECUTION_RECEIPTS_RECORDED.to_owned()],
        })
    }
}

fn validate_execution_inputs(
    schedule: &ProviderToolCallScheduleReport,
    outcomes: &[ProviderToolCallExecutionOutcome],
) -> AxonResult<()> {
    if schedule.entries.is_empty() || outcomes.len() != schedule.entries.len() {
        return Err(AxonError::tool_invalid_definition(
            "provider tool-call 执行回执数量与调度项不匹配",
        ));
    }
    let scheduled_ids = schedule
        .entries
        .iter()
        .map(|entry| entry.tool_call_id)
        .collect::<BTreeSet<_>>();
    let mut outcome_ids = BTreeSet::new();
    for outcome in outcomes {
        if outcome.tool_call_id.0 == 0 || outcome.completed_at_ms == 0 {
            return Err(AxonError::tool_invalid_definition(
                "provider tool-call 执行回执 ID 或时间无效",
            ));
        }
        if !outcome_ids.insert(outcome.tool_call_id) {
            return Err(AxonError::tool_invalid_definition(
                "provider tool-call 执行回执 tool_call_id 重复",
            ));
        }
        if !scheduled_ids.contains(&outcome.tool_call_id) {
            return Err(AxonError::tool_invalid_definition(
                "provider tool-call 执行回执包含未知 tool_call_id",
            ));
        }
        if outcome.bounded_summary.trim().is_empty() {
            return Err(AxonError::tool_invalid_definition(
                "provider tool-call 执行回执摘要不能为空",
            ));
        }
    }
    Ok(())
}

fn execution_permission_for_call(
    registry: &ToolRegistry,
    qualified_name: &str,
    input: &axon_domain::ToolCallInput,
    cwd: &std::path::Path,
) -> ToolExecutionPermission {
    if is_recoverable_tool_call_input(input) {
        return ToolExecutionPermission::Forbidden {
            reason: recoverable_arguments_error(registry, qualified_name),
        };
    }
    let Some(tool) = registry.tool_by_qualified_name(qualified_name) else {
        return ToolExecutionPermission::Forbidden {
            reason: format!("工具未注册，禁止执行：{qualified_name}"),
        };
    };
    if let Err(error) = registry.validate_call_input(qualified_name, input) {
        return ToolExecutionPermission::Forbidden {
            reason: format!("invalid_arguments: {error}"),
        };
    }
    match tool.permission_policy {
        ToolPermissionPolicy::ReadOnly | ToolPermissionPolicy::GovernedWrite => {
            ToolExecutionPermission::Allowed
        }
        ToolPermissionPolicy::CallEvaluated if tool.name == SHELL_COMMAND_TOOL_NAME => {
            ShellCommandPolicyEvaluator::evaluate(input, cwd).execution_permission
        }
        ToolPermissionPolicy::CallEvaluated if tool.name == WRITE_STDIN_TOOL_NAME => {
            ToolExecutionPermission::Allowed
        }
        // 作业工具只作用于本 Agent run 自己启动的作业：owner 隔离由作业注册表保证。
        // 读取与终止都不接触工作区，因此与 write_stdin 的自建会话处理保持一致。
        ToolPermissionPolicy::CallEvaluated
            if tool.name == JOB_OUTPUT_TOOL_NAME || tool.name == JOB_KILL_TOOL_NAME =>
        {
            ToolExecutionPermission::Allowed
        }
        ToolPermissionPolicy::CallEvaluated => ToolExecutionPermission::Forbidden {
            reason: format!("工具缺少调用级权限判定器：{qualified_name}"),
        },
    }
}

fn recoverable_arguments_error(registry: &ToolRegistry, qualified_name: &str) -> String {
    let Some(tool) = registry.tool_by_qualified_name(qualified_name) else {
        return "invalid_arguments: provider 工具参数无法解析，请严格按工具 schema 重写输入"
            .to_owned();
    };
    let ToolInputFormat::Freeform { fallback_field, .. } = &tool.schema.input_format else {
        return "invalid_arguments: provider 工具参数无法解析，请严格按工具 schema 重写输入"
            .to_owned();
    };
    format!(
        "invalid_arguments: provider 工具参数无法解析；{qualified_name} 在 JSON function-call 协议下必须传对象，并将原始文本放入 `{fallback_field}` 字段"
    )
}

fn validate_schedule(input: &ProviderToolCallScheduleInput) -> AxonResult<()> {
    if input.tool_calls.is_empty() {
        return Err(AxonError::tool_invalid_definition(
            "provider tool-call 批量调度不能为空",
        ));
    }
    if input.first_tool_call_id.0 == 0 || input.first_execution_request_id.0 == 0 {
        return Err(AxonError::tool_invalid_definition(
            "provider tool-call 批量调度起始 ID 无效",
        ));
    }
    if input.created_at_ms == 0 {
        return Err(AxonError::tool_invalid_definition(
            "provider tool-call 批量调度时间无效",
        ));
    }
    Ok(())
}

fn offset_tool_call_id(base: ToolCallId, offset: usize) -> AxonResult<ToolCallId> {
    offset_id(base.0, offset)
        .map(ToolCallId)
        .ok_or(AxonError::tool_invalid_definition(
            "provider tool-call 批量调度 tool_call_id 溢出",
        ))
}

fn offset_execution_request_id(
    base: ExecutionRequestId,
    offset: usize,
) -> AxonResult<ExecutionRequestId> {
    offset_id(base.0, offset)
        .map(ExecutionRequestId)
        .ok_or(AxonError::tool_invalid_definition(
            "provider tool-call 批量调度 execution_request_id 溢出",
        ))
}

fn offset_id(base: u64, offset: usize) -> Option<u64> {
    u64::try_from(offset)
        .ok()
        .and_then(|value| base.checked_add(value))
}

#[cfg(test)]
#[path = "provider_tool_schedule_tests.rs"]
mod tests;
