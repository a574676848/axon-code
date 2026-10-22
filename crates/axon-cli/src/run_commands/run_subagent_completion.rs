//! 子代理完成状态的权威收口。

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use axon_agent_runtime::{
    AgentControlGraphSyncRuntime, AgentControlRuntime, AgentExecutionLease, AgentGraphStore,
    AgentPath, AgentStatus, CONTINUATION_RETENTION, CONTINUATION_SUMMARY_MAX_CHARS,
    JOB_REASON_BUDGET_EXHAUSTED, JobId, JobTerminalKind, SubagentJobHandle, SubagentJobOutcome,
    SubagentSessionHandle, SubagentSessionRuntime, SubagentSessionStatus, job_reference, millis_of,
};
use axon_domain::{AxonError, AxonResult};
use axon_session::{
    AgentContinuationQuery, AgentContinuationRecord, AgentContinuationStore, SqliteRuntimeStore,
};

use super::{
    RunCancelToken,
    run_subagent_executor::{
        RunSubagentExecutionInput, RunSubagentExecutionOutput, RunSubagentExecutionStatus,
        extract_result_path, strip_result_path,
    },
    run_subagent_executor_support::child_session_id,
    run_subagent_lifecycle::{
        SubagentLifecyclePublisher, SubagentLifecycleRegistry, subagent_terminal_lifecycle,
    },
};

const COMPLETION_ERROR_PREFIX: &str = "subagent_completion";
const COMPLETION_SESSION_REASON_EXECUTION_NOT_FOUND: &str = "session_execution_not_found";
/// 子代理正常结束的作业退出原因。
const JOB_REASON_SUBAGENT_COMPLETED: &str = "子代理正常结束";
/// 子代理被中断的作业退出原因。
const JOB_REASON_SUBAGENT_INTERRUPTED: &str = "子代理被中断";
/// 子代理执行失败的作业退出原因。
const JOB_REASON_SUBAGENT_ERRORED: &str = "子代理执行失败";
/// 子代理执行被准入裁决拒绝的退出原因前缀。
const JOB_REASON_SUBAGENT_REFUSED_PREFIX: &str = "任务未被接受：";
/// 子代理执行被准入裁决拒绝的退出原因（无细节时的固定措辞）。
const JOB_REASON_SUBAGENT_REFUSED: &str = "任务未被接受";
/// continuation 幂等键前缀。
const CONTINUATION_KEY_PREFIX: &str = "continuation";

/// 子代理终态 continuation 的落盘依赖。
///
/// 与 mailbox 的端口注入不同，本类型直接持有 `axon-session` 的仓储：它位于
/// `axon-cli`，而该 crate 本就依赖 `axon-session`，再套一层端口只会增加无谓的间接。
#[derive(Clone)]
pub(crate) struct RunSubagentContinuationSink {
    store: SqliteRuntimeStore,
    owner_id: String,
    parent_session_id: String,
}

impl RunSubagentContinuationSink {
    /// 打开共享 runtime 库并绑定作用域。
    ///
    /// 库与 `axon run` 主链路同源，因此下一轮（含重启后的会话）能读到本轮写入的引用。
    pub(crate) fn open(
        config_path: &Path,
        owner_id: impl Into<String>,
        parent_session_id: impl Into<String>,
    ) -> Result<Self, String> {
        Ok(Self {
            store: super::run_agent_store::shared_runtime_store(config_path)?,
            owner_id: owner_id.into(),
            parent_session_id: parent_session_id.into(),
        })
    }

    /// 目标子代理是否存在已落盘的终态引用。
    ///
    /// 这是**会话级 re-attach 的前置校验**（设计 §6.2）：有引用才说明这个子代理真的
    /// 跑过、子会话已经落盘；没有引用的「空闲」目标只是控制面里的空壳，唤醒它等于
    /// 凭空造一个没有历史的会话。
    ///
    /// 查询键用 `child_session_id`，与终态写入时是同一个推导口径
    /// （`child_session_id(父会话, AgentPath)`），因此查询结果与写入口径不会漂移。
    pub(crate) fn has_terminal_reference(&self, task_path: &axon_agent_runtime::AgentPath) -> bool {
        let child_session_id = child_session_id(&self.parent_session_id, task_path);
        self.store
            .list_continuations(&AgentContinuationQuery {
                owner_id: Some(self.owner_id.clone()),
                parent_session_id: Some(self.parent_session_id.clone()),
                child_session_id: Some(child_session_id),
                state: None,
                limit: 1,
            })
            .is_ok_and(|records| !records.is_empty())
    }
}

