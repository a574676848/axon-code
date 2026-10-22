//! 统一完成通知：载荷、稳定事件键与去重投递（合同 §5.1.4）。
//!
//! 核心定位：**通知不是状态事实来源**。通知只负责唤醒与提示，真实状态仍以
//! Job 注册表（临时 Job）与 Work Ledger（Goal/Execution）为准。
//!
//! 载荷必须自证归属：收到通知的 turn 能仅凭载荷判断「哪个作业、什么结局、
//! 去哪里读完整结果」。
//!
//! 进程边界（设计 §4.3）：临时 Job 随进程消亡，因此临时 Job 通知**只在本进程内
//! 投递**，不落任何持久表：指向已消亡作业的通知是错误事实。daemon Execution 通知
//! 属持久事实，继续走既有的 `jarvis_completion_notifications` /
//! `jarvis_completion_deliveries` 投递状态机，不经过本模块。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use super::producer::{JobProducerKind, JobTerminalKind};
use crate::runtime_limits::{
    COMPLETION_NOTICE_DEDUP_CAPACITY, COMPLETION_NOTICE_MAX_CHARS, COMPLETION_NOTICE_MAX_PER_TURN,
    COMPLETION_NOTICE_REASON_MAX_CHARS, COMPLETION_NOTICE_RETENTION,
    COMPLETION_NOTICE_SUMMARY_MAX_CHARS,
};

/// 正文超限时的截断标记；截断只丢结算正文，不丢终态与引用。
const NOTICE_TRUNCATION_MARKER: &str = "\n…（通知正文已截断，完整结果见上方引用）";

/// `completed` 类对可续接子代理的行为提示（合同 §5.1.9）。
///
/// 子代理完成后不会自己继续做任何事；没有这句话，父代理容易把它当成仍在推进的
/// 后台任务而空等，或以为它会自行跟进。
const CONTINUABLE_SUBAGENT_HINT: &str = "除非你再给它发消息，否则它不会做更多工作。";

/// 子代理没有留下任何非空 assistant 文本块时的固定替换句（合同 §5.1.9）。
const NO_FINAL_MESSAGE: &str = "它没有留下结束语。";

/// 终态首行的固定措辞表（合同 §5.1.9）。
///
/// 首行按终态分支使用**每类固定**的措辞：父代理只读首行就能判断这次委派的结局，
/// 不必解析正文，也不会把「任务未被接受」误读成「做完了」或「做失败了」。
const fn notice_title(terminal: JobTerminalKind) -> &'static str {
    match terminal {
        JobTerminalKind::Completed => "任务已完成",
        JobTerminalKind::Refusal => "任务未被接受",
        JobTerminalKind::Failed => "任务执行失败",
        JobTerminalKind::Killed => "任务已被终止",
        JobTerminalKind::Timeout => "任务已因墙钟超时被终止",
        JobTerminalKind::Unresponsive => "任务已因空闲超时被终止",
    }
}

/// 统一完成通知载荷（合同 §5.1.4 的六项字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionNotice {
    /// 生产者类型。
    pub producer: JobProducerKind,
    /// 稳定事件键：`{producer}:{runtime_epoch}:{job_id}:{terminal}`，去重的唯一依据。
    pub notice_key: String,
    /// 作业引用：临时 Job 为 `{runtime_epoch}:{job_id}`。
    pub job_ref: String,
    /// 六类终态之一。
    pub terminal: JobTerminalKind,
    /// 有界退出原因。
    pub exit_reason: String,
    /// 有界结算正文；超过上限时只有正文，完整内容在 [`Self::artifact_ref`]。
    ///
    /// 子代理生产者为「最后一条非空 assistant 文本块」（无则为空串，
    /// 渲染时落地为固定替换句），Shell 生产者为有界输出摘要。
    pub summary: String,
    /// 完整结果引用；输出归档可用时给出。
    pub artifact_ref: Option<String>,
    /// 待读 mailbox 引用；无则 `None`。
    pub mailbox_ref: Option<String>,
    /// 投递路由用的 owner；由运行时注入，**不进入渲染正文**。
    pub owner_id: String,
}

