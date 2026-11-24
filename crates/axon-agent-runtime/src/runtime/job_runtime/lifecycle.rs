//! 作业生命周期事件：临时 Job 的启停对外广播。
//!
//! 为什么需要它：Job 注册表是进程内事实源，但「这一轮起了哪些后台作业、各自什么结局」
//! 必须能被订阅者看到并落进会话事件流，否则回放与排障只看到工具调用，看不到作业生命
//! 周期。事件只广播、不改状态：真实状态仍在注册表，订阅方写失败也不得反向影响作业。
//!
//! 与 [`super::notice`] 的分工：Notice 负责唤醒父代理继续工作（进程内、去重、有界正文）；
//! 本模块只产出可落盘的事实，不带渲染语义。临时 Job 随进程消亡，因此这些事件是
//! 「历史可回放」而不是「可续跑」：重启后能读到启停事实，不会让作业复活。

use std::sync::{Arc, LazyLock, Mutex};

use super::producer::{JobProducerKind, JobTerminalKind};

/// 作业标识（注册表内自增序号）。
pub type JobLifecycleId = u64;

/// 作业启动事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStarted {
    /// 作业标识。
    pub job_id: JobLifecycleId,
    /// 归属的运行会话；由运行时注入，不由模型提供。
    pub owner_id: String,
    /// 生产者类型。
    pub producer: JobProducerKind,
    /// 有界展示标签。
    pub label: String,
    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
}

/// 作业终态事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobCompleted {
    /// 作业标识。
    pub job_id: JobLifecycleId,
    /// 归属的运行会话。
    pub owner_id: String,
    /// 生产者类型。
    pub producer: JobProducerKind,
    /// 六类终态之一。
    pub terminal: JobTerminalKind,
    /// 有界退出原因。
    pub reason: Option<String>,
    /// 退出码（若可得）。
    pub exit_code: Option<i32>,
    /// 从启动到终态的耗时；未测得时为 `None`，不用 0 冒充。
    pub elapsed_ms: Option<u64>,
    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
    /// 终态时刻（Unix 毫秒）。
    pub ended_at_ms: u64,
}

/// 作业生命周期事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobLifecycleEvent {
    /// 作业已启动。
    Started(JobStarted),
    /// 作业已进入终态。
    Completed(JobCompleted),
}

impl JobLifecycleEvent {
    /// 事件所属的运行会话。
    #[must_use]
    pub fn owner_id(&self) -> &str {
        match self {
            Self::Started(started) => &started.owner_id,
            Self::Completed(completed) => &completed.owner_id,
        }
    }
}

/// 事件订阅者签名。
pub type JobLifecycleHandler = Arc<dyn Fn(&JobLifecycleEvent) + Send + Sync>;

struct BusState {
    subscribers: Vec<(u64, JobLifecycleHandler)>,
    next_subscription_id: u64,
}

static BUS: LazyLock<Mutex<BusState>> = LazyLock::new(|| {
    Mutex::new(BusState {
        subscribers: Vec::new(),
        next_subscription_id: 1,
    })
});

/// 订阅句柄：删除即退订。宿主应在进程生命周期内持有它。
#[derive(Debug)]
pub struct JobLifecycleSubscription {
    subscription_id: u64,
}

impl Drop for JobLifecycleSubscription {
    fn drop(&mut self) {
        let mut state = BUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .subscribers
            .retain(|(id, _)| *id != self.subscription_id);
    }
}

/// 订阅作业生命周期事件。
pub fn subscribe_job_lifecycle(handler: JobLifecycleHandler) -> JobLifecycleSubscription {
    let mut state = BUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let subscription_id = state.next_subscription_id;
    state.next_subscription_id = state.next_subscription_id.saturating_add(1);
    state.subscribers.push((subscription_id, handler));
    JobLifecycleSubscription { subscription_id }
}

/// 广播一条作业生命周期事件。
///
/// 订阅方是逐条同步调用的：事件顺序与作业真实状态变化顺序一致，调用方因此不需要
/// 另建重排窗口。订阅方自己的错误只能自己吞掉，不得抛回作业线程。
pub fn publish_job_lifecycle(event: JobLifecycleEvent) {
    let handlers = {
        let state = BUS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .subscribers
            .iter()
            .map(|(_, handler)| Arc::clone(handler))
            .collect::<Vec<_>>()
    };
    for handler in handlers {
        handler(&event);
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
