//! builtin.job_output / builtin.job_kill 执行适配回归。
//!
//! 覆盖模型实际能观察到的行为：参数非法即拒绝、增量输出不重复、终止后进入终态、
//! 以及作业按 owner 隔离。作业使用真实短进程，因此这里同时验证了
//! 「作业注册表与会话注册表相互独立」这条构造性保证。

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use axon_agent_runtime::{
    AGENT_TOOL_NAMESPACE, JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME, JOB_TOOL_JOB_ID_FIELD, JobId,
    ProviderToolCall, ShellCommandExecutionInput, ShellCommandToolRuntime,
};

use super::{execute_job_kill, execute_job_output};
use crate::tool_orchestrator::{ToolOrchestratorContext, ToolReceipt};

/// 增量用例的首个输出标记。
const INCREMENTAL_FIRST: &str = "axon-job-first";
/// 增量用例的第二个输出标记。
const INCREMENTAL_SECOND: &str = "axon-job-second";
/// 两个标记之间的间隔；必须长于一次读取的往返，才能观察到两批独立输出。
const INCREMENTAL_GAP: &str = "1";
/// 轮询首个标记的有界期限。
const MARKER_DEADLINE: Duration = Duration::from_secs(10);
/// 轮询间隔。
const MARKER_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// 长任务用例的睡眠时长。
const LONG_TASK_SECONDS: u64 = 60;

/// 臆造的作业 ID 必须被拒绝，且不得被会话 ID 蒙混过关。
#[test]
fn job_output_rejects_malformed_job_id() {
    let tool_call = job_tool_call(
        JOB_OUTPUT_TOOL_NAME,
        serde_json::json!({ JOB_TOOL_JOB_ID_FIELD: "sess_123" }),
    );

    let receipt = execute_job_output(&tool_call, &test_context("cli-job-malformed"));

    assert_eq!(receipt.status, "failed");
    assert_eq!(receipt.error_type.as_deref(), Some("invalid_arguments"));
    assert!(
        receipt.summary.contains(JOB_TOOL_JOB_ID_FIELD),
        "拒绝原因必须点名字段：{}",
        receipt.summary
    );
}

/// 从未产生过的作业 ID 必须报告「不存在」，而不是伪装成成功。
#[test]
fn job_output_reports_unknown_job_id() {
    let tool_call = job_tool_call(
        JOB_OUTPUT_TOOL_NAME,
        serde_json::json!({ JOB_TOOL_JOB_ID_FIELD: "999999" }),
    );

    let receipt = execute_job_output(&tool_call, &test_context("cli-job-unknown"));

    assert_eq!(receipt.status, "failed");
    assert!(
        receipt.summary.contains("不存在"),
        "未知作业必须报告不存在：{}",
        receipt.summary
    );
}

/// 作业输出是**增量**的：已消费的批次不得在后续读取中重复出现。
#[test]
fn job_output_returns_incremental_batches_only_once() {
    let owner = "cli-job-incremental";
    let job_id = start_job(owner, &incremental_script());
    let mut consumed = String::new();
    let deadline = Instant::now() + MARKER_DEADLINE;

    while !consumed.contains(INCREMENTAL_FIRST) && Instant::now() < deadline {
        consumed.push_str(&stdout_preview(&call_job_output(owner, job_id, 0)));
        thread::sleep(MARKER_POLL_INTERVAL);
    }
    assert!(
        consumed.contains(INCREMENTAL_FIRST),
        "首段输出必须在有界轮询内可见：{consumed}"
    );

    let second = stdout_preview(&call_job_output(owner, job_id, 0));

    assert!(
        !second.contains(INCREMENTAL_FIRST),
        "已消费的增量不得重复返回：{second}"
    );
    // 用例结束时经生产终止通道回收自己启动的作业。
    reap_job_via_kill(owner, job_id);
}

/// 作业启动回执只承诺「仍在运行」，且返回 job_id 而不是 session_id。
#[test]
fn job_output_reads_running_status_without_claiming_success() {
    let owner = "cli-job-running";
    let job_id = start_job(owner, &long_script());

    let receipt = call_job_output(owner, job_id, 0);

    assert_eq!(receipt.status, "running");
    assert_eq!(receipt.error_type, None);
    let metadata = receipt.metadata.expect("作业回执必须携带 metadata");
    assert_eq!(metadata["kind"], "job");
    assert_eq!(metadata["jobId"], job_id.to_string());
    assert!(
        metadata["exitCode"].is_null(),
        "Running 时不得给出退出码：{metadata}"
    );
    assert!(
        metadata.get("sessionId").is_none(),
        "作业回执不得出现 session_id：{metadata}"
    );
    reap_job_via_kill(owner, job_id);
}

/// `job_kill` 必须真正终止进程并报告终态。
#[test]
fn job_kill_terminates_the_process() {
    let owner = "cli-job-kill";
    let job_id = start_job(owner, &long_script());

    let receipt = call_job_kill(owner, job_id);

    assert_eq!(receipt.status, "killed");
    assert_eq!(receipt.error_type.as_deref(), Some("killed"));
    let metadata = receipt.metadata.expect("终止回执必须携带 metadata");
    assert_eq!(metadata["jobStatus"], "killed");
    assert_eq!(
        call_job_output(owner, job_id, 0).status,
        "killed",
        "终止后读取必须看到终态，而不是重新运行"
    );
}

