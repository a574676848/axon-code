//! 后台子代理与临时 Job 注册表的桥接（合同 §5.1.2 / §5.1.3）。
//!
//! 后台子代理注册进**同一**作业注册表，因此获得与 Shell 作业一致的 owner 校验、
//! 终态保留、墓碑与回收，以及同一条 `job_output` / `job_kill` 消费通道。
//!
//! owner 与 Shell 作业同源（运行时注入的 `thread_session_id`），不由模型提供；
//! 跨 owner 访问由注册表稳定拒绝。
//!
//! 边界：寻址规则写死：生命周期操作（读输出、等待、终止）走 `job_id`，
//! 协作操作（发消息、列表）走 `AgentPath`。

use std::sync::{Arc, Mutex};

use axon_agent_runtime::{
    AgentControlGraphSyncRuntime, AgentControlRuntime, AgentGraphStore, AgentPath, JobId,
    JobRuntime, JobTerminalKind, RunCancelToken, SubagentJobHandle, SubagentJobInterrupt,
    SubagentJobSpec, SubagentSessionRuntime, submit_subagent_job,
};

use super::ToolOrchestratorContext;
use crate::{
    run_commands::run_subagent_executor::RunSubagentExecutor,
    tool_orchestrator_shell::shell_tee_root,
};

/// 子代理作业桥接结果：作业 ID 与宿主持有的结果回写句柄。
pub(super) struct SubagentJobBridge {
    pub(super) job_id: JobId,
    pub(super) handle: SubagentJobHandle,
}

/// 提交后台子代理作业。
pub(super) fn submit_background_subagent_job(
    context: &ToolOrchestratorContext,
    path: &AgentPath,
    max_runtime_ms: Option<u64>,
) -> axon_domain::AxonResult<SubagentJobBridge> {
    let spec = SubagentJobSpec {
        owner_id: context.thread_session_id.clone(),
        label: path.to_string(),
        tee_root: shell_tee_root(context),
        max_runtime_ms,
        interrupt: Arc::new(SubagentJobInterruptHandle {
            path: path.clone(),
            agent_control: Arc::clone(&context.agent_control),
            agent_graph_store: context.agent_graph_store.clone(),

            subagent_sessions: Arc::clone(&context.subagent_sessions),
            subagent_cancel_tokens: Arc::clone(&context.subagent_cancel_tokens),
            subagent_executor: Arc::clone(&context.subagent_executor),
        }),
    };
    let (job_id, handle) = submit_subagent_job(JobRuntime::global(), spec)?;
    Ok(SubagentJobBridge { job_id, handle })
}

/// 把作业 ID 并入子代理回执载荷，使模型能凭 `job_id` 走生命周期通道。
pub(super) fn merge_job_payload(payload: &serde_json::Value, job_id: JobId) -> serde_json::Value {
    let mut payload = payload.as_object().cloned().unwrap_or_default();
    payload.insert(
        "jobId".to_owned(),
        serde_json::Value::String(job_id.to_string()),
    );
    payload.insert(
        "jobIdHint".to_owned(),
        serde_json::Value::String(
            "该子代理是临时 Job：用 job_output 读取结果与状态、用 job_kill 完整中断；job_id 不能传给 write_stdin 或 send_message。"
                .to_owned(),
        ),
    );
    serde_json::Value::Object(payload)
}

/// `job_kill` 对子代理的完整中断语义（合同 §5.1.2）：中止 turn → 关闭会话 → 显式终态。
///
/// 终态投影、完成通知与回收由作业内核在 `terminate()` 之后承担，本句柄不重复实现。
struct SubagentJobInterruptHandle {
    path: AgentPath,
    agent_control: Arc<Mutex<AgentControlRuntime>>,
    agent_graph_store: Arc<dyn AgentGraphStore>,

    subagent_sessions: Arc<Mutex<SubagentSessionRuntime>>,
    subagent_cancel_tokens: Arc<Mutex<std::collections::HashMap<AgentPath, RunCancelToken>>>,
    subagent_executor: Arc<dyn RunSubagentExecutor>,
}

impl SubagentJobInterrupt for SubagentJobInterruptHandle {
    fn interrupt(&self, terminal: JobTerminalKind) {
        // 加锁顺序固定为「cancel token → agent control → sessions」，
        // 与完成回写路径（control → sessions）不构成交叉持锁，不会死锁。
        cancel_subtree_turns(&self.subagent_cancel_tokens, &self.path);
        // graph store 必须在控制面移除该子树**之前**同步关闭：否则重启恢复会把
        // 已终止的子代理当成仍然开放的子树重新挂回控制面。
        {
            let control = self
                .agent_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Err(error) =
                sync_closing_agent_graph(self.agent_graph_store.as_ref(), &control, &self.path)
            {
                axon_logger::warning(&format!(
                    "子代理关闭的 graph 投影失败: task_path={}, error={error:?}",
                    self.path
                ));
            }
        }
        self.agent_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .close(&self.path)
            .ok();
        self.subagent_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel_subtree_by_task_path(
                self.path.as_str(),
                format!("{} {}", JOB_INTERRUPT_SUMMARY_PREFIX, self.path),
            );
        // 六类终态由作业内核判定后交给宿主投影：宿主不重猜原因，
        // 因此 TUI/Wire 看到的结局与 Job 终态、通知载荷始终是同一个词。
        self.subagent_executor
            .notify_explicit_terminal(std::slice::from_ref(&self.path), terminal);
    }
}

/// 把一次子代理关闭同步进 graph store。
fn sync_closing_agent_graph(
    store: &dyn AgentGraphStore,
    control: &AgentControlRuntime,
    path: &AgentPath,
) -> axon_domain::AxonResult<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| axon_domain::AxonError::runtime_failure("subagent_graph:runtime"))?
        .block_on(AgentControlGraphSyncRuntime.sync_closing_agents(
            control,
            store,
            path,
            crate::tool_orchestrator::now_ms(),
        ))
        .map(|_| ())
}

/// 中断导致的会话摘要前缀。
const JOB_INTERRUPT_SUMMARY_PREFIX: &str = "job_kill 终止";

/// 中止目标路径及其子孙正在运行的 turn。
fn cancel_subtree_turns(
    cancel_tokens: &Arc<Mutex<std::collections::HashMap<AgentPath, RunCancelToken>>>,
    path: &AgentPath,
) {
    let mut tokens = cancel_tokens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cancelled_paths = tokens
        .keys()
        .filter(|candidate| candidate.matches_prefix(path))
        .cloned()
        .collect::<Vec<_>>();
    for cancelled_path in cancelled_paths {
        if let Some(token) = tokens.remove(&cancelled_path) {
            token.cancel();
        }
    }
}
