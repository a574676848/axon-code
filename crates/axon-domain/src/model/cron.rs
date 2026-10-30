use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 当前通用 Cron 持久化模型版本。
pub const CRON_SCHEMA_VERSION: u32 = 2;

/// 飞书投递渠道标识。
pub const FEISHU_CHANNEL: &str = "feishu";

/// Cron 自身主 turn 车道的 destination 前缀。
///
/// Cron 任务在自己的车道里执行，该车道没有人类消费者，因此不允许把唤醒目标指向它，
/// 否则每轮执行都会唤醒一轮新的 Agent 回合。
pub const CRON_LANE_DESTINATION_PREFIX: &str = "cron-";

/// Cron 完成后要唤醒的目标。
///
/// 产品定义：定时任务只在创建时显式声明目标时才产生任何投递；没有目标就只落执行
/// 事实。两类目标语义不同：`Feishu` 是正文直投，不产生 Agent 回合；`Session` 是主
/// turn 注入，会真实执行一轮，因此有 provider 成本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CronWakeTarget {
    /// 飞书群或单聊，载荷为 chat_id。
    Feishu(String),
    /// 复合车道主 turn 注入。
    Session {
        /// 会话表面渠道，例如 `desktop`。
        channel: String,
        /// 渠道内会话表面标识。
        destination: String,
    },
}

impl CronWakeTarget {
    /// 目标投递渠道。
    #[must_use]
    pub fn channel(&self) -> &str {
        match self {
            Self::Feishu(_) => FEISHU_CHANNEL,
            Self::Session { channel, .. } => channel,
        }
    }

    /// 渠道内的投递目标。
    #[must_use]
    pub fn destination(&self) -> &str {
        match self {
            Self::Feishu(chat_id) => chat_id,
            Self::Session { destination, .. } => destination,
        }
    }

    /// 目标是否要求注入一轮主 turn。
    #[must_use]
    pub const fn wakes_turn(&self) -> bool {
        matches!(self, Self::Session { .. })
    }

    /// 校验目标形态；返回中文原因便于直接呈现给调用方。
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Feishu(chat_id) => {
                if chat_id.trim().is_empty() {
                    return Err("飞书唤醒目标的 chat_id 不能为空".to_owned());
                }
            }
            Self::Session {
                channel,
                destination,
            } => {
                let channel = channel.trim();
                let destination = destination.trim();
                if channel.is_empty() {
                    return Err("会话唤醒目标的渠道不能为空".to_owned());
                }
                if destination.is_empty() {
                    return Err("会话唤醒目标的车道标识不能为空".to_owned());
                }
                // TUI 车道标识按进程随机生成，定时任务触发时早已失效，注入只会得到
                // 一条无人消费的孤儿回合。
                if channel != FEISHU_CHANNEL && channel != desktop_session_channel() {
                    return Err(format!(
                        "会话唤醒目标只支持 {} 与 {}，不支持 {channel}",
                        FEISHU_CHANNEL,
                        desktop_session_channel()
                    ));
                }
                if channel == desktop_session_channel()
                    && destination.starts_with(CRON_LANE_DESTINATION_PREFIX)
                {
                    return Err("会话唤醒目标不能指向 Cron 自身车道".to_owned());
                }
            }
        }
        Ok(())
    }
}

/// 桌面会话表面渠道标识。
#[must_use]
pub const fn desktop_session_channel() -> &'static str {
    "desktop"
}

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
    /// 完成后的唤醒目标；`None` 表示执行完只落运行事实，不产生任何投递。
    ///
    /// 该字段没有 serde 默认值：历史版本 1 的载荷必须显式迁移，否则解析失败，
    /// 避免把「显式声明过飞书目标」的任务静默降级成不投递。
    pub wake: Option<CronWakeTarget>,
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
    /// 完成后的唤醒目标。
    pub wake: Option<CronWakeTarget>,
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
    /// 新唤醒目标；配合 `replace_wake` 生效。
    pub wake: Option<CronWakeTarget>,
    /// 是否替换唤醒目标。
    pub replace_wake: bool,
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
    /// 是否已按 `wake` 目标产生一次外部动作（登记投递项或注入一轮主 turn）。
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
    #[error("cron 唤醒目标无效：{0}")]
    InvalidWakeTarget(String),
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
            wake: input.wake,
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
        if update.replace_wake {
            next.wake = update.wake;
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
        if let Some(wake) = self.wake.as_ref() {
            wake.validate()
                .map_err(CronValidationError::InvalidWakeTarget)?;
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
    fn cron_job_transition_replaces_wake_target() {
        let job = CronJob::new(test_input(), 10, Some(20)).expect("合法任务应可创建");
        assert_eq!(job.wake, Some(CronWakeTarget::Feishu("oc_test".to_owned())));
        let updated = job
            .with_update(
                CronJobUpdate {
                    wake: Some(CronWakeTarget::Session {
                        channel: "desktop".to_owned(),
                        destination: "workbench-1".to_owned(),
                    }),
                    replace_wake: true,
                    ..CronJobUpdate::default()
                },
                30,
                Some(40),
            )
            .expect("合法更新应成功");
        let completed = updated.after_run(40, None, true);
        let cleared = updated
            .with_update(
                CronJobUpdate {
                    wake: None,
                    replace_wake: true,
                    ..CronJobUpdate::default()
                },
                50,
                Some(60),
            )
            .expect("清空唤醒目标应成功");

        assert_eq!(
            updated.wake,
            Some(CronWakeTarget::Session {
                channel: "desktop".to_owned(),
                destination: "workbench-1".to_owned(),
            })
        );
        assert_eq!(
            updated.wake.as_ref().map(CronWakeTarget::channel),
            Some("desktop")
        );
        assert_eq!(
            updated.wake.as_ref().map(CronWakeTarget::destination),
            Some("workbench-1")
        );
        assert!(
            updated
                .wake
                .as_ref()
                .is_some_and(CronWakeTarget::wakes_turn)
        );
        assert_eq!(cleared.wake, None);
        assert_eq!(completed.status, CronJobStatus::Completed);
        assert_eq!(completed.last_run_at_ms, Some(40));
    }

    #[test]
    fn wake_target_rejects_unstable_or_self_lane() {
        let tui = CronWakeTarget::Session {
            channel: "tui".to_owned(),
            destination: "terminal-1".to_owned(),
        };
        let self_lane = CronWakeTarget::Session {
            channel: "desktop".to_owned(),
            destination: format!("{CRON_LANE_DESTINATION_PREFIX}cron-abc"),
        };
        let empty_chat = CronWakeTarget::Feishu("   ".to_owned());
        let valid = CronWakeTarget::Session {
            channel: "desktop".to_owned(),
            destination: "workbench-1".to_owned(),
        };

        assert!(
            tui.validate().is_err(),
            "TUI 车道标识不稳定，不得作为唤醒目标"
        );
        assert!(self_lane.validate().is_err(), "不得唤醒 Cron 自身车道");
        assert!(empty_chat.validate().is_err());
        assert!(valid.validate().is_ok());
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
            wake: Some(CronWakeTarget::Feishu("oc_test".to_owned())),
            workdir: None,
            no_agent: false,
            script: None,
        }
    }
}
