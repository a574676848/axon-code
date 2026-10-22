//! 后台子代理的临时 Job 通道验收（合同 §5.1.2 / §5.1.3 / §5.1.7）。
//!
//! 覆盖四件事：后台子代理可经 `job_id` 读取与终止；`job_kill` 执行完整中断语义；
//! 跨 owner 访问稳定拒绝；`job_id` 是生命周期通道、与 `session_id` 硬互斥。
//! 另有两条守恒断言：`delegate_to_fast` 不产生 `job_id`；超阈值结果只回传有界摘要。

use axon_agent_runtime::{
    AgentPath, AgentStatus, JobRuntime, JobStatus, ProviderToolCall, SubagentSessionStatus,
};
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{ModelProfileId, PermissionSnapshotId};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::run_commands::{
    RunRequest,
    run_subagent_executor::{
        RunSubagentCompletionSink, RunSubagentExecutionInput, RunSubagentExecutionOutput,
        RunSubagentExecutionStatus, RunSubagentExecutor,
    },
};

/// 不读取、仅取当前状态的读取窗口。
const NO_WAIT_MS: u64 = 0;
/// 会话 ID 形态的值：绝不能作为 `job_id` 被接受。
const SESSION_SHAPED_ID: &str = "3f0a1c2e-5b6d-4e7f-8a90-1234567890ab";

fn provider_tool_call_with_args(name: &str, arguments: &serde_json::Value) -> ProviderToolCall {
    ProviderToolCall::new(name, name, arguments.to_string())
}

fn fixture_request_with_executor(subagent_executor: Arc<dyn RunSubagentExecutor>) -> RunRequest {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // 配置路径决定运行态根：必须用测试隔离目录，否则 data/runtime.sqlite3 与 data/logs/tee 会写进 crate 目录。
    let config_path = axon_test_support::test_temp_path("config.toml");
    let session_id = super::unique_test_thread_session_id("session-subagent-job");
    let (startup, execution_tools) =
        crate::run_commands::run_startup_snapshot::test_startup_runtime(
            &config_path,
            &cwd,
            &session_id,
        );
    RunRequest {
        user_question_port: None,
        client_channel: axon_client_identity::ClientChannel::Cli,
        config_path,
        session_id: session_id.clone(),
        invocation_id: format!("test-invocation:{session_id}"),
        resumed_from: None,
        parent_session_id: None,
        cwd,
        prompt: "拆分子任务".to_owned(),
        quoted_message: None,
        structured_output: None,
        selected_skill_name: None,
        selected_skill_source_ref: None,
        model_payload: axon_agent_runtime::AgentModelPayload {
            messages: vec![axon_agent_runtime::AgentModelMessage::text(
                "user",
                "拆分子任务",
            )],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        },
        startup,
        execution_tools,
        workspace_files: Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        provider: ProviderConfig::default(),
        model: ModelConfig::default(),
        reasoning_effort: None,
        channel_chain: Vec::new(),
        routing_key: "test".to_owned(),
        balancer: ChannelBalancerConfig::default(),
        timeout: Duration::from_secs(20),
        memory_recall: None,
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        model_profile_id: ModelProfileId(41),
        permission_snapshot_id: PermissionSnapshotId(42),
        current_agent_path: AgentPath::root(),
        agent_control: Arc::new(Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor,
        subagent_cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        steer_inbox: None,
        auto_compaction_threshold: 0.8,
        performance: None,
    }
}

fn fixture_request() -> RunRequest {
    fixture_request_with_executor(Arc::new(RunningSubagentExecutor))
}

fn spawn_call(task_name: &str) -> ProviderToolCall {
    provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": task_name,
            "task": "审查当前改动"
        }),
    )
}

/// 从 spawn 回执中取出作业 ID；回执必须带 `jobId`，否则子代理没有生命周期通道。
fn job_id_from(receipts: &[crate::tool_orchestrator::ToolReceipt]) -> u64 {
    receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload/jobId"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .expect("后台子代理回执必须携带可解析的 jobId")
}

