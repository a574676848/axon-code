//! 子代理 lifecycle 事件构造。

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use axon_agent_runtime::{AgentPath, AgentStatus, JobTerminalKind};
use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecyclePhase, WireLifecycleStatus,
    WireSchemaVersion, WireSessionRef, WireTurnLifecycle,
};

use super::run_progress::{RunLifecycleStatusView, RunProgressEventBus, RunProgressWireEvent};
use super::run_subagent_executor::{
    RunSubagentExecutionInput, RunSubagentExecutionOutput, RunSubagentExecutionStatus,
};

pub(super) const SUBAGENT_LIFECYCLE_TITLE: &str = "子代理";
pub(super) const SUBAGENT_LIFECYCLE_KIND: &str = "subagent_activity";

const SUBAGENT_RUN_ID_PREFIX: &str = "subagent-run";
const SUBAGENT_PROJECTION_LOG_SCOPE: &str = "subagent_projection";
const SUBAGENT_INITIAL_SEQUENCE: u64 = 1;
static SUBAGENT_RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct SubagentLifecyclePublishState {
    next_sequence: u64,
    terminal: bool,
}

#[derive(Clone)]
pub(super) struct SubagentLifecyclePublisher {
    task_path: AgentPath,
    event_bus: Option<RunProgressEventBus>,
    session: WireSessionRef,
    state: Arc<Mutex<SubagentLifecyclePublishState>>,
}

impl SubagentLifecyclePublisher {
    fn new(event_bus: Option<RunProgressEventBus>, session_id: &str, task_path: AgentPath) -> Self {
        let run_id = format!(
            "{SUBAGENT_RUN_ID_PREFIX}-{}-{}",
            crate::tool_orchestrator::now_ms(),
            SUBAGENT_RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            task_path,
            event_bus,
            session: WireSessionRef {
                session_id: session_id.to_owned(),
                run_id: Some(run_id),
                parent_session_id: None,
            },
            state: Arc::new(Mutex::new(SubagentLifecyclePublishState {
                next_sequence: SUBAGENT_INITIAL_SEQUENCE,
                terminal: false,
            })),
        }
    }

    pub(super) fn run_id(&self) -> &str {
        self.session.run_id.as_deref().unwrap_or_default()
    }

    pub(super) fn publish(&self, view: RunLifecycleStatusView) -> Result<bool, String> {
        self.publish_event(view, false)
    }

    pub(super) fn publish_terminal(&self, view: RunLifecycleStatusView) -> Result<bool, String> {
        self.publish_event(view, true)
    }

