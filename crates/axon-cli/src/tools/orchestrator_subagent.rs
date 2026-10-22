//! Subagent 协作工具执行适配。

use axon_agent_runtime::{
    AgentControlRuntime, AgentPath, JobTerminalKind, ProviderToolCall, SEND_MESSAGE_TOOL_NAME,
    SPAWN_AGENT_TOOL_NAME, SubagentForkContextInput, SubagentForkContextReport,
    SubagentForkContextRuntime, SubagentForkTurns, SubagentJobOutcome, SubagentModelIntent,
    SubagentToolExecutionInput, SubagentToolExecutionReceipt, SubagentToolRuntime,
    agent_run_subagent_tool_receipt,
};
use axon_domain::PaneId;

use crate::run_commands::run_subagent_executor::{
    RunSubagentCompletionSink, RunSubagentContinuationSink, RunSubagentExecutionInput,
    RunSubagentExecutionOutput, RunSubagentExecutionStatus, RunSubagentModelIntent,
};

use super::{
    ToolOrchestratorContext, ToolReceipt, failed_tool_receipt, failed_tool_receipt_from_axon_error,
    now_ms, tool_receipt_from_runtime,
};

#[path = "orchestrator_subagent_graph.rs"]
mod orchestrator_subagent_graph;
#[path = "orchestrator_subagent_intent.rs"]
mod orchestrator_subagent_intent;
#[path = "orchestrator_subagent_sessions.rs"]
mod orchestrator_subagent_sessions;
use super::orchestrator_subagent_job::{
    SubagentJobBridge, merge_job_payload, submit_background_subagent_job,
};
use orchestrator_subagent_graph::{
    restore_target_for_tool, restore_visible_agents_for_tool, sync_spawned_agent_graph,
};
pub(crate) use orchestrator_subagent_intent::spawn_intent_fingerprint;
use orchestrator_subagent_intent::{
    SpawnIntentExecution, claim_spawn_intent, complete_spawn_intent,
};
use orchestrator_subagent_sessions::{
    merge_running_session_payload, merge_sessions_payload, merge_spawn_session_payload,
    sync_messaged_subagent_session,
};

const SUBAGENT_CHILD_USAGE_HINT_REASON: &str = "subagent_usage_hint:child_context";

/// `delegate_to_fast` 回执里的等待模式字段（由参数解析写入）。
const DELEGATE_RUN_IN_BACKGROUND_KEY: &str = "runInBackground";
/// 同步模式回执的模式字段与取值。
const SYNC_MODE_KEY: &str = "mode";
const SYNC_MODE_VALUE: &str = "sync";
/// 同步模式的可审计原因。
const SUBAGENT_TOOL_SYNC_REASON: &str = "subagent_tool:sync_wait";

pub(super) fn execute_subagent_tool(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let prepared = match prepare_subagent_call(tool_call, context) {
        Ok(prepared) => prepared,
        Err(receipt) => return *receipt,
    };
    let owner_fingerprint = match resolve_spawn_intent(context, tool_call, &prepared.current_path) {
        Ok(fingerprint) => fingerprint,
        Err(receipt) => return *receipt,
    };
    let execution =
        match execute_subagent_runtime(context, tool_call, &prepared, owner_fingerprint.as_deref())
        {
            Ok(execution) => execution,
            Err(receipt) => return *receipt,
        };
    finish_subagent_execution(
        context,
        tool_call,
        &prepared.current_path,
        owner_fingerprint.as_deref(),
        execution,
    )
}

struct PreparedSubagentCall {
    completed_at_ms: u64,
    current_path: AgentPath,
    input: SubagentToolExecutionInput,
}

