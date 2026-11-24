//! 长任务 RuntimeCell 控制面。

use axon_domain::{
    AxonError, AxonResult, CancelReason, CellId, LongRunningJobId, PaneId, RuntimeCell,
    RuntimeCellSnapshot, RuntimeCellState,
};

/// 长任务进度最大值。
pub const LONG_RUNNING_PROGRESS_MAX: u8 = 100;

/// 长任务运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongRunningJobStatus {
    /// 正在运行。
    Running,
    /// 已暂停。
    Paused,
    /// 已完成。
    Completed,
    /// 已取消。
    Cancelled,
    /// 已失败。
    Failed,
}

/// 长任务句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LongRunningJobHandle {
    /// job ID。
    pub id: LongRunningJobId,
    /// cell ID。
    pub cell_id: CellId,
    /// pane ID。
    pub pane_id: PaneId,
    /// 创建时间戳，毫秒。
    pub created_at_ms: u64,
    /// 最后心跳时间戳，毫秒。
    pub last_heartbeat_ms: u64,
    /// 进度百分比。
    pub progress_percent: u8,
    /// 状态。
    pub status: LongRunningJobStatus,
    /// 中文摘要。
    pub summary: String,
    /// cell 快照。
    pub cell_snapshot: RuntimeCellSnapshot,
}

/// 长任务事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LongRunningJobEvent {
    /// job 已附着到 cell。
    Attached {
        /// job ID。
        job_id: LongRunningJobId,
        /// cell ID。
        cell_id: CellId,
    },
    /// job 心跳。
    Heartbeat {
        /// job ID。
        job_id: LongRunningJobId,
        /// 时间戳，毫秒。
        timestamp_ms: u64,
    },
    /// job 进度更新。
    ProgressUpdated {
        /// job ID。
        job_id: LongRunningJobId,
        /// 进度百分比。
        progress_percent: u8,
    },
    /// job 已暂停。
    Paused {
        /// job ID。
        job_id: LongRunningJobId,
    },
    /// job 已恢复。
    Resumed {
        /// job ID。
        job_id: LongRunningJobId,
    },
    /// job 已取消。
    Cancelled {
        /// job ID。
        job_id: LongRunningJobId,
    },
    /// job 已完成。
    Completed {
        /// job ID。
        job_id: LongRunningJobId,
    },
    /// job 已失败。
    Failed {
        /// job ID。
        job_id: LongRunningJobId,
    },
}

/// 长任务控制面 runtime。
#[derive(Debug, Default, Clone)]
pub struct LongRunningCellRuntime {
    next_job_id: u64,
    jobs: Vec<LongRunningJobHandle>,
    events: Vec<LongRunningJobEvent>,
}