    fn publish_event(&self, view: RunLifecycleStatusView, terminal: bool) -> Result<bool, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.terminal {
            return Ok(false);
        }
        if terminal {
            state.terminal = true;
        }
        let sequence = state.next_sequence;
        state.next_sequence = sequence.saturating_add(1);
        let Some(event_bus) = &self.event_bus else {
            return Ok(true);
        };
        let event_kind = view.phase.clone();
        let event = RunProgressWireEvent {
            event: WireEventEnvelope {
                schema_version: WireSchemaVersion::default(),
                event_id: WireCorrelationId(format!("{}:{sequence}", self.run_id())),
                correlation_id: WireCorrelationId(format!(
                    "{SUBAGENT_LIFECYCLE_KIND}:{}",
                    self.task_path
                )),
                session: self.session.clone(),
                sequence,
                recorded_at_ms: i64::try_from(crate::tool_orchestrator::now_ms())
                    .unwrap_or(i64::MAX),
                kind: WireEventKind::TurnLifecycle(lifecycle_from_view(view)),
            },
        };
        match event_bus(event) {
            Ok(()) => Ok(true),
            Err(error) => {
                if terminal {
                    state.terminal = false;
                }
                log_projection_failure(
                    &self.task_path,
                    self.run_id(),
                    sequence,
                    &event_kind,
                    &error,
                );
                Err(error)
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct SubagentLifecycleRegistry {
    event_bus: Option<RunProgressEventBus>,
    session_id: String,
    publishers: Arc<Mutex<HashMap<AgentPath, SubagentLifecyclePublisher>>>,
}

impl SubagentLifecycleRegistry {
    pub(super) fn new(event_bus: Option<RunProgressEventBus>, session_id: String) -> Self {
        Self {
            event_bus,
            session_id,
            publishers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn begin_execution(&self, task_path: &AgentPath) -> SubagentLifecyclePublisher {
        let publisher = SubagentLifecyclePublisher::new(
            self.event_bus.clone(),
            &self.session_id,
            task_path.clone(),
        );
        self.publishers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(task_path.clone(), publisher.clone());
        publisher
    }

    #[cfg(test)]
    pub(super) fn current(&self, task_path: &AgentPath) -> Option<SubagentLifecyclePublisher> {
        self.publishers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(task_path)
            .cloned()
    }

    pub(super) fn take_current(&self, task_path: &AgentPath) -> Option<SubagentLifecyclePublisher> {
        self.publishers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(task_path)
    }

    pub(super) fn remove(&self, task_path: &AgentPath, publisher: &SubagentLifecyclePublisher) {
        self.publishers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|path, current| path != task_path || current.run_id() != publisher.run_id());
    }
}

fn lifecycle_from_view(view: RunLifecycleStatusView) -> WireTurnLifecycle {
    WireTurnLifecycle {
        part_id: view.id,
        turn_id: view.kind.clone(),
        phase: WireLifecyclePhase::RegularTurn,
        status: view.status,
        title: view.title,
        kind: Some(view.kind),
        ui_phase: Some(view.phase),
        tool_name: view.tool_name,
        attempt_index: view
            .attempt_index
            .and_then(|value| u64::try_from(value).ok()),
        summary: Some(view.summary),
        duration_ms: view.duration_ms,
        started_at_ms: view.started_at_ms,
        total_tokens: view.total_tokens,
        cache_hit_rate_basis_points: view.cache_hit_rate_basis_points,
        warnings: Vec::new(),
    }
}

fn log_projection_failure(
    task_path: &AgentPath,
    run_id: &str,
    sequence: u64,
    event_kind: &str,
    error: &str,
) {
    axon_logger::warning(&format!(
        "[{SUBAGENT_PROJECTION_LOG_SCOPE}] task_path={task_path} run_id={run_id} sequence={sequence} event_kind={event_kind} error={error}"
    ));
}

pub(super) fn subagent_terminal_lifecycle(
    input: &RunSubagentExecutionInput,
    output: &RunSubagentExecutionOutput,
    authoritative_status: &AgentStatus,
) -> RunLifecycleStatusView {
    let task_path = &input.task_path;
    let (phase, status, summary) = match authoritative_status {
        AgentStatus::Completed(summary) => (
            "completed",
            WireLifecycleStatus::Completed,
            format!(
                "{task_path} 已完成：{}",
                summary.as_deref().unwrap_or(&output.summary)
            ),
        ),
        AgentStatus::Interrupted => (
            "interrupted",
            WireLifecycleStatus::Cancelled,
            format!("{task_path} 已中断：{}", output.summary),
        ),
        AgentStatus::Refused(reason) => (
            "refused",
            WireLifecycleStatus::Blocked,
            format!("{task_path} 的任务未被接受：{reason}"),
        ),
        AgentStatus::Errored(error) => (
            "errored",
            WireLifecycleStatus::Failed,
            format!("{task_path} 执行失败：{error}"),
        ),
        AgentStatus::Shutdown | AgentStatus::NotFound => (
            "closed",
            WireLifecycleStatus::Cancelled,
            format!("{task_path} 已关闭"),
        ),
        AgentStatus::PendingInit | AgentStatus::Running => match output.status {
            RunSubagentExecutionStatus::Completed => (
                "completed",
                WireLifecycleStatus::Completed,
                format!("{task_path} 已完成：{}", output.summary),
            ),
            RunSubagentExecutionStatus::Refused => (
                "refused",
                WireLifecycleStatus::Blocked,
                format!("{task_path} 的任务未被接受：{}", output.summary),
            ),
            RunSubagentExecutionStatus::Interrupted => (
                "interrupted",
                WireLifecycleStatus::Cancelled,
                format!("{task_path} 已中断：{}", output.summary),
            ),
            RunSubagentExecutionStatus::Errored => (
                "errored",
                WireLifecycleStatus::Failed,
                format!("{task_path} 执行失败：{}", output.summary),
            ),
        },
    };
    let duration_ms = crate::tool_orchestrator::now_ms().saturating_sub(input.started_at_ms);
    let mut view = RunLifecycleStatusView::with_id(
        subagent_lifecycle_part_id(task_path),
        SUBAGENT_LIFECYCLE_TITLE,
        SUBAGENT_LIFECYCLE_KIND,
        summary,
        phase,
        None,
    )
    .with_started_at_ms(input.started_at_ms)
    .with_duration_ms(duration_ms);
    view.status = status;
    if let Some(usage) = output.token_usage.filter(|usage| usage.has_tokens()) {
        view = view
            .with_total_tokens(usage.total_tokens())
            .with_cache_hit_rate_basis_points(usage.cache_hit_rate_basis_points());
    }
    if let Some(model) = output.model.as_deref() {
        view = view.with_tool_name(model);
    }
    view
}

pub(super) fn subagent_started_lifecycle(
    input: &RunSubagentExecutionInput,
    model: Option<&str>,
) -> RunLifecycleStatusView {
    let task_path = &input.task_path;
    let summary = format!("{task_path} 已启动：{}", input.message);
    let mut view = RunLifecycleStatusView::with_id(
        subagent_lifecycle_part_id(task_path),
        SUBAGENT_LIFECYCLE_TITLE,
        SUBAGENT_LIFECYCLE_KIND,
        summary,
        "started",
        None,
    )
    .with_started_at_ms(input.started_at_ms);
    view.status = WireLifecycleStatus::Started;
    if let Some(model) = model {
        view = view.with_tool_name(model);
    }
    view
}

/// 子代理的显式终态投影。
///
/// 六类退出（completed / refusal / failed / killed / timeout / unresponsive）共用**同一**结构：
/// 终态种类取自临时 Job 的统一终态枚举，投影只负责把它翻译成 wire 生命周期。
/// 这样 Job 终态、通知载荷与 TUI/Wire 快照三处看到的「结局」是同一个词。
pub(super) fn subagent_explicit_terminal_lifecycle(
    task_path: &AgentPath,
    terminal: JobTerminalKind,
) -> RunLifecycleStatusView {
    let (phase, status) = subagent_terminal_projection(terminal);
    let summary = format!("{task_path} {}", subagent_terminal_summary_suffix(terminal));
    let mut view = RunLifecycleStatusView::with_id(
        subagent_lifecycle_part_id(task_path),
        SUBAGENT_LIFECYCLE_TITLE,
        SUBAGENT_LIFECYCLE_KIND,
        summary,
        phase,
        None,
    );
    view.status = status;
    view
}

/// 统一终态到 wire 生命周期状态的唯一映射。
///
/// 「任务未被接受」映射为 `Blocked` 而不是 `Failed`：拒绝发生在执行之前，
/// 把它画成失败会让 TUI 与 wire 的读者以为子代理真的跑过一轮（合同 §5.2.3）。
fn subagent_terminal_projection(terminal: JobTerminalKind) -> (&'static str, WireLifecycleStatus) {
    match terminal {
        JobTerminalKind::Completed => ("completed", WireLifecycleStatus::Completed),
        JobTerminalKind::Refusal => ("refused", WireLifecycleStatus::Blocked),
        JobTerminalKind::Failed => ("errored", WireLifecycleStatus::Failed),
        JobTerminalKind::Killed => ("killed", WireLifecycleStatus::Cancelled),
        JobTerminalKind::Timeout => ("timeout", WireLifecycleStatus::Cancelled),
        JobTerminalKind::Unresponsive => ("unresponsive", WireLifecycleStatus::Cancelled),
    }
}

fn subagent_terminal_summary_suffix(terminal: JobTerminalKind) -> &'static str {
    match terminal {
        JobTerminalKind::Completed => "已完成",
        JobTerminalKind::Refusal => "的任务未被接受",
        JobTerminalKind::Failed => "执行失败",
        JobTerminalKind::Killed => "已被终止",
        JobTerminalKind::Timeout => "已因墙钟超时被终止",
        JobTerminalKind::Unresponsive => "已因空闲超时被终止",
    }
}

fn subagent_lifecycle_part_id(task_path: &AgentPath) -> String {
    let suffix = task_path.as_str().trim_start_matches('/').replace('/', ":");
    format!("lifecycle:{SUBAGENT_LIFECYCLE_KIND}:{suffix}")
}