fn prepare_subagent_call(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> Result<PreparedSubagentCall, Box<ToolReceipt>> {
    let completed_at_ms = now_ms();
    let current_path = context.current_agent_path.clone();
    // 协作工具在控制面缺失目标时按需从 graph store 恢复可见代理：
    // 这是重启后 `list_agents` / `send_message` 仍能寻址的前提。
    {
        let mut control = context
            .agent_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        restore_visible_agents_for_tool(context, &mut control, tool_call)
            .map_err(|error| Box::new(failed_tool_receipt_from_axon_error(tool_call, error)))?;
        restore_target_for_tool(context, &mut control, tool_call, &current_path)
            .map_err(|error| Box::new(failed_tool_receipt_from_axon_error(tool_call, error)))?;
        // 信箱事实的补全不能只挂在「有开放子树」的拓扑恢复上：父代理的 mailbox
        // （子代理终态消息的收件方）在没有任何开放子树时同样是收件方，上一进程遗留的
        // 「已投递未读」消息必须在重启后回到内存，否则它们永远读不到（合同 §8.2.5）。
        // 装回是一次性动作，自带幂等保护。
        let mailbox_now_ms = now_ms();
        control
            .hydrate_mailbox_once(mailbox_now_ms)
            .map_err(|error| Box::new(failed_tool_receipt_from_axon_error(tool_call, error)))?;
        control
            .drain_mailbox_delivery(mailbox_now_ms)
            .map_err(|error| Box::new(failed_tool_receipt_from_axon_error(tool_call, error)))?;
    }
    Ok(PreparedSubagentCall {
        completed_at_ms,
        input: SubagentToolExecutionInput {
            tool_call: tool_call.clone(),
            current_path: current_path.clone(),
            remote_requested: false,
            completed_at_ms,
        },
        current_path,
    })
}

fn resolve_spawn_intent(
    context: &ToolOrchestratorContext,
    tool_call: &ProviderToolCall,
    current_path: &AgentPath,
) -> Result<Option<String>, Box<ToolReceipt>> {
    match claim_spawn_intent(context, tool_call, current_path) {
        Some(SpawnIntentExecution::Duplicate(waiter)) => match waiter.wait(context.timeout) {
            Some(Ok(receipt)) => Err(Box::new(tool_receipt_from_runtime(
                agent_run_subagent_tool_receipt(tool_call, &receipt),
            ))),
            Some(Err(reason)) => Err(Box::new(failed_tool_receipt(tool_call, reason))),
            None => Err(Box::new(failed_tool_receipt(
                tool_call,
                "重复子代理意图等待首个回执超时",
            ))),
        },
        Some(SpawnIntentExecution::Owner(fingerprint)) => Ok(Some(fingerprint)),
        None => Ok(None),
    }
}

fn execute_subagent_runtime(
    context: &ToolOrchestratorContext,
    tool_call: &ProviderToolCall,
    prepared: &PreparedSubagentCall,
    owner_fingerprint: Option<&str>,
) -> Result<SubagentToolExecutionPlan, Box<ToolReceipt>> {
    let mut control = context
        .agent_control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let receipt = SubagentToolRuntime
        .execute(&mut control, prepared.input.clone())
        .map_err(|error| {
            let reason = format!("{error:?}");
            complete_spawn_intent(
                context,
                &prepared.current_path,
                owner_fingerprint,
                Err(reason.clone()),
            );
            Box::new(failed_tool_receipt_from_axon_error(tool_call, error))
        })?;
    attach_subagent_session(context, &mut control, receipt, prepared.completed_at_ms)
        .map_err(|error| Box::new(failed_tool_receipt_from_axon_error(tool_call, error)))
}

fn finish_subagent_execution(
    context: &ToolOrchestratorContext,
    tool_call: &ProviderToolCall,
    current_path: &AgentPath,
    owner_fingerprint: Option<&str>,
    execution: SubagentToolExecutionPlan,
) -> ToolReceipt {
    let receipt = match execution {
        SubagentToolExecutionPlan::Ready(receipt) => receipt,
        SubagentToolExecutionPlan::Run {
            mut receipt,
            input,
            run_in_background,
        } => {
            start_subagent_executor(context, &mut receipt, input.as_ref(), run_in_background);
            receipt
        }
        SubagentToolExecutionPlan::Failed(receipt) => {
            complete_spawn_intent(
                context,
                current_path,
                owner_fingerprint,
                Err(receipt.summary.clone()),
            );
            return receipt;
        }
    };
    complete_spawn_intent(
        context,
        current_path,
        owner_fingerprint,
        Ok(receipt.clone()),
    );
    tool_receipt_from_runtime(agent_run_subagent_tool_receipt(tool_call, &receipt))
}

pub(super) enum SubagentToolExecutionPlan {
    Ready(SubagentToolExecutionReceipt),
    Run {
        receipt: SubagentToolExecutionReceipt,
        input: Box<RunSubagentExecutionInput>,
        /// 是否后台执行（合同 §5.1.7 双模式）；`false` 表示同步等待并直接返回回执。
        run_in_background: bool,
    },
    Failed(ToolReceipt),
}

impl From<ToolReceipt> for SubagentToolExecutionPlan {
    fn from(receipt: ToolReceipt) -> Self {
        Self::Failed(receipt)
    }
}

fn attach_subagent_session(
    context: &ToolOrchestratorContext,
    control: &mut AgentControlRuntime,
    receipt: SubagentToolExecutionReceipt,
    completed_at_ms: u64,
) -> axon_domain::AxonResult<SubagentToolExecutionPlan> {
    if !is_completed_subagent_receipt(&receipt) {
        return Ok(SubagentToolExecutionPlan::Ready(receipt));
    }
    match receipt.tool_name.as_str() {
        SPAWN_AGENT_TOOL_NAME | "builtin.spawn_agent" => {
            attach_spawned_subagent_session(context, control, receipt, completed_at_ms)
        }
        SEND_MESSAGE_TOOL_NAME | "builtin.send_message" => {
            sync_messaged_subagent_session(context, receipt)
        }
        axon_agent_runtime::DELEGATE_TO_FAST_TOOL_NAME | "builtin.delegate_to_fast" => {
            attach_spawned_subagent_session(context, control, receipt, completed_at_ms)
        }
        _ => Ok(SubagentToolExecutionPlan::Ready(receipt)),
    }
}

fn attach_spawned_subagent_session(
    context: &ToolOrchestratorContext,
    control: &mut AgentControlRuntime,
    mut receipt: SubagentToolExecutionReceipt,
    completed_at_ms: u64,
) -> axon_domain::AxonResult<SubagentToolExecutionPlan> {
    let Some(task_name) = receipt
        .payload
        .get("taskName")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(SubagentToolExecutionPlan::Ready(receipt));
    };
    let initial_task = receipt
        .payload
        .get("task")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    let summary = format!("spawned {task_name}");
    sync_spawned_agent_graph(context, control, &task_name, completed_at_ms)?;
    let fork_context = plan_spawn_fork_context(context, &receipt)?;
    let mut sessions = context
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let handle = sessions.start(axon_agent_runtime::SubagentSessionStartInput {
        parent_session_id: context.session_id,
        task_path: task_name.clone(),
        forked_from_session_id: Some(context.session_id),
        pane_id: PaneId(context.cell_id.0),
        cwd: context.cwd.to_string_lossy().to_string(),
        model_profile_id: context.model_profile_id,
        permission_snapshot_id: context.permission_snapshot_id,
        created_at_ms: completed_at_ms,
        summary,
    })?;
    receipt.payload =
        merge_spawn_session_payload(&receipt.payload, &task_name, &handle, &fork_context);
    drop(sessions);
    let model_intent = delegate_model_intent(&receipt);
    let execution = subagent_execution_input(
        &task_name,
        initial_task,
        &fork_context,
        completed_at_ms,
        model_intent,
    )?;
    let run_in_background = spawn_run_in_background(&receipt);
    Ok(SubagentToolExecutionPlan::Run {
        receipt,
        input: Box::new(execution),
        run_in_background,
    })
}

