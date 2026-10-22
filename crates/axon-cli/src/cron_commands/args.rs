use std::path::PathBuf;

use clap::{Args, Subcommand};

/// `axon cron` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct CronCommand {
    #[command(subcommand)]
    pub(crate) action: CronAction,
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub(crate) enum CronAction {
    /// 创建用户级 Agent Cron。
    Create(CronCreateCommand),
    /// 更新用户级 Agent Cron。
    Update(CronUpdateCommand),
    /// 列出用户级 Agent Cron。
    List(CronListCommand),
    /// 暂停用户级 Agent Cron。
    Pause(CronJobIdCommand),
    /// 恢复用户级 Agent Cron。
    Resume(CronJobIdCommand),
    /// 移除用户级 Agent Cron。
    Remove(CronJobIdCommand),
    /// 立即运行单个用户级 Agent Cron。
    Run(CronRunCommand),
    /// 执行一次用户级 Agent Cron tick。
    Tick(CronTickCommand),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronCreateCommand {
    #[arg(long)]
    pub(crate) name: String,
    /// 计划表达式：`interval:5m`、本地时间 `once:YYYY-MM-DDTHH:MM:SS` 或五段 cron。
    #[arg(long)]
    pub(crate) schedule: String,
    #[arg(long, default_value = "")]
    pub(crate) prompt: String,
    #[arg(long = "skill")]
    pub(crate) skills: Vec<String>,
    /// 可选飞书投递目标；省略时任务执行但不产生外部通知。
    #[arg(long)]
    pub(crate) feishu_chat_id: Option<String>,
    /// 可选飞书线程目标，必须与 `--feishu-chat-id` 一起使用。
    #[arg(long, requires = "feishu_chat_id")]
    pub(crate) feishu_thread_id: Option<String>,
    #[arg(long)]
    pub(crate) workdir: Option<PathBuf>,
    #[arg(long)]
    pub(crate) no_agent: bool,
    #[arg(long)]
    pub(crate) script: Option<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronUpdateCommand {
    pub(crate) job_id: String,
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// 计划表达式：`interval:5m`、本地时间 `once:YYYY-MM-DDTHH:MM:SS` 或五段 cron。
    #[arg(long)]
    pub(crate) schedule: Option<String>,
    #[arg(long)]
    pub(crate) prompt: Option<String>,
    #[arg(long = "skill")]
    pub(crate) skills: Vec<String>,
    #[arg(long)]
    pub(crate) clear_skills: bool,
    #[arg(long)]
    pub(crate) feishu_chat_id: Option<String>,
    #[arg(long)]
    pub(crate) feishu_thread_id: Option<String>,
    #[arg(long)]
    pub(crate) clear_thread: bool,
    #[arg(long)]
    pub(crate) workdir: Option<PathBuf>,
    #[arg(long)]
    pub(crate) clear_workdir: bool,
    #[arg(long)]
    pub(crate) no_agent: Option<bool>,
    #[arg(long)]
    pub(crate) script: Option<String>,
    #[arg(long)]
    pub(crate) clear_script: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronListCommand {
    #[arg(long)]
    pub(crate) limit: Option<usize>,
    #[arg(long)]
    pub(crate) include_removed: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronJobIdCommand {
    pub(crate) job_id: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronRunCommand {
    pub(crate) job_id: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronTickCommand {
    #[arg(long)]
    pub(crate) limit: Option<usize>,
}
