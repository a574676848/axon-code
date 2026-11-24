//! Agent TUI 完成通知收件箱。
//!
//! 后台 Execution 的完成消息由 Daemon 的持久收件箱负责投递：每个 Execution 只通知
//! 主 turn 一次，主 turn 汇总回复会投递到所有可用渠道；TUI 是其中一个在线客户端。
//! TUI 在启动时声明客户端 presence，展示收件箱正文后才确认 delivery，确保没有静默 ack。

use std::{
    collections::HashSet,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    time::Duration,
};

/// 单次轮询的完成通知上限：保证一次 tick 的展示量有界。
const COMPLETION_INBOX_POLL_LIMIT: u32 = 8;
/// 后台轮询间隔：决定完成消息进入 transcript 的最大延迟。
const COMPLETION_INBOX_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 通知通道容量：满载时未取走的通知保持 dispatched，由下一轮重投。
const COMPLETION_INBOX_CHANNEL_CAPACITY: usize = 32;
/// 退出时等待后台线程注销 presence 的上限；超时交给 Daemon 的 presence TTL 兜底，
/// 绝不允许收件箱阻塞终端恢复。
const COMPLETION_INBOX_SHUTDOWN_WAIT: Duration = Duration::from_millis(1_000);
/// 后台线程启动失败说明。
const COMPLETION_INBOX_THREAD_FAILED: &str = "完成通知收件箱后台线程启动失败";

/// 一条已派发到当前客户端、等待展示与回执的完成通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiCompletionNotification {
    /// 稳定通知 ID，用于回执确认与展示去重。
    pub notification_id: String,
    /// 后台执行结论摘要。
    pub content: String,
}

/// 宿主提供的完成通知收件箱客户端。
pub trait AgentTuiCompletionInboxClient: Send + 'static {
    /// 拉取一批待展示通知，并顺带续期客户端 presence。
    fn poll(&mut self, limit: u32) -> Result<Vec<AgentTuiCompletionNotification>, String>;

    /// 回执确认已展示的通知；失败时调用方会在下一轮重试。
    fn acknowledge(&mut self, notification_ids: &[String]) -> Result<(), String>;

    /// 注销客户端 presence；其他渠道 delivery 不受影响。
    fn unregister(&mut self);
}

/// 后台轮询线程句柄。
pub struct AgentTuiCompletionInbox {
    notifications: Receiver<AgentTuiCompletionNotification>,
    acknowledgements: Sender<Vec<String>>,
    failures: Receiver<String>,
    shutdown: Sender<()>,
    finished: Receiver<()>,
    handle: Option<std::thread::JoinHandle<()>>,
    displayed: HashSet<String>,
}

impl AgentTuiCompletionInbox {
    /// 启动后台收件箱线程。
    ///
    /// 客户端既已在构造阶段完成 presence 注册，也可以在 `poll` 内续期。
    #[must_use]
    pub fn spawn(
        name: impl Into<String>,
        mut client: Box<dyn AgentTuiCompletionInboxClient>,
    ) -> Self {
        let (notifications_tx, notifications) =
            mpsc::sync_channel(COMPLETION_INBOX_CHANNEL_CAPACITY);
        let (acknowledgements, acknowledgements_rx) = mpsc::channel::<Vec<String>>();
        let (failures_tx, failures) = mpsc::channel::<String>();
        let (shutdown, shutdown_rx) = mpsc::channel::<()>();
        let (finished_tx, finished) = mpsc::sync_channel(1);
        // 线程启动失败也要给出显式失败说明：保留一个句柄供 spawn 失败后上报。
        let startup_failure = failures_tx.clone();
        let handle = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                run_completion_inbox(
                    client.as_mut(),
                    &notifications_tx,
                    &acknowledgements_rx,
                    &failures_tx,
                    &shutdown_rx,
                );
                client.unregister();
                let _ = finished_tx.try_send(());
            })
            .ok();
        if handle.is_none() {
            let _ = startup_failure.send(COMPLETION_INBOX_THREAD_FAILED.to_owned());
        }
        Self {
            notifications,
            acknowledgements,
            failures,
            shutdown,
            finished,
            handle,
            displayed: HashSet::new(),
        }
    }

    /// 取出本轮需要展示的通知；同一通知只返回一次。
    pub(in crate::r#loop::agent_tui) fn drain(&mut self) -> Vec<AgentTuiCompletionNotification> {
        let mut pending = Vec::new();
        while let Ok(notification) = self.notifications.try_recv() {
            if self.displayed.insert(notification.notification_id.clone()) {
                pending.push(notification);
            } else {
                // 已展示过但回执未达：重新回执，避免通知长期停留在 dispatched。
                self.acknowledge(&notification.notification_id);
            }
        }
        pending
    }

    /// 回执确认一条已展示的通知。
    pub(in crate::r#loop::agent_tui) fn acknowledge(&self, notification_id: &str) {
        let _ = self.acknowledgements.send(vec![notification_id.to_owned()]);
    }

    /// 取出最近一次后台失败说明；同一失败只报告一次。
    pub(in crate::r#loop::agent_tui) fn take_failure(&self) -> Option<String> {
        self.failures.try_recv().ok()
    }
}

impl Drop for AgentTuiCompletionInbox {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        let Some(handle) = self.handle.take() else {
            return;
        };
        if self
            .finished
            .recv_timeout(COMPLETION_INBOX_SHUTDOWN_WAIT)
            .is_ok()
        {
            let _ = handle.join();
        }
    }
}

/// 后台收件箱主循环：先回执，再轮询，最后按固定间隔等待唤醒信号。
fn run_completion_inbox(
    client: &mut dyn AgentTuiCompletionInboxClient,
    notifications: &mpsc::SyncSender<AgentTuiCompletionNotification>,
    acknowledgements: &Receiver<Vec<String>>,
    failures: &Sender<String>,
    shutdown: &Receiver<()>,
) {
    let mut last_failure: Option<String> = None;
    loop {
        // 回执优先：确认越早，Daemon 就能越早结算对应 Result。
        while let Ok(ids) = acknowledgements.try_recv() {
            if let Err(error) = client.acknowledge(&ids) {
                report_failure(failures, &mut last_failure, error);
            }
        }
        match client.poll(COMPLETION_INBOX_POLL_LIMIT) {
            Ok(items) => {
                last_failure = None;
                for item in items {
                    if notifications.try_send(item).is_err() {
                        // 通道满载：未取走的通知保持 dispatched，下一轮重新拉取。
                        break;
                    }
                }
            }
            Err(error) => report_failure(failures, &mut last_failure, error),
        }
        match shutdown.recv_timeout(COMPLETION_INBOX_POLL_INTERVAL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// 上报后台失败；连续相同的失败只报告一次，避免每轮轮询都刷同一条提示。
fn report_failure(failures: &Sender<String>, last_failure: &mut Option<String>, error: String) {
    if last_failure.as_deref() == Some(error.as_str()) {
        return;
    }
    *last_failure = Some(error.clone());
    // 失败通道不设容量：丢弃失败说明等于让 TUI 静默失联，必须尽力投递。
    let _ = failures.send(error);
}