/// 本次创建是否后台执行。
///
/// `delegate_to_fast` 由模型按调用决定（缺省后台）；`spawn_agent` 没有等待模式参数，
/// 一律后台：它的生命周期操作本来就固定在 `job_id` 通道上。
fn spawn_run_in_background(receipt: &SubagentToolExecutionReceipt) -> bool {
    if receipt.tool_name.as_str() != axon_agent_runtime::DELEGATE_TO_FAST_TOOL_NAME
        && receipt.tool_name.as_str() != "builtin.delegate_to_fast"
    {
        return true;
    }
    receipt
        .payload
        .get(DELEGATE_RUN_IN_BACKGROUND_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

fn plan_spawn_fork_context(
    context: &ToolOrchestratorContext,
    receipt: &SubagentToolExecutionReceipt,
) -> axon_domain::AxonResult<SubagentForkContextReport> {
    let fork_turns = receipt
        .payload
        .get("forkTurns")
        .and_then(serde_json::Value::as_str);
    let child_usage_hint_text = receipt
        .payload
        .get("taskName")
        .and_then(serde_json::Value::as_str)
        .map(|task_path| child_usage_hint(&context.current_agent_path, task_path));
    SubagentForkContextRuntime.plan(SubagentForkContextInput {
        parent_messages: context.model_messages.clone(),
        fork_turns: SubagentForkTurns::parse(fork_turns)?,
        parent_usage_hint_texts: Vec::new(),
        child_usage_hint_text,
        usage_hint_enabled: true,
    })
}

fn child_usage_hint(parent_path: &AgentPath, task_path: &str) -> String {
    format!(
        "你正在作为 AXON 子代理运行。\n\
         - 当前 canonical task path: {task_path}\n\
         - 直接父代理路径: {parent_path}\n\
         - 可使用相对路径联系同一父代理下的子代理；跨分支通信请使用 /root/... canonical path。\n\
         - send_message 只排队消息，不触发目标代理继续执行；结果与终态按 job_id 用 builtin.job_output 收集。\n\
         - 原因标记: {SUBAGENT_CHILD_USAGE_HINT_REASON}"
    )
}

fn subagent_execution_input(
    task_path: &str,
    message: String,
    fork_context: &SubagentForkContextReport,
    started_at_ms: u64,
    model_intent: RunSubagentModelIntent,
) -> axon_domain::AxonResult<RunSubagentExecutionInput> {
    Ok(RunSubagentExecutionInput {
        task_path: AgentPath::parse(task_path)?,
        message,
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
        fork_context: fork_context.clone(),
        started_at_ms,
        max_runtime_ms: None,
        model_intent,
    })
}

fn delegate_model_intent(receipt: &SubagentToolExecutionReceipt) -> RunSubagentModelIntent {
    match receipt.model_intent {
        SubagentModelIntent::Fast => RunSubagentModelIntent::FastDelegate,
        SubagentModelIntent::Inherit => RunSubagentModelIntent::Inherit,
    }
}

/// 子代理执行前的准入裁决拒绝前缀（合同 §5.2.3）。
const SUBAGENT_REFUSAL_PREFIX: &str = "任务未被接受：";

fn start_subagent_executor(
    context: &ToolOrchestratorContext,
    receipt: &mut SubagentToolExecutionReceipt,
    input: &RunSubagentExecutionInput,
    run_in_background: bool,
) {
    // 作业注册先于准入裁决：只有作业先存在，一次「拒绝」才有通道以统一终态 +
    // 完成通知回流父代理。否则父代理只能从工具回执里推断这次任务没被接受，
    // 而「拒绝必须独立成一类终态」要解决的正是这件事（合同 §5.2.3）。
    //
    // 同步委托（`run_in_background: false`）不注册作业：它按合同 §5.1.7
    // 「同步路径回执直返，不产生 job_id」。
    let bridge = if run_in_background {
        match submit_background_subagent_job(context, &input.task_path, input.max_runtime_ms) {
            Ok(bridge) => {
                receipt.payload = merge_job_payload(&receipt.payload, bridge.job_id);
                Some(bridge)
            }
            Err(error) => {
                // 注册失败发生在执行体开始之前：没有会话需要收口，回执承载阻断即可。
                let reason = format!("{error:?}");
                mark_subagent_start_blocked(receipt, input, &reason);
                return;
            }
        }
    } else {
        None
    };
    let execution_lease = match context
        .agent_control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .lease_execution(&input.task_path)
    {
        Ok(lease) => lease,
        Err(error) => {
            refuse_subagent_execution(receipt, input, bridge.as_ref(), &format!("{error:?}"));
            return;
        }
    };
    let completion = RunSubagentCompletionSink::new(
        context.agent_control.clone(),
        context.agent_graph_store.clone(),
        context.subagent_sessions.clone(),
        execution_lease.clone(),
    );
    // continuation 落盘依赖：未配置 config_path 时保持不产出跨轮引用（与 M2 行为一致），
    // 配置了但库不可达时记录告警并降级：持久化是协作事实的增强，不该阻断子代理启动。
    //
    // 同步模式不绑定 continuation：结果已经直接回到本轮回执里，
    // 再产出跨轮引用只会让下一轮把同一段终态重复注入一次（合同 §5.1.7）。
    let completion = if run_in_background {
        match context.config_path.as_deref() {
            Some(config_path) => match RunSubagentContinuationSink::open(
                config_path,
                context.thread_session_id.clone(),
                context.thread_session_id.clone(),
            ) {
                Ok(sink) => completion.with_continuation(sink),
                Err(error) => {
                    axon_logger::warning(&format!(
                        "子代理 continuation 持久化不可用，本次不产出跨轮引用: task_path={}, error={error}",
                        input.task_path
                    ));
                    completion
                }
            },
            None => completion,
        }
    } else {
        completion
    };
    let session = context
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .begin_execution_by_task_path(
            input.task_path.as_str(),
            execution_lease.execution_id(),
            input.message.clone(),
        );
    let session = match session {
        Ok(session) => session,
        Err(error) => {
            let reason = format!("{error:?}");
            let output = errored_output(&reason);
            if let Err(completion_error) = completion.complete(input, &output) {
                mark_subagent_start_blocked(receipt, input, &format!("{completion_error:?}"));
                return;
            }
            mark_subagent_start_blocked(receipt, input, &reason);
            return;
        }
    };
    receipt.payload = merge_running_session_payload(&receipt.payload, &session);
    if !run_in_background {
        complete_subagent_execution_sync(context, receipt, input, &completion);
        return;
    }
    let completion = match bridge.as_ref() {
        Some(bridge) => completion.with_job(bridge.handle.clone(), bridge.job_id),
        None => completion,
    };
    let started = context
        .subagent_executor
        .start(input.clone(), completion.clone());
    if started {
        set_turn_started(&mut receipt.payload, true);
        return;
    }
    let output = errored_output(SUBAGENT_EXECUTOR_NOT_STARTED);
    if let Err(error) = completion.complete(input, &output) {
        mark_subagent_start_blocked(receipt, input, &format!("{error:?}"));
        return;
    }
    mark_subagent_start_blocked(receipt, input, SUBAGENT_EXECUTOR_NOT_STARTED);
}

/// 同步模式：阻塞执行并把结果直接写进回执（合同 §5.1.7）。
///
/// 同步等待是模型显式选择的例外（`run_in_background: false`），因此这里不做
/// 「后台化」那一套：不注册作业、不产出跨轮引用，结果是本轮回执的一部分。
fn complete_subagent_execution_sync(
    context: &ToolOrchestratorContext,
    receipt: &mut SubagentToolExecutionReceipt,
    input: &RunSubagentExecutionInput,
    completion: &RunSubagentCompletionSink,
) {
    let Some(output) = context.subagent_executor.execute(input.clone()) else {
        let output = errored_output(SUBAGENT_EXECUTOR_NOT_STARTED);
        if let Err(error) = completion.complete(input, &output) {
            mark_subagent_start_blocked(receipt, input, &format!("{error:?}"));
            return;
        }
        mark_subagent_start_blocked(receipt, input, SUBAGENT_EXECUTOR_NOT_STARTED);
        return;
    };
    if let Err(error) = completion.complete(input, &output) {
        axon_logger::warning(&format!(
            "同步子代理完成回写失败: task_path={}, error={error:?}",
            input.task_path
        ));
    }
    merge_sync_result_payload(receipt, &output);
}

/// 把同步模式的执行结果并入工具回执：同步路径的「回执直返」就是这一句。
fn merge_sync_result_payload(
    receipt: &mut SubagentToolExecutionReceipt,
    output: &RunSubagentExecutionOutput,
) {
    let succeeded = output.status == RunSubagentExecutionStatus::Completed;
    let mut payload = receipt.payload.as_object().cloned().unwrap_or_default();
    payload.insert(
        SYNC_MODE_KEY.to_owned(),
        serde_json::Value::String(SYNC_MODE_VALUE.to_owned()),
    );
    payload.insert(
        "status".to_owned(),
        serde_json::Value::String(
            match output.status {
                RunSubagentExecutionStatus::Completed => "completed",
                RunSubagentExecutionStatus::Refused => "refused",
                RunSubagentExecutionStatus::Interrupted => "interrupted",
                RunSubagentExecutionStatus::Errored => "errored",
            }
            .to_owned(),
        ),
    );
    payload.insert(
        "summary".to_owned(),
        serde_json::Value::String(output.summary.clone()),
    );
    payload.insert("turnStarted".to_owned(), serde_json::Value::Bool(false));
    receipt.payload = serde_json::Value::Object(payload);
    receipt.bounded_summary =
        serde_json::to_string(&receipt.payload).unwrap_or_else(|_| "{}".to_owned());
    receipt.status = if succeeded {
        axon_agent_runtime::SubagentToolExecutionStatus::Completed
    } else {
        axon_agent_runtime::SubagentToolExecutionStatus::Blocked
    };
    receipt.reasons.push(SUBAGENT_TOOL_SYNC_REASON.to_owned());
}

/// 构造一次「执行体未能启动」的终态输出。
fn errored_output(reason: &str) -> RunSubagentExecutionOutput {
    RunSubagentExecutionOutput {
        status: RunSubagentExecutionStatus::Errored,
        summary: reason.to_owned(),
        memory_write_count: 0,
        token_usage: None,
        model: None,
        budget_exhausted: false,
    }
}

fn set_turn_started(payload: &mut serde_json::Value, started: bool) {
    if let Some(payload) = payload.as_object_mut() {
        payload.insert("turnStarted".to_owned(), serde_json::Value::Bool(started));
    }
}

fn is_completed_subagent_receipt(receipt: &SubagentToolExecutionReceipt) -> bool {
    receipt.status == axon_agent_runtime::SubagentToolExecutionStatus::Completed
}

/// 子代理执行器未接管时的阻断原因。
const SUBAGENT_EXECUTOR_NOT_STARTED: &str = "subagent_executor:not_started";

/// 准入裁决拒绝：任务未被接受（合同 §5.2.3）。
///
/// 拒绝与失败必须分开投影：`Failed` 说的是「执行中出错」，拒绝说的是
/// 「这次任务根本没被接受」。父代理据此决定是重试、缩小任务范围还是换路子；
/// 把两者压成一个词，它就只能在错误的前提下继续推理。
///
/// 拒绝同样经**作业终态**回流，与其余五类共用同一条完成通知通道；
/// 没有作业通道的同步委托由工具回执承载。
fn refuse_subagent_execution(
    receipt: &mut SubagentToolExecutionReceipt,
    input: &RunSubagentExecutionInput,
    bridge: Option<&SubagentJobBridge>,
    denial: &str,
) {
    let reason = format!("{SUBAGENT_REFUSAL_PREFIX}{denial}");
    if let Some(bridge) = bridge {
        bridge.handle.report(SubagentJobOutcome {
            kind: JobTerminalKind::Refusal,
            exit_reason: Some(reason.clone()),
            summary: reason.clone(),
            final_message: None,
            final_artifact_ref: None,
        });
    }
    mark_subagent_start_blocked(receipt, input, &reason);
    if let Some(bridge) = bridge {
        receipt.payload = merge_job_payload(&receipt.payload, bridge.job_id);
    }
}

fn mark_subagent_start_blocked(
    receipt: &mut SubagentToolExecutionReceipt,
    input: &RunSubagentExecutionInput,
    reason: &str,
) {
    receipt.status = axon_agent_runtime::SubagentToolExecutionStatus::Blocked;
    reason.clone_into(&mut receipt.bounded_summary);
    receipt.payload = merge_sessions_payload(
        &serde_json::json!({
            "agent": input.task_path.to_string(),
            "blocked": true,
            "reason": reason,
            "turnStarted": false,
        }),
        &[],
        Some(&input.fork_context),
    );
    receipt.reasons = vec![reason.to_owned()];
}