#[derive(Clone)]
struct SubagentCancelTokenRegistration {
    registry: Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>,
    execution_token: RunCancelToken,
}

/// 子代理完成回写入口。
#[derive(Clone)]
pub(crate) struct RunSubagentCompletionSink {
    agent_control: Arc<Mutex<AgentControlRuntime>>,
    agent_graph_store: Arc<dyn AgentGraphStore>,
    subagent_sessions: Arc<Mutex<SubagentSessionRuntime>>,
    lifecycle: Option<(SubagentLifecycleRegistry, SubagentLifecyclePublisher)>,
    cancel_token: Option<SubagentCancelTokenRegistration>,
    execution_lease: AgentExecutionLease,
    job: Option<SubagentJobHandle>,
    job_id: Option<JobId>,
    continuation: Option<RunSubagentContinuationSink>,
}

impl RunSubagentCompletionSink {
    pub(crate) fn new(
        agent_control: Arc<Mutex<AgentControlRuntime>>,
        agent_graph_store: Arc<dyn AgentGraphStore>,
        subagent_sessions: Arc<Mutex<SubagentSessionRuntime>>,
        execution_lease: AgentExecutionLease,
    ) -> Self {
        Self {
            agent_control,
            agent_graph_store,
            subagent_sessions,
            lifecycle: None,
            cancel_token: None,
            execution_lease,
            job: None,
            job_id: None,
            continuation: None,
        }
    }

    /// 绑定临时 Job 回写句柄；子代理结果同时回流到作业注册表。
    ///
    /// `job_id` 一并保留，用于在 continuation 里写下可读的作业线索；
    /// 它不是解析凭据（进程代际跨进程失效）。
    pub(crate) fn with_job(mut self, handle: SubagentJobHandle, job_id: JobId) -> Self {
        self.job = Some(handle);
        self.job_id = Some(job_id);
        self
    }

    /// 绑定 continuation 落盘依赖；未绑定时不产出跨轮引用。
    pub(crate) fn with_continuation(mut self, sink: RunSubagentContinuationSink) -> Self {
        self.continuation = Some(sink);
        self
    }

    pub(super) fn with_lifecycle(
        mut self,
        registry: SubagentLifecycleRegistry,
        publisher: SubagentLifecyclePublisher,
    ) -> Self {
        self.lifecycle = Some((registry, publisher));
        self
    }

    pub(super) fn with_cancel_token(
        mut self,
        cancel_tokens: Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>,
        execution_token: RunCancelToken,
    ) -> Self {
        self.cancel_token = Some(SubagentCancelTokenRegistration {
            registry: cancel_tokens,
            execution_token,
        });
        self
    }

    pub(crate) fn complete(
        &self,
        input: &RunSubagentExecutionInput,
        output: &RunSubagentExecutionOutput,
    ) -> AxonResult<Option<SubagentSessionHandle>> {
        self.report_job(output);
        let mut errors = Vec::new();
        let handle = self.complete_authoritative_state(input, output, &mut errors);
        if handle.is_some() {
            // 只有权威状态确实收口（本次执行代际有效）时才产出 continuation：
            // 迟到回写不该为未生效的终态留下可被下一轮装配的引用。
            self.persist_continuation(input, output, &mut errors);
        }
        self.cleanup(input);

        if errors.is_empty() {
            Ok(handle)
        } else {
            Err(AxonError::runtime_failure(format!(
                "{COMPLETION_ERROR_PREFIX}: {}",
                errors.join("; ")
            )))
        }
    }

    /// 把子代理终态与结果回流到临时 Job 注册表。
    ///
    /// 先于权威状态收口执行：`job_kill` 造成的终态由作业内核先写入，
    /// 这里的迟到回写因而被幂等忽略，不会把已终止作业改写成别的结局。
    fn report_job(&self, output: &RunSubagentExecutionOutput) {
        let Some(job) = self.job.as_ref() else {
            return;
        };
        let (kind, reason) = job_terminal_of(output);
        job.report(SubagentJobOutcome {
            kind,
            exit_reason: Some(reason),
            summary: output.summary.clone(),
            final_message: final_message_of(output),
            final_artifact_ref: extract_result_path(&output.summary),
        });
    }

