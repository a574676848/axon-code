//! 作业记录本体：状态、终态与完成通知所需的事实。
//!
//! 单独成文件只是为了让注册表保持可读：注册表负责身份、归属与生命周期顺序，
//! 单个作业「怎么记、怎么推进、能交出哪些事实」都在这里。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::producer::{JobExecution, JobProducerKind, JobTerminalState};
use super::registry::JobStatus;

/// 完成通知所需的作业事实；在持锁期间组装、在释放锁之后投递。
pub(super) struct JobNoticeFacts {
    pub(super) owner_id: String,
    pub(super) producer: JobProducerKind,
    pub(super) label: String,
    pub(super) terminal: JobTerminalState,
    pub(super) summary: Option<String>,
    pub(super) artifact_ref: PathBuf,
}

/// 作业终态记录。
pub(super) struct JobTerminal {
    pub(super) status: JobStatus,
    pub(super) exit_code: Option<i32>,
    pub(super) reason: Option<String>,
    pub(super) ended_at: Instant,
}

/// 单个后台作业。
pub(super) struct Job {
    pub(super) owner_id: String,
    pub(super) label: String,
    pub(super) producer: JobProducerKind,
    pub(super) started: Instant,
    /// 启动时刻的墙上时间；生命周期事件对外只认这个值。
    pub(super) started_at_ms: u64,
    pub(super) last_activity: Instant,
    pub(super) execution: Box<dyn JobExecution>,
    pub(super) terminal: Option<JobTerminal>,
}

impl Job {
    /// 推进作业状态：取消、超时、空闲超时与自然结束。
    ///
    /// 返回本次推进是否**首次**进入终态；调用方据此在释放作业锁之后再产出完成通知，
    /// 避免持锁回调。
    pub(super) fn poll(&mut self, now: Instant, idle_timeout: Duration) -> std::io::Result<bool> {
        if self.terminal.is_some() {
            return Ok(false);
        }
        let outcome = self.execution.poll(now, self.last_activity + idle_timeout);
        if outcome.is_err() && self.execution.terminal().is_none() {
            // 执行体自身推进失败时不留下半途状态：终止它并投影为失败终态。
            let _ = self.execution.terminate();
        }
        let terminal = self.execution.terminal();
        let mut newly_terminal = false;
        if let Some(terminal) = terminal {
            newly_terminal = self.mark_terminal(terminal);
        }
        outcome.map(|()| newly_terminal)
    }

    pub(super) fn mark_terminal(&mut self, state: JobTerminalState) -> bool {
        if self.terminal.is_some() {
            return false;
        }
        self.terminal = Some(JobTerminal {
            status: state.status,
            exit_code: state.exit_code,
            reason: state.reason,
            ended_at: Instant::now(),
        });
        true
    }

    /// 组装完成通知所需的全部事实；未终态时为 `None`。
    ///
    /// 结算正文与完整结果引用分开取：引用优先使用生产者给出的结果归档，
    /// 缺失时退回作业自身的输出归档：前者指向落盘的完整结果，
    /// 后者指向流式输出归档，两者都不参与正文截断（§4 不变量 6）。
    pub(super) fn notice_facts(&self) -> Option<JobNoticeFacts> {
        let terminal = self.terminal.as_ref()?;
        let settlement = self.execution.notice_settlement();
        Some(JobNoticeFacts {
            owner_id: self.owner_id.clone(),
            producer: self.producer,
            label: self.label.clone(),
            terminal: JobTerminalState {
                status: terminal.status,
                exit_code: terminal.exit_code,
                reason: terminal.reason.clone(),
            },
            summary: settlement
                .as_ref()
                .and_then(|settlement| settlement.final_message.clone()),
            artifact_ref: settlement
                .as_ref()
                .and_then(|settlement| settlement.final_artifact_ref.clone())
                .map_or_else(|| self.execution.artifact_path(), PathBuf::from),
        })
    }

    pub(super) fn status(&self) -> JobStatus {
        self.terminal
            .as_ref()
            .map_or(JobStatus::Running, |terminal| terminal.status)
    }

    pub(super) fn exit_code(&self) -> Option<i32> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.exit_code)
    }
}