impl CompletionNotice {
    /// 构造通知：所有文本字段在此统一收敛到有界长度。
    #[must_use]
    pub fn new(
        producer: JobProducerKind,
        notice_key: String,
        job_ref: String,
        owner_id: String,
        terminal: JobTerminalKind,
        exit_reason: &str,
        summary: &str,
        artifact_ref: Option<String>,
        mailbox_ref: Option<String>,
    ) -> Self {
        Self {
            producer,
            notice_key,
            job_ref,
            terminal,
            exit_reason: bounded_text(exit_reason, COMPLETION_NOTICE_REASON_MAX_CHARS),
            summary: bounded_text(summary, COMPLETION_NOTICE_SUMMARY_MAX_CHARS),
            artifact_ref,
            mailbox_ref,
            owner_id,
        }
    }

    /// 渲染为注入主 turn 的有界正文。
    ///
    /// 结构固定为「终态首行 → 引用（完整结果 / 待读 mailbox）→ 可续接提示 →
    /// 结算正文」。引用写在结算正文之前：正文超限时先丢正文尾部，
    /// **宁可丢正文，不可丢 artifact 引用与终态**。
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = format!(
            "{}（生产者为 {}）：\n- 作业引用：{}\n- 终态：{}\n- 退出原因：{}\n",
            notice_title(self.terminal),
            self.producer.as_str(),
            self.job_ref,
            self.terminal.as_str(),
            self.exit_reason
        );
        if let Some(artifact) = self.artifact_ref.as_deref() {
            text.push_str(&format!("- 完整结果：{artifact}\n"));
        }
        if let Some(mailbox) = self.mailbox_ref.as_deref() {
            text.push_str(&format!("- 待读 mailbox：{mailbox}\n"));
        }
        if self.terminal == JobTerminalKind::Completed && self.mailbox_ref.is_some() {
            text.push_str(CONTINUABLE_SUBAGENT_HINT);
            text.push('\n');
        }
        text.push_str(&self.settlement_body());
        bounded_notice_text(&text)
    }

    /// 结算正文：子代理给结束语（空正文落地为固定替换句），Shell 作业给有界摘要。
    fn settlement_body(&self) -> String {
        match self.producer {
            JobProducerKind::Subagent => {
                let message = self.summary.trim();
                let message = if message.is_empty() {
                    NO_FINAL_MESSAGE
                } else {
                    message
                };
                format!("- 结束语：{message}\n")
            }
            JobProducerKind::Shell => match self.summary.trim() {
                "" => String::new(),
                preview => format!("- 摘要：{preview}\n"),
            },
        }
    }
}

/// 一次投递的聚合批次：单次不超过 `COMPLETION_NOTICE_MAX_PER_TURN` 条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionNoticeBatch {
    /// 本次投递的通知，按产生顺序排列。
    pub notices: Vec<CompletionNotice>,
    /// 因超出单次上限而被合并为计数摘要的条数。
    pub merged_count: usize,
}

impl CompletionNoticeBatch {
    /// 渲染为一条注入正文：批次内逐条渲染，超限部分只留计数摘要。
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = self
            .notices
            .iter()
            .map(CompletionNotice::render)
            .collect::<Vec<_>>()
            .join("\n\n");
        if self.merged_count > 0 {
            text.push_str(&format!(
                "\n\n另有 {} 个后台作业已完成，请用 job_output 逐个读取。",
                self.merged_count
            ));
        }
        bounded_notice_text(&text)
    }
}

/// 通知投递回调。
pub type CompletionNoticeHandler = Arc<dyn Fn(CompletionNoticeBatch) + Send + Sync>;

/// 订阅句柄：drop 即退订。
pub struct CompletionNoticeSubscription {
    subscription_id: u64,
}

impl Drop for CompletionNoticeSubscription {
    fn drop(&mut self) {
        let mut state = notice_bus()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .subscribers
            .retain(|subscriber| subscriber.subscription_id != self.subscription_id);
    }
}

struct NoticeSubscriber {
    subscription_id: u64,
    owner_id: String,
    handler: CompletionNoticeHandler,
}

struct DeliveredNoticeKey {
    key: String,
    delivered_at: Instant,
}

#[derive(Default)]
struct NoticeBusState {
    subscribers: Vec<NoticeSubscriber>,
    delivered: VecDeque<DeliveredNoticeKey>,
    delivered_keys: HashSet<String>,
    pending: HashMap<String, Vec<CompletionNotice>>,
}

static NOTICE_BUS: LazyLock<Mutex<NoticeBusState>> =
    LazyLock::new(|| Mutex::new(NoticeBusState::default()));