    fn complete_authoritative_state(
        &self,
        input: &RunSubagentExecutionInput,
        output: &RunSubagentExecutionOutput,
        errors: &mut Vec<String>,
    ) -> Option<SubagentSessionHandle> {
        let requested_status = output.agent_status();
        let mut control = self
            .agent_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let handle = if control.is_current_execution(&self.execution_lease) {
            let mut sessions = self
                .subagent_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.update_session(&mut sessions, input, output, errors)
        } else {
            None
        };
        let report = match control.complete_execution(
            &self.execution_lease,
            requested_status.clone(),
            input.started_at_ms,
        ) {
            Ok(report) => report,
            Err(error) => {
                errors.push(format!("agent_control={error:?}"));
                self.publish_terminal(input, output, &requested_status, errors);
                return None;
            }
        };
        if report.execution_applied
            && let Err(error) = sync_completed_subagent_graph(
                &control,
                self.agent_graph_store.as_ref(),
                &input.task_path,
                input.started_at_ms,
            )
        {
            errors.push(format!("agent_graph={error:?}"));
        }
        let handle = report.execution_applied.then_some(handle).flatten();
        self.publish_terminal(input, output, &report.status, errors);
        handle
    }

    fn update_session(
        &self,
        sessions: &mut SubagentSessionRuntime,
        input: &RunSubagentExecutionInput,
        output: &RunSubagentExecutionOutput,
        errors: &mut Vec<String>,
    ) -> Option<SubagentSessionHandle> {
        let status = match output.status {
            RunSubagentExecutionStatus::Completed => SubagentSessionStatus::Completed,
            RunSubagentExecutionStatus::Refused => SubagentSessionStatus::Refused,
            RunSubagentExecutionStatus::Interrupted => SubagentSessionStatus::Cancelled,
            RunSubagentExecutionStatus::Errored => SubagentSessionStatus::Errored,
        };
        let result = sessions.finish_execution_by_task_path(
            input.task_path.as_str(),
            self.execution_lease.execution_id(),
            status,
            strip_result_path(&output.summary),
        );
        match result {
            Ok(Some(handle)) => Some(handle),
            Ok(None) => {
                errors.push(format!(
                    "session={COMPLETION_SESSION_REASON_EXECUTION_NOT_FOUND}"
                ));
                None
            }
            Err(error) => {
                errors.push(format!("session={error:?}"));
                None
            }
        }
    }

    /// 把子代理终态与其结果引用写入 continuation（合同 §5.3.3）。
    ///
    /// 引用必须重启后可解析：`artifact_path` 是磁盘绝对路径、`summary` 是落盘文本，
    /// 两者共同承担解析职责；`job_ref` 只作可读线索：它含进程代际，跨进程必然失效。
    fn persist_continuation(
        &self,
        input: &RunSubagentExecutionInput,
        output: &RunSubagentExecutionOutput,
        errors: &mut Vec<String>,
    ) {
        let Some(sink) = self.continuation.as_ref() else {
            return;
        };
        let child_session_id = child_session_id(&sink.parent_session_id, &input.task_path);
        let terminal_kind = continuation_terminal_kind(output.status);
        let now_ms = crate::tool_orchestrator::now_ms();
        let mut record = AgentContinuationRecord::pending(
            uuid::Uuid::new_v4().to_string(),
            format!(
                "{CONTINUATION_KEY_PREFIX}:{}:{child_session_id}:{terminal_kind}",
                sink.owner_id
            ),
            sink.owner_id.clone(),
            sink.parent_session_id.clone(),
            child_session_id,
            input.task_path.as_str(),
            terminal_kind,
            continuation_exit_reason(output.status),
            bounded_continuation_summary(&output.summary),
            now_ms,
            now_ms.saturating_add(millis_of(CONTINUATION_RETENTION)),
        )
        .with_execution_ref(self.execution_lease.execution_id().get().to_string());
        if let Some(job_id) = self.job_id {
            record = record.with_job_ref(job_reference(job_id));
        }
        if let Some(path) = extract_result_path(&output.summary) {
            record = record.with_artifact_path(path);
        }
        if let Some(usage) = output.token_usage {
            record = record.with_tokens(
                usage.total_input_tokens,
                usage.output_tokens,
                usage.reasoning_output_tokens,
            );
        }
        if let Err(error) = sink.store.append_continuation(&record) {
            errors.push(format!("continuation={error}"));
        }
    }

    fn publish_terminal(
        &self,
        input: &RunSubagentExecutionInput,
        output: &RunSubagentExecutionOutput,
        authoritative_status: &AgentStatus,
        errors: &mut Vec<String>,
    ) {
        let Some((_, publisher)) = &self.lifecycle else {
            return;
        };
        let lifecycle = subagent_terminal_lifecycle(input, output, authoritative_status);
        if let Err(error) = publisher.publish_terminal(lifecycle) {
            errors.push(format!("lifecycle={error}"));
        }
    }