impl LongRunningCellRuntime {
    /// 创建空 runtime。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_job_id: 0,
            jobs: Vec::new(),
            events: Vec::new(),
        }
    }

    /// 附着一个正在运行的 cell 为长任务。
    pub fn attach_running_cell(
        &mut self,
        cell: &RuntimeCell,
        timestamp_ms: u64,
        summary: impl Into<String>,
    ) -> AxonResult<LongRunningJobId> {
        if !matches!(
            cell.state,
            RuntimeCellState::Running | RuntimeCellState::WaitingTool(_)
        ) {
            return Err(AxonError::cell_invalid_transition(
                "只有 Running/WaitingTool cell 能附着长任务",
            ));
        }
        if self.jobs.iter().any(|job| job.cell_id == cell.id) {
            return Err(AxonError::cell_invalid_transition(
                "RuntimeCell 已附着长任务",
            ));
        }

        self.next_job_id += 1;
        let job_id = LongRunningJobId(self.next_job_id);
        self.jobs.push(LongRunningJobHandle {
            id: job_id,
            cell_id: cell.id,
            pane_id: cell.pane_id,
            created_at_ms: timestamp_ms,
            last_heartbeat_ms: timestamp_ms,
            progress_percent: 0,
            status: LongRunningJobStatus::Running,
            summary: summary.into(),
            cell_snapshot: cell.snapshot(),
        });
        self.events.push(LongRunningJobEvent::Attached {
            job_id,
            cell_id: cell.id,
        });
        Ok(job_id)
    }

    /// 记录长任务心跳。
    pub fn heartbeat(&mut self, job_id: LongRunningJobId, timestamp_ms: u64) -> AxonResult<()> {
        let job = self.find_mut(job_id)?;
        ensure_job_active(job)?;
        job.last_heartbeat_ms = timestamp_ms;
        self.events.push(LongRunningJobEvent::Heartbeat {
            job_id,
            timestamp_ms,
        });
        Ok(())
    }

    /// 更新长任务进度。
    pub fn update_progress(
        &mut self,
        job_id: LongRunningJobId,
        progress_percent: u8,
        summary: impl Into<String>,
    ) -> AxonResult<()> {
        if progress_percent > LONG_RUNNING_PROGRESS_MAX {
            return Err(AxonError::runtime_failure("长任务进度不能超过 100"));
        }
        let job = self.find_mut(job_id)?;
        ensure_job_active(job)?;
        job.progress_percent = progress_percent;
        job.summary = summary.into();
        self.events.push(LongRunningJobEvent::ProgressUpdated {
            job_id,
            progress_percent,
        });
        Ok(())
    }

    /// 暂停长任务，并同步暂停 cell。
    pub fn pause(&mut self, job_id: LongRunningJobId, cell: &mut RuntimeCell) -> AxonResult<()> {
        let job = self.find_mut(job_id)?;
        ensure_job_matches_cell(job, cell.id)?;
        ensure_job_active(job)?;
        cell.pause()?;
        job.status = LongRunningJobStatus::Paused;
        job.cell_snapshot = cell.snapshot();
        self.events.push(LongRunningJobEvent::Paused { job_id });
        Ok(())
    }

    /// 恢复长任务，并同步恢复 cell。
    pub fn resume(&mut self, job_id: LongRunningJobId, cell: &mut RuntimeCell) -> AxonResult<()> {
        let job = self.find_mut(job_id)?;
        ensure_job_matches_cell(job, cell.id)?;
        if job.status != LongRunningJobStatus::Paused {
            return Err(AxonError::cell_invalid_transition(
                "只有 Paused 长任务能恢复",
            ));
        }
        cell.resume()?;
        job.status = LongRunningJobStatus::Running;
        job.cell_snapshot = cell.snapshot();
        self.events.push(LongRunningJobEvent::Resumed { job_id });
        Ok(())
    }

    /// 取消长任务，并同步取消 cell。
    pub fn cancel(
        &mut self,
        job_id: LongRunningJobId,
        cell: &mut RuntimeCell,
        reason: impl Into<String>,
    ) -> AxonResult<()> {
        let job = self.find_mut(job_id)?;
        ensure_job_matches_cell(job, cell.id)?;
        ensure_job_not_final(job)?;
        cell.cancel(CancelReason {
            message: reason.into(),
        })?;
        job.status = LongRunningJobStatus::Cancelled;
        job.cell_snapshot = cell.snapshot();
        self.events.push(LongRunningJobEvent::Cancelled { job_id });
        Ok(())
    }

    /// 标记长任务完成。
    pub fn complete(&mut self, job_id: LongRunningJobId, cell: &RuntimeCell) -> AxonResult<()> {
        let job = self.find_mut(job_id)?;
        ensure_job_matches_cell(job, cell.id)?;
        if !matches!(cell.state, RuntimeCellState::Completed(_)) {
            return Err(AxonError::cell_invalid_transition(
                "只有 Completed RuntimeCell 能完成长任务",
            ));
        }
        ensure_job_not_final(job)?;
        job.status = LongRunningJobStatus::Completed;
        job.progress_percent = LONG_RUNNING_PROGRESS_MAX;
        job.cell_snapshot = cell.snapshot();
        self.events.push(LongRunningJobEvent::Completed { job_id });
        Ok(())
    }

    /// 标记长任务失败。
    pub fn fail(&mut self, job_id: LongRunningJobId, cell: &RuntimeCell) -> AxonResult<()> {
        let job = self.find_mut(job_id)?;
        ensure_job_matches_cell(job, cell.id)?;
        if !matches!(cell.state, RuntimeCellState::Failed(_)) {
            return Err(AxonError::cell_invalid_transition(
                "只有 Failed RuntimeCell 能失败长任务",
            ));
        }
        ensure_job_not_final(job)?;
        job.status = LongRunningJobStatus::Failed;
        job.cell_snapshot = cell.snapshot();
        self.events.push(LongRunningJobEvent::Failed { job_id });
        Ok(())
    }

    /// 查询 job。
    #[must_use]
    pub fn job(&self, job_id: LongRunningJobId) -> Option<&LongRunningJobHandle> {
        self.jobs.iter().find(|job| job.id == job_id)
    }

    /// 列出 job。
    #[must_use]
    pub fn jobs(&self) -> &[LongRunningJobHandle] {
        &self.jobs
    }

    /// 列出事件。
    #[must_use]
    pub fn events(&self) -> &[LongRunningJobEvent] {
        &self.events
    }

    fn find_mut(&mut self, job_id: LongRunningJobId) -> AxonResult<&mut LongRunningJobHandle> {
        self.jobs
            .iter_mut()
            .find(|job| job.id == job_id)
            .ok_or(AxonError::cell_invalid_transition("长任务不存在"))
    }
}

fn ensure_job_matches_cell(job: &LongRunningJobHandle, cell_id: CellId) -> AxonResult<()> {
    if job.cell_id == cell_id {
        Ok(())
    } else {
        Err(AxonError::cell_invalid_transition(
            "长任务与 RuntimeCell 不匹配",
        ))
    }
}

fn ensure_job_active(job: &LongRunningJobHandle) -> AxonResult<()> {
    if matches!(job.status, LongRunningJobStatus::Running) {
        Ok(())
    } else {
        Err(AxonError::cell_invalid_transition("长任务当前不可更新"))
    }
}

fn ensure_job_not_final(job: &LongRunningJobHandle) -> AxonResult<()> {
    if matches!(
        job.status,
        LongRunningJobStatus::Completed
            | LongRunningJobStatus::Cancelled
            | LongRunningJobStatus::Failed
    ) {
        Err(AxonError::cell_invalid_transition("终态长任务不能再次变更"))
    } else {
        Ok(())
    }
}
