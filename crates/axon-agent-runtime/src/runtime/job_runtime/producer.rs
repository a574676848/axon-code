//! 生产者接口与统一终态投影。
//!
//! 统一生命周期是七个阶段的固定序列：create → start → read/wait → cancel/kill →
//! terminal → artifact → tombstone/reap。注册表固定**阶段顺序、owner 规则、
//! 终态结构与保留窗口**，生产者只提供该阶段特有的动作（构建执行载荷、启动执行体、
//! 完整中断语义、结果归档），不重写阶段顺序（合同 §5.1.1）。
//!
//! 本模块只依赖纯数据类型，不感知注册表内部结构。

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use axon_domain::AxonResult;

use super::registry::{JobId, JobStatus};
use crate::ShellCommandBoundedStream;

/// 生产者类型（合同 §5.1.4 通知载荷字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobProducerKind {
    /// Shell 后台作业。
    Shell,
    /// 一次性后台子代理。
    Subagent,
}

impl JobProducerKind {
    /// 稳定标签；事件键与通知载荷使用。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Subagent => "subagent",
        }
    }
}

/// 六类终态（合同 §8.1 第 6 条）。
///
/// 六类退出**互不合并**：`Refusal` 只表示执行前的准入裁决拒绝了这次任务
/// （父代理不得把它误判为「做完了」或「做失败了」），`Killed` 只表示显式终止，
/// 墙钟超时与空闲超时各有独立终态，避免把不同结局压成同一个词。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobTerminalKind {
    /// 正常结束。
    Completed,
    /// 任务未被接受：执行前的准入裁决（策略门 / 治理门）拒绝了这次任务。
    Refusal,
    /// 结束但失败。
    Failed,
    /// 被显式终止。
    Killed,
    /// 墙钟超时。
    Timeout,
    /// 空闲超时。
    Unresponsive,
}

impl JobTerminalKind {
    /// 稳定标签；通知载荷与事件键使用。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Refusal => "refusal",
            Self::Failed => "failed",
            Self::Killed => "killed",
            Self::Timeout => "timeout",
            Self::Unresponsive => "unresponsive",
        }
    }

    /// 对应的作业状态。
    #[must_use]
    pub const fn status(self) -> JobStatus {
        match self {
            Self::Completed => JobStatus::Completed,
            Self::Refusal => JobStatus::Refusal,
            Self::Failed => JobStatus::Failed,
            Self::Killed => JobStatus::Killed,
            Self::Timeout => JobStatus::Timeout,
            Self::Unresponsive => JobStatus::Unresponsive,
        }
    }
}

impl JobStatus {
    /// 六类终态投影；仍在运行时为 `None`。
    #[must_use]
    pub const fn terminal_kind(self) -> Option<JobTerminalKind> {
        match self {
            Self::Running => None,
            Self::Completed => Some(JobTerminalKind::Completed),
            Self::Refusal => Some(JobTerminalKind::Refusal),
            Self::Failed => Some(JobTerminalKind::Failed),
            Self::Killed => Some(JobTerminalKind::Killed),
            Self::Timeout => Some(JobTerminalKind::Timeout),
            Self::Unresponsive => Some(JobTerminalKind::Unresponsive),
        }
    }
}

/// 临时 Job 的生产者：把一类执行体接入同一生命周期。
///
/// 泛型参数是该生产者独有的启动载荷类型；注册表按载荷类型选择生产者，
/// 因此不存在运行期向下转型。
pub(crate) trait JobProducer<P>: Send + Sync {
    /// 生产者类型。
    fn kind(&self) -> JobProducerKind;

    /// 本次提交的 owner；由载荷给出，注册表负责校验。
    fn owner_id<'a>(&self, payload: &'a P) -> &'a str;

    /// 本次提交的展示标签。
    fn label<'a>(&self, payload: &'a P) -> &'a str;

    /// 校验载荷并启动执行体；`job_id` 由注册表在调用前分配。
    ///
    /// 返回错误即本次提交失败：注册表不登记作业，调用方拿到的是稳定拒绝原因。
    fn start(&self, job_id: JobId, payload: P) -> AxonResult<Box<dyn JobExecution>>;
}

/// 执行体投影出的终态。
#[derive(Debug, Clone)]
pub(crate) struct JobTerminalState {
    /// 终态状态。
    pub(crate) status: JobStatus,
    /// 终态退出码；无退出码的结局为 `None`。
    pub(crate) exit_code: Option<i32>,
    /// 有界退出原因；人可读单行。
    pub(crate) reason: Option<String>,
}

/// 执行体的一次增量输出批次。
pub(crate) struct JobExecutionBatch {
    /// 本批 stdout 增量。
    pub(crate) stdout: ShellCommandBoundedStream,
    /// 本批 stderr 增量。
    pub(crate) stderr: ShellCommandBoundedStream,
}

