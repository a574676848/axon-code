//! 远控线程控制面规划。
//!
//! 该模块只把远控线程操作收敛为本机可审计命令，不启动网络。

use axon_domain::{AxonError, AxonResult, RemoteAction, RemoteCapabilityPolicy, SessionId};

/// 远控线程标题最大字符数。
pub const REMOTE_THREAD_TITLE_MAX_CHARS: usize = 120;
/// 远控线程列表最大返回数量。
pub const REMOTE_THREAD_LIST_LIMIT: usize = 20;
/// 远控线程控制命令已规划。
pub const REMOTE_THREAD_REASON_COMMAND_PLANNED: &str = "remote_thread:command_planned";
/// 远控线程控制保持本机执行。
pub const REMOTE_THREAD_REASON_LOCAL_EXECUTION_REQUIRED: &str =
    "remote_thread:local_execution_required";

/// 远控线程状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteThreadState {
    /// 空闲，可直接接收后续指令。
    Idle,
    /// 正在运行，后续普通指令应排队。
    Busy,
    /// 已完成。
    Completed,
    /// 已阻断。
    Blocked,
}

/// 远控可见线程摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteThreadSummary {
    /// 线程所属 session。
    pub session_id: SessionId,
    /// 线程标题。
    pub title: String,
    /// 线程状态。
    pub state: RemoteThreadState,
    /// 排队消息数。
    pub queued_message_count: usize,
}

/// 远控线程控制动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteThreadControlAction {
    /// 启动新线程。
    StartNew {
        /// 初始指令有界预览。
        bounded_instruction: String,
        /// 可选标题。
        title: Option<String>,
    },
    /// 继续既有线程。
    ContinueExisting {
        /// 目标 session。
        session_id: SessionId,
        /// 后续指令有界预览。
        bounded_instruction: String,
        /// 忙碌时是否进入队列。
        queued: bool,
    },
    /// 列出线程。
    ListThreads {
        /// 返回上限。
        limit: usize,
    },
    /// 查询线程状态。
    QueryStatus {
        /// 目标 session。
        session_id: SessionId,
    },
}

/// 远控线程控制请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteThreadControlRequest {
    /// 请求动作。
    pub action: RemoteThreadControlAction,
    /// 当前远控可见线程快照。
    pub known_threads: Vec<RemoteThreadSummary>,
}

/// 本机 runtime 可执行的结构化线程命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteThreadCommand {
    /// 请求动作。
    pub action: RemoteThreadControlAction,
    /// 是否要求本机 runtime 执行。
    pub local_execution_required: bool,
    /// 是否允许远控切换 backend。
    pub remote_backend_switch_allowed: bool,
}

/// 远控线程控制报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteThreadControlReport {
    /// 结构化命令。
    pub command: RemoteThreadCommand,
    /// 线程状态摘要。
    pub threads: Vec<RemoteThreadSummary>,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控线程控制 runtime。
#[derive(Debug, Clone)]
pub struct RemoteThreadControlRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteThreadControlRuntime {
    /// 创建远控线程控制 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 规划远控线程控制命令。
    pub fn plan(
        &self,
        request: RemoteThreadControlRequest,
    ) -> AxonResult<RemoteThreadControlReport> {
        self.policy.ensure_allowed(RemoteAction::ControlThread)?;
        validate_action(&request.action, &request.known_threads)?;

        let threads = match &request.action {
            RemoteThreadControlAction::ListThreads { limit } => {
                bounded_threads(&request.known_threads, *limit)
            }
            RemoteThreadControlAction::QueryStatus { session_id } => request
                .known_threads
                .iter()
                .filter(|thread| thread.session_id == *session_id)
                .cloned()
                .collect(),
            RemoteThreadControlAction::StartNew { .. }
            | RemoteThreadControlAction::ContinueExisting { .. } => Vec::new(),
        };

        Ok(RemoteThreadControlReport {
            command: RemoteThreadCommand {
                action: request.action,
                local_execution_required: true,
                remote_backend_switch_allowed: false,
            },
            threads,
            reasons: vec![
                REMOTE_THREAD_REASON_COMMAND_PLANNED.to_owned(),
                REMOTE_THREAD_REASON_LOCAL_EXECUTION_REQUIRED.to_owned(),
            ],
        })
    }
}

fn validate_action(
    action: &RemoteThreadControlAction,
    known_threads: &[RemoteThreadSummary],
) -> AxonResult<()> {
    match action {
        RemoteThreadControlAction::StartNew {
            bounded_instruction,
            title,
        } => {
            validate_instruction(bounded_instruction)?;
            if let Some(title) = title {
                validate_bounded_text(title, REMOTE_THREAD_TITLE_MAX_CHARS)?;
            }
        }
        RemoteThreadControlAction::ContinueExisting {
            session_id,
            bounded_instruction,
            queued: _,
        } => {
            validate_instruction(bounded_instruction)?;
            ensure_known_thread(*session_id, known_threads)?;
        }
        RemoteThreadControlAction::ListThreads { limit } => {
            if *limit == 0 || *limit > REMOTE_THREAD_LIST_LIMIT {
                return Err(AxonError::runtime_failure("远控线程列表 limit 超出范围"));
            }
        }
        RemoteThreadControlAction::QueryStatus { session_id } => {
            ensure_known_thread(*session_id, known_threads)?;
        }
    }
    Ok(())
}

fn validate_instruction(value: &str) -> AxonResult<()> {
    if value.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    Ok(())
}

fn validate_bounded_text(value: &str, limit: usize) -> AxonResult<()> {
    let actual = value.chars().count();
    if value.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    if actual > limit {
        return Err(AxonError::InputTooLong { limit, actual });
    }
    Ok(())
}

fn ensure_known_thread(
    session_id: SessionId,
    known_threads: &[RemoteThreadSummary],
) -> AxonResult<()> {
    if known_threads
        .iter()
        .any(|thread| thread.session_id == session_id)
    {
        return Ok(());
    }
    Err(AxonError::runtime_failure("远控线程不存在"))
}

fn bounded_threads(threads: &[RemoteThreadSummary], limit: usize) -> Vec<RemoteThreadSummary> {
    threads.iter().take(limit).cloned().collect()
}
