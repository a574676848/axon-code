use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 当前通用 Cron 持久化模型版本。
pub const CRON_SCHEMA_VERSION: u32 = 1;

/// Cron 任务生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronJobStatus {
    /// 等待调度。
    Active,
    /// 已暂停。
    Paused,
    /// 已移除。
    Removed,
    /// 一次性任务已完成。
    Completed,
}

impl CronJobStatus {
    /// 返回稳定的持久化状态名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Removed => "removed",
            Self::Completed => "completed",
        }
    }
}

/// Cron 投递 adapter 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronDeliveryAdapter {
    /// 不产生外部投递，用于 Axon 内建维护任务。
    None,
    /// 飞书消息投递。
    Feishu,
}

/// 平台无关的 Cron 投递路由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronDeliveryTarget {
    /// 负责投递的 adapter。
    pub adapter: CronDeliveryAdapter,
    /// Axon 会话命名空间。
    pub session_id: u64,
    /// adapter 内部的接收目标标识。
    pub recipient_id: String,
    /// adapter 内部的可选线程标识。
    pub thread_id: Option<String>,
}

impl CronDeliveryTarget {
    /// 创建不产生外部投递的路由。
    #[must_use]
    pub const fn none() -> Self {
        Self {
            adapter: CronDeliveryAdapter::None,
            session_id: 0,
            recipient_id: String::new(),
            thread_id: None,
        }
    }
}

/// 用户级 Cron 任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    /// 持久化模型版本。
    pub schema_version: u32,
    /// 任务标识。
    pub job_id: String,
    /// 任务名称。
    pub name: String,
    /// 原始计划表达式。
    pub schedule: String,
    /// Agent prompt。
    pub prompt: String,
    /// 显式加载的 Skill 名称。
    pub skills: Vec<String>,
    /// 投递路由。
    pub delivery: CronDeliveryTarget,
    /// 可选工作目录文本，由外层 adapter 解析。
    pub workdir: Option<String>,
    /// 是否跳过 Agent，仅运行脚本。
    pub no_agent: bool,
    /// 可选脚本正文。
    pub script: Option<String>,
    /// 生命周期状态。
    pub status: CronJobStatus,
    /// 乐观并发版本。
    pub revision: u64,
    /// 下次运行时间戳。
    pub next_run_at_ms: Option<u64>,
    /// 上次运行时间戳。
    pub last_run_at_ms: Option<u64>,
    /// 创建时间戳。
    pub created_at_ms: u64,
    /// 更新时间戳。
    pub updated_at_ms: u64,
}

/// 创建 Cron 任务所需的领域输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronJobInput {
    /// 任务标识。
    pub job_id: String,
    /// 任务名称。
    pub name: String,
    /// 计划表达式。
    pub schedule: String,
    /// Agent prompt。
    pub prompt: String,
    /// 显式 Skill 名称。
    pub skills: Vec<String>,
    /// 投递路由。
    pub delivery: CronDeliveryTarget,
    /// 可选工作目录文本。
    pub workdir: Option<String>,
    /// 是否只运行脚本。
    pub no_agent: bool,
    /// 可选脚本正文。
    pub script: Option<String>,
}

/// 更新 Cron 任务所需的领域输入。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CronJobUpdate {
    /// 新任务名称。
    pub name: Option<String>,
    /// 新计划表达式。
    pub schedule: Option<String>,
    /// 新 Agent prompt。
    pub prompt: Option<String>,
    /// 新 Skill 集合。
    pub skills: Option<Vec<String>>,
    /// 新接收目标标识。
    pub recipient_id: Option<String>,
    /// 新线程标识。
    pub thread_id: Option<String>,
    /// 是否替换线程标识。
    pub replace_thread: bool,
    /// 新工作目录文本。
    pub workdir: Option<String>,
    /// 是否替换工作目录。
    pub replace_workdir: bool,
    /// 是否只运行脚本。
    pub no_agent: Option<bool>,
    /// 新脚本正文。
    pub script: Option<String>,
    /// 是否替换脚本正文。
    pub replace_script: bool,
}

/// 单次 Cron 执行记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronRunRecord {
    /// 持久化模型版本。
    pub schema_version: u32,
    /// 运行标识。
    pub run_id: String,
    /// 任务标识。
    pub job_id: String,
    /// 执行时任务版本。
    pub job_revision: u64,
    /// 开始时间戳。
    pub started_at_ms: u64,
    /// 完成时间戳。
    pub finished_at_ms: u64,
    /// 是否成功。
    pub success: bool,
    /// 是否执行了 Agent。
    pub agent_executed: bool,
    /// 是否已将结果加入投递队列。
    pub delivery_enqueued: bool,
    /// 脚本标准输出是否为空。
    pub stdout_empty: bool,
    /// adapter 返回的投递标识。
    pub dispatch_id: Option<String>,
    /// 有界错误摘要。
    pub error_summary: Option<String>,
}

/// Cron 领域输入违反不可变约束。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CronValidationError {
    /// 任务标识为空。
    #[error("cron 任务 ID 不能为空")]
    EmptyJobId,
    /// 任务名称为空。
    #[error("cron 任务名称不能为空")]
    EmptyName,
    /// 计划表达式为空。
    #[error("cron schedule 不能为空")]
    EmptySchedule,
    /// Agent 任务缺少 prompt。
    #[error("Agent cron prompt 不能为空")]
    EmptyPrompt,
    /// 投递目标为空。
    #[error("cron 投递目标不能为空")]
    EmptyRecipient,
    /// no-agent 任务缺少脚本。
    #[error("no-agent cron 必须提供 script")]
    MissingScript,
}