/// 背景执行器：启动后保持运行，用于观察 `job_output` / `job_kill`。
#[derive(Debug)]
struct RunningSubagentExecutor;

impl RunSubagentExecutor for RunningSubagentExecutor {
    fn execute(&self, _input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        None
    }

    fn start(
        &self,
        _input: RunSubagentExecutionInput,
        _completion: RunSubagentCompletionSink,
    ) -> bool {
        true
    }
}

/// 立即完成的执行器：把给定正文当作子代理结果回写。
#[derive(Debug)]
struct CompletingSubagentExecutor {
    summary: String,
}

impl RunSubagentExecutor for CompletingSubagentExecutor {
    fn execute(&self, _input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        None
    }

    fn start(
        &self,
        input: RunSubagentExecutionInput,
        completion: RunSubagentCompletionSink,
    ) -> bool {
        let output = RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: self.summary.clone(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        };
        completion
            .complete(&input, &output)
            .expect("测试执行器的完成回写必须成功");
        true
    }
}

/// 同步模式测试执行器：`execute` 直接返回结果；同步路径不经过 `start`。
struct SyncCompletingSubagentExecutor {
    summary: String,
}

impl RunSubagentExecutor for SyncCompletingSubagentExecutor {
    fn execute(&self, _input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        Some(RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: self.summary.clone(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        })
    }
}

// ---------- 注册与读取 ----------

#[test]
fn background_subagent_is_registered_as_a_readable_temporary_job() {
    let request = fixture_request();
    let receipts = super::execute_supported_tool_calls(&request, &[spawn_call("review")], None);
    let job_id = job_id_from(&receipts);

    let output = JobRuntime::global()
        .read(
            axon_agent_runtime::JobId(job_id),
            &request.session_id,
            Some(NO_WAIT_MS),
        )
        .expect("后台子代理必须可按 job_id 与 owner 读取");

    assert_eq!(
        output.status,
        JobStatus::Running,
        "刚派发的子代理作业仍在运行"
    );
    assert_eq!(output.label, "/root/review", "作业标签必须是子代理任务路径");
    assert_eq!(output.exit_code, None, "运行中的作业不得有退出码");
    assert!(
        output.artifact_path.exists(),
        "子代理作业必须创建输出归档：{}",
        output.artifact_path.display()
    );
}

#[test]
fn cross_owner_access_to_a_subagent_job_is_rejected() {
    let request = fixture_request();
    let receipts = super::execute_supported_tool_calls(&request, &[spawn_call("review")], None);
    let job_id = axon_agent_runtime::JobId(job_id_from(&receipts));

    let read_error = JobRuntime::global()
        .read(job_id, "intruder-owner", Some(NO_WAIT_MS))
        .expect_err("跨 owner 读取必须被拒绝");
    assert!(
        format!("{read_error:?}").contains("Job 不属于当前 Agent run"),
        "拒绝原因必须是归属不符：{read_error:?}"
    );

    let kill_error = JobRuntime::global()
        .kill(job_id, "intruder-owner")
        .expect_err("跨 owner 终止必须被拒绝");
    assert!(
        format!("{kill_error:?}").contains("Job 不属于当前 Agent run"),
        "拒绝原因必须是归属不符：{kill_error:?}"
    );

    // 反例对照：真 owner 仍可达，证明上面的拒绝没有造成副作用。
    JobRuntime::global()
        .read(job_id, &request.session_id, Some(NO_WAIT_MS))
        .expect("真正的 owner 必须仍可读取自己的子代理作业");
}

// ---------- 完整中断语义 ----------

