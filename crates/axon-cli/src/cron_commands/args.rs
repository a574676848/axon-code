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
    /// 完成后的通知目标，两种取值互斥。
    ///
    /// `feishu:<chat_id>`：把结果正文直接发到该飞书会话，用于通知用户，不消耗模型。
    ///
    /// `session:<channel>:<destination>`：向该会话注入一轮主 turn，用于让发布任务的会话
    /// 自己跟进结果，按一轮对话计费。要回调当前会话，把运行上下文里给出的会话标识原样填上。
    ///
    /// channel 只支持 feishu 与 desktop。省略时任务执行完只记录运行事实，不产生任何投递。
    #[arg(long, verbatim_doc_comment)]
    pub(crate) wake: Option<String>,
    #[arg(long)]
    pub(crate) workdir: Option<PathBuf>,
    #[arg(long)]
    pub(crate) no_agent: bool,
    #[arg(long)]
    pub(crate) script: Option<String>,
    /// 已移除：命中时给出中文迁移提示，不再参与任何投递。
    #[arg(long, hide = true, value_name = "CHAT_ID")]
    pub(crate) feishu_chat_id: Option<String>,
    /// 已移除：命中时给出中文迁移提示，不再参与任何投递。
    #[arg(long, hide = true, value_name = "THREAD_ID")]
    pub(crate) feishu_thread_id: Option<String>,
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
    /// 新的通知目标，语义与 create 的 `--wake` 一致。
    ///
    /// `feishu:<chat_id>` 直投飞书会话；`session:<channel>:<destination>` 注入一轮主 turn。
    /// channel 只支持 feishu 与 desktop。
    #[arg(long, verbatim_doc_comment)]
    pub(crate) wake: Option<String>,
    /// 清空通知目标；此后的执行不再产生任何投递。
    #[arg(long)]
    pub(crate) clear_wake: bool,
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
    /// 已移除：命中时给出中文迁移提示，不再参与任何投递。
    #[arg(long, hide = true, value_name = "CHAT_ID")]
    pub(crate) feishu_chat_id: Option<String>,
    /// 已移除：命中时给出中文迁移提示，不再参与任何投递。
    #[arg(long, hide = true, value_name = "THREAD_ID")]
    pub(crate) feishu_thread_id: Option<String>,
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