impl CronJob {
    /// 创建满足领域约束的任务。
    pub fn new(
        input: CronJobInput,
        now_ms: u64,
        next_run_at_ms: Option<u64>,
    ) -> Result<Self, CronValidationError> {
        let job = Self {
            schema_version: CRON_SCHEMA_VERSION,
            job_id: input.job_id,
            name: input.name,
            schedule: input.schedule,
            prompt: input.prompt,
            skills: input.skills,
            delivery: input.delivery,
            workdir: input.workdir,
            no_agent: input.no_agent,
            script: input.script,
            status: CronJobStatus::Active,
            revision: 1,
            next_run_at_ms,
            last_run_at_ms: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        job.validate()?;
        Ok(job)
    }

    /// 生成更新后的任务版本。
    pub fn with_update(
        &self,
        update: CronJobUpdate,
        now_ms: u64,
        next_run_at_ms: Option<u64>,
    ) -> Result<Self, CronValidationError> {
        let mut next = self.clone();
        if let Some(name) = update.name {
            next.name = name;
        }
        if let Some(schedule) = update.schedule {
            next.schedule = schedule;
        }
        if let Some(prompt) = update.prompt {
            next.prompt = prompt;
        }
        if let Some(skills) = update.skills {
            next.skills = skills;
        }
        if let Some(recipient_id) = update.recipient_id {
            next.delivery.recipient_id = recipient_id;
        }
        if update.replace_thread {
            next.delivery.thread_id = update.thread_id;
        }
        if update.replace_workdir {
            next.workdir = update.workdir;
        }
        if let Some(no_agent) = update.no_agent {
            next.no_agent = no_agent;
        }
        if update.replace_script {
            next.script = update.script;
        }
        next.validate()?;
        next.revision = next.revision.saturating_add(1);
        next.updated_at_ms = now_ms;
        next.next_run_at_ms = next_run_at_ms;
        Ok(next)
    }

    /// 生成状态已更新的任务版本。
    #[must_use]
    pub fn with_status(&self, status: CronJobStatus, now_ms: u64) -> Self {
        let mut next = self.clone();
        next.revision = next.revision.saturating_add(1);
        next.status = status;
        next.updated_at_ms = now_ms;
        next
    }

    /// 生成一次执行完成后的任务版本。
    #[must_use]
    pub fn after_run(&self, now_ms: u64, next_run_at_ms: Option<u64>, one_shot: bool) -> Self {
        let mut next = self.clone();
        next.revision = next.revision.saturating_add(1);
        next.updated_at_ms = now_ms;
        next.last_run_at_ms = Some(now_ms);
        next.next_run_at_ms = next_run_at_ms;
        if one_shot {
            next.status = CronJobStatus::Completed;
        }
        next
    }

    fn validate(&self) -> Result<(), CronValidationError> {
        if self.job_id.trim().is_empty() {
            return Err(CronValidationError::EmptyJobId);
        }
        if self.name.trim().is_empty() {
            return Err(CronValidationError::EmptyName);
        }
        if self.schedule.trim().is_empty() {
            return Err(CronValidationError::EmptySchedule);
        }
        if self.prompt.trim().is_empty() && !self.no_agent {
            return Err(CronValidationError::EmptyPrompt);
        }
        if self.delivery.adapter != CronDeliveryAdapter::None
            && self.delivery.recipient_id.trim().is_empty()
        {
            return Err(CronValidationError::EmptyRecipient);
        }
        if self.no_agent
            && self
                .script
                .as_deref()
                .is_none_or(|script| script.trim().is_empty())
        {
            return Err(CronValidationError::MissingScript);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_job_transition_preserves_generic_delivery_route() {
        let job = CronJob::new(test_input(), 10, Some(20)).expect("合法任务应可创建");
        let updated = job
            .with_update(
                CronJobUpdate {
                    recipient_id: Some("oc_next".to_owned()),
                    ..CronJobUpdate::default()
                },
                30,
                Some(40),
            )
            .expect("合法更新应成功");
        let completed = updated.after_run(40, None, true);

        assert_eq!(updated.delivery.adapter, CronDeliveryAdapter::Feishu);
        assert_eq!(updated.delivery.recipient_id, "oc_next");
        assert_eq!(completed.status, CronJobStatus::Completed);
        assert_eq!(completed.last_run_at_ms, Some(40));
    }

    #[test]
    fn no_agent_job_requires_script_at_domain_boundary() {
        let mut input = test_input();
        input.prompt.clear();
        input.no_agent = true;

        assert_eq!(
            CronJob::new(input, 10, Some(20)),
            Err(CronValidationError::MissingScript)
        );
    }

    fn test_input() -> CronJobInput {
        CronJobInput {
            job_id: "cron-test".to_owned(),
            name: "测试".to_owned(),
            schedule: "interval:1h".to_owned(),
            prompt: "执行测试".to_owned(),
            skills: Vec::new(),
            delivery: CronDeliveryTarget {
                adapter: CronDeliveryAdapter::Feishu,
                session_id: 7,
                recipient_id: "oc_test".to_owned(),
                thread_id: None,
            },
            workdir: None,
            no_agent: false,
            script: None,
        }
    }
}
