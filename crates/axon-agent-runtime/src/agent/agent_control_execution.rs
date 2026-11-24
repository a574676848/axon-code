//! 子代理真实执行占位治理。

use axon_domain::{AxonError, AxonResult};

use crate::agent_communication::AgentStatus;
use crate::agent_control_runtime::{
    AGENT_CONTROL_REASON_NOT_FOUND, AgentControlRuntime, AgentPath,
};

/// 子代理执行并发达到上限。
pub const AGENT_CONTROL_REASON_EXECUTION_LIMIT_REACHED: &str =
    "agent_control:execution_limit_reached";
/// 子代理已有运行中的 turn。
pub const AGENT_CONTROL_REASON_ALREADY_RUNNING: &str = "agent_control:already_running";
/// 子代理没有可中断的运行中 turn。
pub const AGENT_CONTROL_REASON_NO_ACTIVE_EXECUTION: &str = "agent_control:no_active_execution";
/// 子代理执行序号耗尽。
pub const AGENT_CONTROL_REASON_EXECUTION_ID_EXHAUSTED: &str =
    "agent_control:execution_id_exhausted";

/// 子代理执行代际。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentExecutionId(u64);

impl AgentExecutionId {
    /// 返回执行代际的稳定数值。
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 子代理执行占位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExecutionLease {
    agent: AgentPath,
    execution_id: AgentExecutionId,
}

impl AgentExecutionLease {
    /// 执行目标路径。
    #[must_use]
    pub const fn agent(&self) -> &AgentPath {
        &self.agent
    }

    /// 本次执行代际。
    #[must_use]
    pub const fn execution_id(&self) -> AgentExecutionId {
        self.execution_id
    }
}

/// 子代理中断报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInterruptReport {
    /// 中断前状态。
    pub previous_status: AgentStatus,
    /// 中断后状态。
    pub status: AgentStatus,
    /// 被中断的真实 turn 执行代际。
    pub execution_id: AgentExecutionId,
}

impl AgentControlRuntime {
    /// 为一次真实子代理执行获取占位。
    pub fn lease_execution(&mut self, path: &AgentPath) -> AxonResult<AgentExecutionLease> {
        if path.is_root() {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ));
        }
        if !self.agents.contains_key(path) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ));
        }
        if self.running_turns.contains_key(path) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_ALREADY_RUNNING,
            ));
        }
        if self.running_turns.len() >= self.config.max_active_turns {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_EXECUTION_LIMIT_REACHED,
            ));
        }
        let execution_id = AgentExecutionId(self.next_execution_id);
        self.next_execution_id = self.next_execution_id.checked_add(1).ok_or_else(|| {
            AxonError::runtime_failure(AGENT_CONTROL_REASON_EXECUTION_ID_EXHAUSTED)
        })?;
        self.update_status(path, AgentStatus::Running)?;
        self.running_turns.insert(path.clone(), execution_id);
        Ok(AgentExecutionLease {
            agent: path.clone(),
            execution_id,
        })
    }

    /// 判断执行占位是否仍是指定路径的当前代际。
    #[must_use]
    pub fn is_current_execution(&self, lease: &AgentExecutionLease) -> bool {
        self.running_turns
            .get(lease.agent())
            .is_some_and(|current| *current == lease.execution_id())
    }

    /// 中断子代理并释放其运行中执行占位。
    pub fn interrupt(&mut self, path: &AgentPath) -> AxonResult<AgentInterruptReport> {
        if path.is_root() {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ));
        }
        let previous_status = self.status(path);
        if previous_status == AgentStatus::NotFound {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ));
        }
        let execution_id = self.running_turns.remove(path).ok_or_else(|| {
            AxonError::runtime_invalid_state(AGENT_CONTROL_REASON_NO_ACTIVE_EXECUTION)
        })?;
        let status = self.update_status(path, AgentStatus::Interrupted)?;
        Ok(AgentInterruptReport {
            previous_status,
            status,
            execution_id,
        })
    }

    /// 当前正在执行真实 turn 的子代理数量。
    #[must_use]
    pub fn active_execution_count(&self) -> usize {
        self.running_turns.len()
    }

    /// 指定子代理是否已有运行中的真实 turn。
    #[must_use]
    pub fn has_active_execution(&self, path: &AgentPath) -> bool {
        self.running_turns.contains_key(path)
    }
}
