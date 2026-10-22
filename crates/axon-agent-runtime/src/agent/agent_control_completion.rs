//! 子代理完成通知。
//!
//! 子代理进入 final 状态后，控制面向父代理 mailbox 投递非触发消息。
//! 这对齐 Codex MultiAgentV2 completion watcher：它只投递事实、不自动开启父 turn；
//! 父 turn 的继续由临时 Job 的完成通知通道负责。

use axon_domain::{AxonError, AxonResult};

use crate::agent_communication::AgentStatus;
use crate::agent_control_execution::AgentExecutionLease;
use crate::agent_control_mailbox::{AgentMessageRequest, AgentMessageTrigger};
use crate::agent_control_runtime::{AgentControlRuntime, AgentPath};
use crate::agent_mailbox_persistence::AgentMailboxMessageKind;

/// 子代理完成通知已投递。
pub const AGENT_CONTROL_REASON_COMPLETION_NOTIFIED: &str = "agent_control:completion_notified";
/// 子代理完成状态无需投递。
pub const AGENT_CONTROL_REASON_COMPLETION_NOTIFICATION_SKIPPED: &str =
    "agent_control:completion_notification_skipped";
/// completion 只接受终态。
pub const AGENT_CONTROL_REASON_COMPLETION_STATUS_NOT_FINAL: &str =
    "agent_control:completion_status_not_final";
/// 迟到 completion 不属于当前执行代际。
pub const AGENT_CONTROL_REASON_STALE_EXECUTION: &str = "agent_control:stale_execution";
/// Codex MultiAgentV2 使用的完成消息类型标记。
pub const AGENT_COMPLETION_MESSAGE_TYPE: &str = "FINAL_ANSWER";

/// 子代理完成通知报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCompletionReport {
    /// 完成的子代理路径。
    pub agent: AgentPath,
    /// 父代理路径。
    pub parent: Option<AgentPath>,
    /// 子代理 final 状态。
    pub status: AgentStatus,
    /// completion 是否应用于当前执行代际。
    pub execution_applied: bool,
    /// 是否投递了 mailbox 通知。
    pub notified: bool,
    /// 投递后的父 mailbox 消息数。
    pub queued_count: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

impl AgentControlRuntime {
    /// 完成指定执行代际，并向父代理 mailbox 投递非触发完成通知。
    pub fn complete_execution(
        &mut self,
        lease: &AgentExecutionLease,
        status: AgentStatus,
        completed_at_ms: u64,
    ) -> AxonResult<AgentCompletionReport> {
        if !is_final_status(&status) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_COMPLETION_STATUS_NOT_FINAL,
            ));
        }
        let path = lease.agent();
        if !self.is_current_execution(lease) {
            return Ok(stale_execution(path.clone(), self.status(path)));
        }
        self.running_turns.remove(path);
        let current_status = self.status(path);
        if is_final_status(&current_status) {
            return Ok(completion_skipped_with_parent(
                path.clone(),
                path.parent(),
                current_status,
                true,
            ));
        }
        let status = self.update_status(path, status)?;
        let Some(parent) = path.parent() else {
            return Ok(completion_skipped(path.clone(), status, true));
        };
        if !is_final_status(&status) || !self.has_agent(&parent) {
            return Ok(completion_skipped_with_parent(
                path.clone(),
                Some(parent),
                status,
                true,
            ));
        }
        let message = completion_message(path, &status);
        let report = self.send_message(AgentMessageRequest {
            author: path.clone(),
            recipient: parent.clone(),
            content: message,
            trigger: AgentMessageTrigger::QueueOnly,
            created_at_ms: completed_at_ms,
            kind: AgentMailboxMessageKind::Completion,
        })?;
        Ok(AgentCompletionReport {
            agent: path.clone(),
            parent: Some(parent),
            status,
            execution_applied: true,
            notified: true,
            queued_count: report.queued_count,
            reasons: vec![AGENT_CONTROL_REASON_COMPLETION_NOTIFIED.to_owned()],
        })
    }
}

fn is_final_status(status: &AgentStatus) -> bool {
    matches!(
        status,
        AgentStatus::Completed(_) | AgentStatus::Errored(_) | AgentStatus::Interrupted
    )
}

fn completion_skipped(
    agent: AgentPath,
    status: AgentStatus,
    execution_applied: bool,
) -> AgentCompletionReport {
    completion_skipped_with_parent(agent, None, status, execution_applied)
}

fn completion_skipped_with_parent(
    agent: AgentPath,
    parent: Option<AgentPath>,
    status: AgentStatus,
    execution_applied: bool,
) -> AgentCompletionReport {
    AgentCompletionReport {
        agent,
        parent,
        status,
        execution_applied,
        notified: false,
        queued_count: 0,
        reasons: vec![AGENT_CONTROL_REASON_COMPLETION_NOTIFICATION_SKIPPED.to_owned()],
    }
}