#[test]
fn job_kill_fully_interrupts_the_subagent_and_projects_a_terminal_state() {
    let request = fixture_request();
    let receipts = super::execute_supported_tool_calls(&request, &[spawn_call("review")], None);
    let job_id = axon_agent_runtime::JobId(job_id_from(&receipts));
    let path = AgentPath::parse("/root/review").expect("测试路径必须合法");

    let status = JobRuntime::global()
        .kill(job_id, &request.session_id)
        .expect("终止运行中的子代理作业必须成功");
    assert_eq!(status, JobStatus::Killed, "job_kill 必须投影为 Killed");

    // 步骤 2：会话被关闭（控制面不再持有该代理）。
    assert_eq!(
        request
            .agent_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status(&path),
        AgentStatus::NotFound,
        "job_kill 必须关闭子代理会话"
    );
    // 步骤 2 的另一半：运行会话元数据被取消，不残留 Running。
    assert_eq!(
        request
            .subagent_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions()
            .iter()
            .find(|handle| handle.task_path == "/root/review")
            .map(|handle| handle.status),
        Some(SubagentSessionStatus::Cancelled),
        "被终止子代理的会话必须进入取消终态，而不是停在 Running"
    );

    // 步骤 3：终态可读且无退出码。
    let output = JobRuntime::global()
        .read(job_id, &request.session_id, Some(NO_WAIT_MS))
        .expect("被终止的作业在保留窗口内仍可读");
    assert_eq!(output.status, JobStatus::Killed);
    assert_eq!(output.exit_code, None, "被终止的子代理作业没有退出码");

    // 重复终止必须报错，而不是二次中断。
    assert!(
        JobRuntime::global()
            .kill(job_id, &request.session_id)
            .is_err(),
        "已终态的作业不得被重复终止"
    );
}

// ---------- 句柄互斥 ----------

#[test]
fn session_shaped_id_is_rejected_by_the_job_channel() {
    let request = fixture_request();
    let call = provider_tool_call_with_args(
        "builtin.job_output",
        &serde_json::json!({ "job_id": SESSION_SHAPED_ID }),
    );
    let receipts = super::execute_supported_tool_calls(&request, &[call], None);

    assert_eq!(
        receipts[0].status, "failed",
        "session_id 形态不得被作业通道接受"
    );
    assert!(
        receipts[0].summary.contains("job_id"),
        "拒绝原因必须点明 job_id 形态要求：{}",
        receipts[0].summary
    );
}

#[test]
fn job_id_is_rejected_by_the_session_channel() {
    let request = fixture_request();
    let receipts = super::execute_supported_tool_calls(&request, &[spawn_call("review")], None);
    let job_id = job_id_from(&receipts);

    let call = provider_tool_call_with_args(
        "builtin.write_stdin",
        &serde_json::json!({ "session_id": job_id.to_string(), "yield_time_ms": 0 }),
    );
    let receipts = super::execute_supported_tool_calls(&request, &[call], None);

    assert_ne!(
        receipts[0].status, "completed",
        "作业 ID 不得被交互会话通道接受：{}",
        receipts[0].summary
    );
}

// ---------- 结果回流与超阈值归档 ----------

#[test]
fn short_subagent_result_is_inlined_into_the_job_receipt() {
    let summary = "结论：当前改动安全，建议合并。".to_owned();
    let request = fixture_request_with_executor(Arc::new(CompletingSubagentExecutor {
        summary: summary.clone(),
    }));
    let receipts = super::execute_supported_tool_calls(&request, &[spawn_call("review")], None);
    let job_id = axon_agent_runtime::JobId(job_id_from(&receipts));

    let output = JobRuntime::global()
        .read(job_id, &request.session_id, Some(NO_WAIT_MS))
        .expect("已完成的子代理作业必须可读");

    assert_eq!(output.status, JobStatus::Completed);
    assert!(!output.stdout.truncated, "短结果不得被判定为截断");
    assert_eq!(output.stdout.preview, summary, "短结果必须内联回传");
    assert!(
        std::fs::read_to_string(&output.artifact_path)
            .expect("作业归档必须可读")
            .contains(&summary),
        "完整结果必须落盘归档"
    );
}

