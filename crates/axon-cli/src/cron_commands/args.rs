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
    /// 同步或查看法定节假日年历。
    Holiday(CronHolidayCommand),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronHolidayCommand {
    #[command(subcommand)]
    pub(crate) action: CronHolidayAction,
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub(crate) enum CronHolidayAction {
    /// 从远端年历同步节假日安排。
    Sync(CronHolidaySyncCommand),
    /// 查看已同步年份、数据目录与数据源。
    Status,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronHolidaySyncCommand {
    /// 指定年份，可重复；省略时同步当前年与次年。
    #[arg(long = "year")]
    pub(crate) years: Vec<i32>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub(crate) struct CronCreateCommand {
    #[arg(long)]
    pub(crate) name: String,
    /// 计划表达式，三类写法。
    ///
    /// 固定间隔：`interval:<数值><单位>`，单位 ms、s、m、h，例如 interval:30m。
    ///
    /// 一次性任务：`once:YYYY-MM-DDTHH:MM:SS`，按本机时区解析。
    ///
    /// cron 计划：`[日历前缀:]分 时 日 月 星期`，五个字段均支持 * 与 ?、
    /// 单值、区间 1-9、步长 */5、范围步长 9-18/2、单值步长 9/2、逗号组合 9,12-14；
    /// 星期 0 表示周日。日与星期一侧为 * 或 ? 时只按另一侧判定，
    /// 两侧都具体时任一满足即触发。
    ///
    /// 日历前缀可选且能与任意字段自由组合，按中国日历筛选日期
    /// （weekday 只按周一到周五，不读年历）：
    /// workday 表示工作日（排除法定节假日，含调休上班日），
    /// weekday 表示周一至周五（不看节假日），
    /// holiday 表示法定节假日，weekend 表示周末（排除调休上班日）。
    ///
    /// 例：`workday:0 8-20 * * *` 工作日 8 点到 20 点每个整点；
    /// `workday:*/30 9-18 * * *` 工作日 9 点到 18 点每半小时；
    /// `holiday:0 10 * * *` 法定节假日 10 点；
    /// `weekend:0 9 * * *` 周末 9 点。
    ///
    /// 另支持标准别名 @hourly、@daily、@midnight、@weekly、@monthly、
    /// @yearly、@annually，也可加日历前缀，例如 `workday:@daily`。
    ///
    /// 日历按本机时区判定，年历数据用 axon cron holiday sync 更新。
    #[arg(long, verbatim_doc_comment)]
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
    /// 计划表达式，语法与 create 的 `--schedule` 完全一致：
    /// `interval:<数值><单位>`、`once:YYYY-MM-DDTHH:MM:SS`、或
    /// `[日历前缀:]分 时 日 月 星期`，也接受 @hourly、@daily 等标准别名。
    ///
    /// 五个字段均支持 * 与 ?、单值、区间 1-9、步长 */5、范围步长 9-18/2、
    /// 单值步长 9/2、逗号组合 9,12-14；日与星期一侧为 * 或 ? 时只按另一侧判定。
    /// 日历前缀 workday、weekday、holiday、weekend 可与任意字段组合，
    /// 例如 workday:0 8-20 * * *；weekday 只按周一到周五，不读年历。
    #[arg(long, verbatim_doc_comment)]
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