fn stale_execution(agent: AgentPath, status: AgentStatus) -> AgentCompletionReport {
    AgentCompletionReport {
        parent: agent.parent(),
        agent,
        status,
        execution_applied: false,
        notified: false,
        queued_count: 0,
        reasons: vec![AGENT_CONTROL_REASON_STALE_EXECUTION.to_owned()],
    }
}

fn completion_message(path: &AgentPath, status: &AgentStatus) -> String {
    let payload = match status {
        AgentStatus::Completed(Some(summary)) => summary.clone(),
        AgentStatus::Completed(None) => String::new(),
        AgentStatus::Errored(message) => format!("子代理执行失败：{message}"),
        AgentStatus::Interrupted => "子代理已中断。".to_owned(),
        _ => format!("子代理状态更新：{:?}", status),
    };
    format!(
        "Message Type: {AGENT_COMPLETION_MESSAGE_TYPE}\nTask name: {}\nSender: {}\nPayload:\n{}",
        path, path, payload,
    )
}

#[cfg(test)]
mod tests {
    use crate::{AgentControlRuntime, AgentPath, AgentSpawnRequest};

    use super::*;

    fn spawn_request(parent_path: AgentPath, task_name: &str) -> AgentSpawnRequest {
        AgentSpawnRequest {
            parent_path,
            task_name: task_name.to_owned(),
            message: format!("处理 {task_name}"),
            nickname: None,
            role: None,
        }
    }

    #[test]
    fn complete_execution_notifies_parent_mailbox_without_triggering_turn() {
        let mut runtime = AgentControlRuntime::default();
        let review = runtime
            .spawn(spawn_request(AgentPath::root(), "review"))
            .unwrap();
        let lease = runtime.lease_execution(&review.path).unwrap();

        let report = runtime
            .complete_execution(&lease, AgentStatus::Completed(Some("done".to_owned())), 42)
            .unwrap();

        assert!(report.execution_applied);
        assert!(report.notified);
        assert_eq!(report.parent, Some(AgentPath::root()));
        assert_eq!(report.queued_count, 1);
        let snapshot = runtime.mailbox_snapshot(&AgentPath::root()).unwrap();
        assert!(snapshot.mailbox_activity);
        assert_eq!(snapshot.messages[0].author, review.path);
        assert_eq!(snapshot.messages[0].recipient, AgentPath::root());
        assert_eq!(
            snapshot.messages[0].content,
            "Message Type: FINAL_ANSWER\nTask name: /root/review\nSender: /root/review\nPayload:\ndone"
        );
        assert!(!snapshot.messages[0].trigger_turn);
    }

    #[test]
    fn complete_execution_rejects_non_final_without_releasing_execution() {
        let mut runtime = AgentControlRuntime::default();
        let review = runtime
            .spawn(spawn_request(AgentPath::root(), "review"))
            .unwrap();
        let lease = runtime.lease_execution(&review.path).unwrap();

        let error = runtime
            .complete_execution(&lease, AgentStatus::Running, 42)
            .unwrap_err();

        assert!(matches!(
            error,
            axon_domain::AxonError::Runtime(
                axon_domain::RuntimeError::InvalidState(reason),
            ) if reason == AGENT_CONTROL_REASON_COMPLETION_STATUS_NOT_FINAL
        ));
        assert!(runtime.is_current_execution(&lease));
        assert_eq!(runtime.active_execution_count(), 1);
        assert_eq!(runtime.status(&review.path), AgentStatus::Running);
        assert_eq!(runtime.mailbox_len(&AgentPath::root()), 0);
    }

    #[test]
    fn stale_completion_cannot_release_or_complete_new_execution() {
        let mut runtime = AgentControlRuntime::default();
        let review = runtime
            .spawn(spawn_request(AgentPath::root(), "review"))
            .unwrap();
        let old_lease = runtime.lease_execution(&review.path).unwrap();
        runtime.interrupt(&review.path).unwrap();
        let current_lease = runtime.lease_execution(&review.path).unwrap();

        let report = runtime
            .complete_execution(
                &old_lease,
                AgentStatus::Completed(Some("迟到结果".to_owned())),
                42,
            )
            .unwrap();

        assert!(!report.execution_applied);
        assert_eq!(report.status, AgentStatus::Running);
        assert!(runtime.is_current_execution(&current_lease));
        assert_eq!(runtime.active_execution_count(), 1);
        assert_eq!(runtime.mailbox_len(&AgentPath::root()), 0);
    }
}