#[test]
fn oversized_subagent_result_is_archived_and_only_a_bounded_summary_returns() {
    let summary = "长".repeat(axon_agent_runtime::JOB_SUMMARY_MAX_CHARS + 200);
    let request = fixture_request_with_executor(Arc::new(CompletingSubagentExecutor {
        summary: summary.clone(),
    }));
    let receipts = super::execute_supported_tool_calls(&request, &[spawn_call("review")], None);
    let job_id = axon_agent_runtime::JobId(job_id_from(&receipts));

    let output = JobRuntime::global()
        .read(job_id, &request.session_id, Some(NO_WAIT_MS))
        .expect("已完成的子代理作业必须可读");

    assert_eq!(output.status, JobStatus::Completed);
    assert!(
        output.stdout.truncated,
        "超阈值结果必须只回传有界摘要，不得整段进回执"
    );
    assert_eq!(
        output.stdout.original_chars,
        summary.chars().count(),
        "原始长度必须如实报告"
    );
    assert!(
        output.stdout.retained_chars < output.stdout.original_chars,
        "保留长度必须小于原始长度"
    );
    assert!(
        std::fs::read_to_string(&output.artifact_path)
            .expect("作业归档必须可读")
            .contains(&summary),
        "超阈值结果必须完整落盘到作业归档"
    );
}

// ---------- 双模式：delegate_to_fast（合同 §5.1.7） ----------

/// 默认后台：回执带 `jobId`，且作业注册进注册表（与 `spawn_agent` 同一通道）。
#[test]
fn delegate_to_fast_defaults_to_background_and_registers_a_job() {
    let request = fixture_request();
    let call = provider_tool_call_with_args(
        "builtin.delegate_to_fast",
        &serde_json::json!({ "task": "扫描 auth 模块" }),
    );
    let receipts = super::execute_supported_tool_calls(&request, &[call], None);

    assert_eq!(receipts[0].status, "completed");
    assert!(
        receipts[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/payload/jobId"))
            .is_some(),
        "默认后台委托必须返回 job_id：{:?}",
        receipts[0].metadata
    );
    assert_eq!(
        JobRuntime::global()
            .pending_job_ids(&request.session_id)
            .len(),
        1,
        "默认后台委托必须注册进作业注册表"
    );
    // 守恒的另一半：会话侧确实产生了子代理（说明上面的注册不是因为别的路径）。
    assert_eq!(
        request
            .subagent_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions()
            .len(),
        1
    );
}

/// 显式同步：`run_in_background: false` 回执直返，不产生 `job_id`、不注册作业。
#[test]
fn delegate_to_fast_sync_mode_returns_inline_receipt_without_a_job() {
    let summary = "auth 模块结论：无越权写入".to_owned();
    let request = fixture_request_with_executor(Arc::new(SyncCompletingSubagentExecutor {
        summary: summary.clone(),
    }));
    let call = provider_tool_call_with_args(
        "builtin.delegate_to_fast",
        &serde_json::json!({ "task": "扫描 auth 模块", "run_in_background": false }),
    );
    let receipts = super::execute_supported_tool_calls(&request, &[call], None);

    assert_eq!(receipts[0].status, "completed");
    let payload = receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload"))
        .cloned()
        .unwrap_or_default();
    assert!(
        payload.pointer("/jobId").is_none() && payload.pointer("/jobIdHint").is_none(),
        "同步路径不得产生 job_id：{payload}"
    );
    assert_eq!(payload["mode"], serde_json::json!("sync"));
    assert_eq!(payload["summary"], serde_json::json!(summary));
    assert!(
        JobRuntime::global()
            .pending_job_ids(&request.session_id)
            .is_empty(),
        "同步路径不得注册进作业注册表"
    );
}