/// 单个作业的执行体。
///
/// 注册表只通过本接口触达执行体：它不感知进程、子代理会话或任何具体执行机制。
///
/// 只要求 `Send` 而不要求 `Sync`：执行体一律被 `Mutex` 包裹（`Mutex<T>: Sync`
/// 只要求 `T: Send`），而 PTY 主设备等真实句柄本就只有 `Send`。
pub(crate) trait JobExecution: Send {
    /// 推进状态：取消标记、墙钟超时、空闲超时与自然结束。
    ///
    /// `idle_deadline` 由注册表按「最后一次读取 + 空闲窗口」算出，
    /// 因此「什么是空闲」是统一生命周期职责，不是生产者自定规则。
    fn poll(&mut self, now: Instant, idle_deadline: Instant) -> std::io::Result<()>;

    /// 终止执行体。
    ///
    /// 实现必须保证成功返回后执行体处于终态（终止即 `Killed`）；
    /// 完整中断语义（如子代理的中止 turn + 关闭会话）由生产者自负。
    ///
    /// 默认原因按 Agent 自己发起的 `job_kill` 记录；需要区分终止来源时用
    /// [`Self::terminate_with_reason`]。
    fn terminate(&mut self) -> std::io::Result<()> {
        self.terminate_with_reason(JOB_REASON_KILLED)
    }

    /// 按调用方给定的原因终止执行体。
    ///
    /// 原因是终态的一部分，会随完成通知回到发起它的 Agent：模型据此判断
    /// 「是我自己停的」还是「用户从界面上停的」。两条来源必须给出不同原因，
    /// 否则模型只能靠猜。
    fn terminate_with_reason(&mut self, reason: &str) -> std::io::Result<()>;

    /// 当前终态；仍在运行时为 `None`。
    fn terminal(&self) -> Option<JobTerminalState>;

    /// 输出变更代号：每次输出追加或状态跃迁时递增。
    fn change_generation(&self) -> u64;

    /// 等待输出变更或超时。
    fn wait_for_change(&self, observed: u64, timeout: Duration);

    /// 取走本批增量输出。
    fn take_batch(&self) -> JobExecutionBatch;

    /// 完整输出归档路径；作业启动时即创建。
    fn artifact_path(&self) -> PathBuf;

    /// 归档是否完整（未被容量上限截断、写入未失败）。
    fn full_output_available(&self) -> bool;

    /// 完成通知载荷里的结算正文。
    ///
    /// 输出本身就在归档里、通知只需给出引用的生产者返回 `None`；
    /// 有终局正文的生产者（子代理）返回其结束语与引用。
    fn notice_settlement(&self) -> Option<JobNoticeSettlement> {
        None
    }
}

/// 完成通知里的结算正文（合同 §5.1.4 / §5.1.9）。
///
/// 两个字段刻意分开承载：通知渲染时引用**先于**正文出现，因此正文被有界截断时
/// 丢掉的只是文本尾部，`final_artifact_ref` 不会被截掉（§4 不变量 6）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JobNoticeSettlement {
    /// 结束语：子代理最后一条非空 assistant 文本块；没有则为 `None`，
    /// 由通知层用固定替换句落地。
    pub(crate) final_message: Option<String>,
    /// 结算正文超过内联阈值时的完整结果归档引用。
    pub(crate) final_artifact_ref: Option<String>,
}

/// 取消标记命中导致的终止原因。
pub(crate) const JOB_REASON_CANCELLED: &str = "取消标记命中";
/// 墙钟超时导致的终止原因。
pub(crate) const JOB_REASON_TIMEOUT: &str = "墙钟超时";
/// 空闲超时导致的终止原因。
pub(crate) const JOB_REASON_IDLE: &str = "空闲超时";
/// 显式 `job_kill` 导致的终止原因。
pub(crate) const JOB_REASON_KILLED: &str = "job_kill 终止";
/// 用户在界面上手动停止作业导致的终止原因。
///
/// 与 [`JOB_REASON_KILLED`] 分开：前者由 Agent 自己发起，后者由人发起；
/// 模型收到的完成通知必须能区分这两种来源。
pub const JOB_REASON_USER_STOP: &str = "用户手动停止";
/// 执行体推进失败导致的终止原因。
pub(crate) const JOB_REASON_EXECUTION_ERROR: &str = "执行体推进失败";
/// 预算耗尽导致的终止原因。
///
/// 预算耗尽**独立于普通失败**：它描述的是「模型输出 token 上限把这一轮截断了」，
/// 而不是「这次执行出错了」，因此必须有一句自己的固定措辞，否则父代理只能看到
/// 一句泛化的失败原因而无法判断该重试还是该换策略。
pub const JOB_REASON_BUDGET_EXHAUSTED: &str = "预算耗尽（模型输出 token 上限）";

/// 退出原因统一截断到有界长度。
#[must_use]
pub(crate) fn bounded_job_reason(reason: &str) -> String {
    reason
        .chars()
        .take(crate::runtime_limits::JOB_EXIT_REASON_MAX_CHARS)
        .collect()
}

#[cfg(test)]
#[path = "producer_tests.rs"]
mod tests;