/// owner 隔离：其他 Agent run 不得读取或终止该作业。
#[test]
fn job_tools_are_isolated_by_owner() {
    let owner = "cli-job-owner";
    let other = "cli-job-owner-other";
    let job_id = start_job(owner, &long_script());

    let read = call_job_output(other, job_id, 0);
    let kill = call_job_kill(other, job_id);

    assert_eq!(read.status, "failed");
    assert_eq!(kill.status, "failed");
    assert!(
        read.summary.contains("不属于当前 Agent run"),
        "越权读取必须报告 owner 不符：{}",
        read.summary
    );
    assert_eq!(
        call_job_output(owner, job_id, 0).status,
        "running",
        "越权调用不得影响原 owner 的作业"
    );
    reap_job_via_kill(owner, job_id);
}

fn call_job_output(owner: &str, job_id: JobId, yield_time_ms: u64) -> ToolReceipt {
    let tool_call = job_tool_call(
        JOB_OUTPUT_TOOL_NAME,
        serde_json::json!({
            JOB_TOOL_JOB_ID_FIELD: job_id.to_string(),
            axon_agent_runtime::JOB_TOOL_YIELD_TIME_FIELD: yield_time_ms,
        }),
    );

    execute_job_output(&tool_call, &test_context(owner))
}

fn call_job_kill(owner: &str, job_id: JobId) -> ToolReceipt {
    let tool_call = job_tool_call(
        JOB_KILL_TOOL_NAME,
        serde_json::json!({ JOB_TOOL_JOB_ID_FIELD: job_id.to_string() }),
    );

    execute_job_kill(&tool_call, &test_context(owner))
}

/// 用例结束时的作业回收：命令可能已自然终态（如增量脚本跑完），此时生产的
/// `job_kill` 会报告「已进入终态」：同样证明作业曾被登记且回收目标已达成。
fn reap_job_via_kill(owner: &str, job_id: JobId) {
    let receipt = call_job_kill(owner, job_id);
    if receipt.status == "killed" {
        return;
    }
    let detail = format!(
        "summary={:?} error_type={:?}",
        receipt.summary, receipt.error_type
    );
    assert!(
        detail.contains("已进入终态"),
        "作业清理只接受 killed 或「已自然终态」：status={} / {detail}",
        receipt.status
    );
}

fn job_tool_call(tool_name: &str, input: serde_json::Value) -> ProviderToolCall {
    ProviderToolCall::new(
        format!("call-{tool_name}"),
        format!("{AGENT_TOOL_NAMESPACE}.{tool_name}"),
        input,
    )
}

/// 用生产的 shell 工具入口启动作业：测试与真实路径共用同一套校验链。
fn start_job(owner: &str, script: &str) -> JobId {
    ShellCommandToolRuntime
        .start_background(ShellCommandExecutionInput {
            owner_id: owner.to_owned(),
            command: ShellCommandToolRuntime::command_from_script(script, false)
                .expect("命令应规范化成功"),
            cwd: test_workspace(),
            timeout_ms: None,
            yield_time_ms: 0,
            tty: false,
            run_in_background: true,
            max_output_tokens: None,
            tee_root: Some(axon_test_support::test_temp_root()),
            cancel_token: None,
            write_paths: Vec::new(),
            remote_requested: false,
        })
        .expect("后台作业应启动成功")
}

fn stdout_preview(receipt: &ToolReceipt) -> String {
    receipt
        .metadata
        .as_ref()
        .and_then(|metadata| metadata["output"]["stdout"]["preview"].as_str())
        .unwrap_or_default()
        .to_owned()
}

fn test_workspace() -> PathBuf {
    let root =
        axon_test_support::test_temp_root().join(format!("axon-cli-job-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("测试工作区应可创建");
    root
}

#[cfg(windows)]
fn incremental_script() -> String {
    format!(
        "Write-Output '{INCREMENTAL_FIRST}'; Start-Sleep -Seconds {INCREMENTAL_GAP}; Write-Output '{INCREMENTAL_SECOND}'"
    )
}

#[cfg(unix)]
fn incremental_script() -> String {
    format!(
        "printf '{INCREMENTAL_FIRST}\\n'; sleep {INCREMENTAL_GAP}; printf '{INCREMENTAL_SECOND}\\n'"
    )
}

#[cfg(windows)]
fn long_script() -> String {
    format!("Start-Sleep -Seconds {LONG_TASK_SECONDS}")
}

#[cfg(unix)]
fn long_script() -> String {
    format!("sleep {LONG_TASK_SECONDS}")
}

fn test_context(owner: &str) -> ToolOrchestratorContext {
    ToolOrchestratorContext {
        config_path: None,
        cwd: test_workspace(),
        session_id: axon_domain::SessionId(1),
        thread_session_id: owner.to_owned(),
        cell_id: axon_domain::CellId(1),
        model_messages: Vec::new(),
        execution_tools: std::sync::Arc::new(std::sync::RwLock::new(
            axon_agent_runtime::ToolRegistry::new(),
        )),
        memory_write_runtime: std::sync::Arc::new(
            crate::run_commands::AgentMemoryWriteRuntime::default(),
        ),
        timeout: Duration::from_secs(15),
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        run_cancel_token: None,
        model_profile_id: axon_domain::ModelProfileId(0),
        supports_vision: true,
        permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: std::sync::Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: std::sync::Arc::new(
            crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
        ),
        subagent_cancel_tokens: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        user_question_port: None,
    }
}