    fn cleanup(&self, input: &RunSubagentExecutionInput) {
        if let Some(cancel_token) = &self.cancel_token {
            remove_execution_cancel_token(
                &cancel_token.registry,
                &input.task_path,
                &cancel_token.execution_token,
            );
        }
        if let Some((registry, publisher)) = &self.lifecycle {
            registry.remove(&input.task_path, publisher);
        }
    }
}

fn remove_execution_cancel_token(
    cancel_tokens: &Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>,
    task_path: &AgentPath,
    execution_token: &RunCancelToken,
) {
    let mut cancel_tokens = cancel_tokens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cancel_tokens
        .get(task_path)
        .is_some_and(|current| current.same_execution(execution_token))
    {
        cancel_tokens.remove(task_path);
    }
}

fn sync_completed_subagent_graph(
    control: &AgentControlRuntime,
    store: &dyn AgentGraphStore,
    task_path: &AgentPath,
    completed_at_ms: u64,
) -> AxonResult<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| AxonError::runtime_failure("subagent_graph:runtime"))?
        .block_on(AgentControlGraphSyncRuntime.sync_completed_agent(
            control,
            store,
            task_path,
            completed_at_ms,
        ))
        .map(|_| ())
}

/// 子代理终态到 continuation 终态标签的映射；与通知载荷使用同一个词。
const fn continuation_terminal_kind(status: RunSubagentExecutionStatus) -> &'static str {
    match status {
        RunSubagentExecutionStatus::Completed => "completed",
        RunSubagentExecutionStatus::Refused => "refusal",
        RunSubagentExecutionStatus::Interrupted => "killed",
        RunSubagentExecutionStatus::Errored => "failed",
    }
}

/// 子代理终态到退出原因的映射；复用作业终态常量，避免两处措辞漂移。
const fn continuation_exit_reason(status: RunSubagentExecutionStatus) -> &'static str {
    match status {
        RunSubagentExecutionStatus::Completed => JOB_REASON_SUBAGENT_COMPLETED,
        RunSubagentExecutionStatus::Refused => JOB_REASON_SUBAGENT_REFUSED,
        RunSubagentExecutionStatus::Interrupted => JOB_REASON_SUBAGENT_INTERRUPTED,
        RunSubagentExecutionStatus::Errored => JOB_REASON_SUBAGENT_ERRORED,
    }
}

/// 子代理终态到临时 Job 终态的映射（合同 §5.2.3 的六类投影）。
///
/// 单列成纯函数是为了让两条判据可以被直接断言，而不必搭起整个作业注册表：
/// ①预算耗尽落在失败类但用自己的固定措辞；
/// ②拒绝落在拒绝类，且退出原因明说「任务未被接受」。
fn job_terminal_of(output: &RunSubagentExecutionOutput) -> (JobTerminalKind, String) {
    match output.status {
        RunSubagentExecutionStatus::Completed if output.budget_exhausted => (
            JobTerminalKind::Failed,
            JOB_REASON_BUDGET_EXHAUSTED.to_owned(),
        ),
        RunSubagentExecutionStatus::Completed => (
            JobTerminalKind::Completed,
            JOB_REASON_SUBAGENT_COMPLETED.to_owned(),
        ),
        RunSubagentExecutionStatus::Refused => (
            JobTerminalKind::Refusal,
            format!(
                "{JOB_REASON_SUBAGENT_REFUSED_PREFIX}{}",
                output.summary.trim()
            ),
        ),
        RunSubagentExecutionStatus::Interrupted => (
            JobTerminalKind::Killed,
            JOB_REASON_SUBAGENT_INTERRUPTED.to_owned(),
        ),
        RunSubagentExecutionStatus::Errored => (
            JobTerminalKind::Failed,
            JOB_REASON_SUBAGENT_ERRORED.to_owned(),
        ),
    }
}

/// 结束语：只有正常结束的子代理才可能留下 assistant 文本块。
///
/// 其余终态的 `summary` 承载的是拒绝、中断或错误诊断，不是子代理要说的话；
/// 把它当作结束语会让父代理误以为那是子代理的结论。
fn final_message_of(output: &RunSubagentExecutionOutput) -> Option<String> {
    if output.status != RunSubagentExecutionStatus::Completed {
        return None;
    }
    let message = strip_result_path(&output.summary);
    let message = message.trim();
    (!message.is_empty()).then(|| message.to_owned())
}