static NOTICE_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(1);

fn notice_bus() -> &'static Mutex<NoticeBusState> {
    &NOTICE_BUS
}

/// 订阅指定 owner 的临时 Job 完成通知。
pub fn subscribe_completion_notices(
    owner_id: &str,
    handler: CompletionNoticeHandler,
) -> CompletionNoticeSubscription {
    let subscription_id = NOTICE_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed);
    notice_bus()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .subscribers
        .push(NoticeSubscriber {
            subscription_id,
            owner_id: owner_id.to_owned(),
            handler,
        });
    CompletionNoticeSubscription { subscription_id }
}

/// 生产一条完成通知。
///
/// 同一 `notice_key` 只投递一次：重复生产在去重窗口内被静默忽略，
/// 因此「同一终态重复投递只展示一次」是结构性保证而不是调用方约定。
pub fn publish_completion_notice(notice: CompletionNotice) {
    publish_completion_notices(vec![notice]);
}

/// 批量生产完成通知：同一轮次内产生的多条通知合并为**一次**投递。
///
/// 批量入口是单次条数上限真正生效的地方：一次投递最多携带
/// `COMPLETION_NOTICE_MAX_PER_TURN` 条，其余只留计数摘要，因此不存在无界入队。
pub fn publish_completion_notices(notices: Vec<CompletionNotice>) {
    let deliveries = {
        let mut state = notice_bus()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_delivered(&mut state, Instant::now());
        for notice in notices {
            if !state.delivered_keys.insert(notice.notice_key.clone()) {
                continue;
            }
            state.delivered.push_back(DeliveredNoticeKey {
                key: notice.notice_key.clone(),
                delivered_at: Instant::now(),
            });
            state
                .pending
                .entry(notice.owner_id.clone())
                .or_default()
                .push(notice);
        }
        take_deliveries(&mut state)
    };
    for (handler, batch) in deliveries {
        handler(batch);
    }
}

/// 取出本轮全部待投递批次：每个 owner 至多一批，且不超过单次条数上限。
fn take_deliveries(
    state: &mut NoticeBusState,
) -> Vec<(CompletionNoticeHandler, CompletionNoticeBatch)> {
    let owners = state.pending.keys().cloned().collect::<Vec<_>>();
    let mut deliveries = Vec::new();
    for owner in owners {
        let Some(pending) = state.pending.remove(&owner) else {
            continue;
        };
        if pending.is_empty() {
            continue;
        }
        let handlers = state
            .subscribers
            .iter()
            .filter(|subscriber| subscriber.owner_id == owner)
            .map(|subscriber| Arc::clone(&subscriber.handler))
            .collect::<Vec<_>>();
        let mut notices = pending;
        let merged_count = notices.len().saturating_sub(COMPLETION_NOTICE_MAX_PER_TURN);
        notices.truncate(COMPLETION_NOTICE_MAX_PER_TURN);
        let batch = CompletionNoticeBatch {
            notices,
            merged_count,
        };
        for handler in handlers {
            deliveries.push((handler, batch.clone()));
        }
    }
    deliveries
}

/// 剪除越过保留窗口或超出容量的去重键。
fn prune_delivered(state: &mut NoticeBusState, now: Instant) {
    while state
        .delivered
        .front()
        .is_some_and(|entry| now.duration_since(entry.delivered_at) >= COMPLETION_NOTICE_RETENTION)
    {
        if let Some(entry) = state.delivered.pop_front() {
            state.delivered_keys.remove(&entry.key);
        }
    }
    while state.delivered.len() > COMPLETION_NOTICE_DEDUP_CAPACITY {
        if let Some(entry) = state.delivered.pop_front() {
            state.delivered_keys.remove(&entry.key);
        }
    }
}

fn bounded_text(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn bounded_notice_text(text: &str) -> String {
    if text.chars().count() <= COMPLETION_NOTICE_MAX_CHARS {
        return text.to_owned();
    }
    let head_chars =
        COMPLETION_NOTICE_MAX_CHARS.saturating_sub(NOTICE_TRUNCATION_MARKER.chars().count());
    let head = text.chars().take(head_chars).collect::<String>();
    format!("{head}{NOTICE_TRUNCATION_MARKER}")
}

#[cfg(test)]
#[path = "notice_tests.rs"]
mod tests;