/// continuation 的摘要正文：去掉归档路径行后有界。
///
/// 服从 §4 不变量 6：摘要有界，完整结果只经 `artifact_path` 引用，不整段进入父上下文。
fn bounded_continuation_summary(summary: &str) -> String {
    strip_result_path(summary)
        .chars()
        .take(CONTINUATION_SUMMARY_MAX_CHARS)
        .collect()
}

pub(super) fn log_completion_failure(task_path: &AgentPath, error: &impl std::fmt::Debug) {
    axon_logger::warning(&format!(
        "[{COMPLETION_ERROR_PREFIX}] task_path={task_path} error={error:?}"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_commands::run_subagent_executor::SUBAGENT_RESULT_MARKER_TEXT;

    #[test]
    fn cancel_token_cleanup_only_removes_matching_execution() {
        let task_path = AgentPath::parse("/root/review").unwrap();
        let old_token = RunCancelToken::default();
        let current_token = RunCancelToken::default();
        let cancel_tokens = Arc::new(Mutex::new(HashMap::from([(
            task_path.clone(),
            current_token.clone(),
        )])));

        remove_execution_cancel_token(&cancel_tokens, &task_path, &old_token);
        assert!(
            cancel_tokens
                .lock()
                .unwrap()
                .get(&task_path)
                .is_some_and(|token| token.same_execution(&current_token))
        );

        remove_execution_cancel_token(&cancel_tokens, &task_path, &current_token);
        assert!(!cancel_tokens.lock().unwrap().contains_key(&task_path));
    }

    fn output(status: RunSubagentExecutionStatus, summary: &str) -> RunSubagentExecutionOutput {
        RunSubagentExecutionOutput {
            status,
            summary: summary.to_owned(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        }
    }

    /// 拒绝必须落在拒绝终态，且退出原因明说「任务未被接受」。
    #[test]
    fn refusal_maps_to_refusal_terminal_with_its_own_reason() {
        let output = output(
            RunSubagentExecutionStatus::Refused,
            "agent_control:not_found",
        );
        let (kind, reason) = job_terminal_of(&output);

        assert_eq!(kind, JobTerminalKind::Refusal);
        assert!(
            reason.starts_with("任务未被接受：") && reason.contains("agent_control:not_found"),
            "拒绝原因必须自证是哪一道门挡下的：{reason}"
        );
        assert!(
            final_message_of(&output).is_none(),
            "被拒绝的任务没有结束语"
        );
    }

    /// 预算耗尽落在失败类，但退出原因必须说的是预算而不是普通出错。
    #[test]
    fn budget_exhausted_maps_to_failed_with_its_own_wording() {
        let mut output = output(RunSubagentExecutionStatus::Completed, "被截断的结论");
        output.budget_exhausted = true;

        let (kind, reason) = job_terminal_of(&output);
        assert_eq!(kind, JobTerminalKind::Failed);
        assert_eq!(reason, JOB_REASON_BUDGET_EXHAUSTED);
        assert_ne!(reason, JOB_REASON_SUBAGENT_ERRORED);
    }

    /// 正常结束与其余终态的退出原因必须与既有措辞一致，不因扩展而漂移。
    #[test]
    fn other_terminal_kinds_keep_their_reasons() {
        for (status, expected) in [
            (
                RunSubagentExecutionStatus::Completed,
                JobTerminalKind::Completed,
            ),
            (
                RunSubagentExecutionStatus::Interrupted,
                JobTerminalKind::Killed,
            ),
            (RunSubagentExecutionStatus::Errored, JobTerminalKind::Failed),
        ] {
            let (kind, _) = job_terminal_of(&output(status, "摘要"));
            assert_eq!(kind, expected, "{status:?} 的终态映射不得漂移");
        }
    }

    /// 结束语取归档路径行之前的正文；归档结果不把路径行当成子代理的话。
    #[test]
    fn final_message_excludes_the_archive_path_line() {
        let archived = format!("结论正文\n{}\n/tmp/result.md", SUBAGENT_RESULT_MARKER_TEXT);
        let output = output(RunSubagentExecutionStatus::Completed, &archived);

        assert_eq!(final_message_of(&output).as_deref(), Some("结论正文"));
    }

    /// 空白最终正文不得被当成结束语（父代理会看到固定替换句）。
    #[test]
    fn blank_final_message_is_not_a_final_message() {
        let output = output(RunSubagentExecutionStatus::Completed, "   \n ");
        assert!(final_message_of(&output).is_none());
    }
}
