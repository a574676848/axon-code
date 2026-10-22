//! 前台 Shell 会话的**空闲回收通知**。
//!
//! 会话在「距 spawn 或最后一次 `write_stdin` 触碰满 `SHELL_IDLE_TIMEOUT`」时被判为模型
//! 不再跟进：进程被终止（退出码 130、`cancelled=true`），并由本模块投递一次通知，
//! 经 steer 通道进入下一个 provider 请求：模型不必轮询也能知道「它发起的那条命令
//! 已经被回收」，而不是在若干轮之后才发现会话不可达。
//!
//! **与临时 Job 完成通知刻意分离**（`crate::job_runtime::notice`）：会话与作业是两条
//! 通道（`session_id` 与 `job_id` 不可互传），引用标签、后续可用工具（`write_stdin`
//! 与 `job_output`）与去重键都不同。复用同一个 tag 会让消费侧无法区分二者，也会让
//! 模型拿着 `session_id` 去调 `job_output`。
//!
//! 通知不是状态事实来源：会话终态仍以会话注册表为准，通知只负责唤醒与指路。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::runtime_limits::{
    SHELL_RECLAIM_NOTICE_COMMAND_MAX_CHARS, SHELL_RECLAIM_NOTICE_DEDUP_CAPACITY,
    SHELL_RECLAIM_NOTICE_MAX_CHARS, SHELL_RECLAIM_NOTICE_MAX_PENDING_PER_OWNER,
    SHELL_RECLAIM_NOTICE_RETENTION,
};

/// 正文超限时的截断标记；截断只丢正文，保留终态与引用。
const RECLAIM_TRUNCATION_MARKER: &str = "\n…（通知正文已截断）";

/// 一条前台会话回收通知。
///
/// 载荷必须自证归属：收到通知的 turn 能仅凭载荷判断「哪个会话、哪条命令、什么结局、
/// 去哪里读完整输出」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSessionReclaim {
    /// 去重键：会话 UUID（一个会话至多被回收一次）。
    pub notice_key: String,
    /// 归属：`shell_command` 的 owner，同时也是 steer 通道的注册键。
    pub owner_id: String,
    /// 会话 ID（`write_stdin` 的句柄，**不是** `job_id`）。
    pub session_id: String,
    /// 被回收的命令（有界）。
    pub command: String,
    /// 工作目录（有界）。
    pub cwd: String,
    /// 终态退出码；空闲回收固定为 130。
    pub exit_code: i32,
    /// 实际无轮询时长。
    pub idle: Duration,
    /// 完整输出归档路径。
    pub artifact_ref: String,
}

impl ShellSessionReclaim {
    /// 构造通知：文本字段在此统一收敛到有界长度。
    #[must_use]
    pub fn new(
        session_id: &str,
        owner_id: &str,
        command: &str,
        cwd: &str,
        exit_code: i32,
        idle: Duration,
        artifact_ref: &str,
    ) -> Self {
        Self {
            notice_key: session_id.to_owned(),
            owner_id: owner_id.to_owned(),
            session_id: session_id.to_owned(),
            command: bounded_text(command, SHELL_RECLAIM_NOTICE_COMMAND_MAX_CHARS),
            cwd: bounded_text(cwd, SHELL_RECLAIM_NOTICE_COMMAND_MAX_CHARS),
            exit_code,
            idle,
            artifact_ref: artifact_ref.to_owned(),
        }
    }

    /// 渲染为注入主 turn 的有界正文。
    ///
    /// 结构固定为「终态首行 → 会话引用 → 终态事实 → 完整输出 → 后续动作」：
    /// `session_id` 与归档路径**必须**在截断之前，否则模型无从接管。
    #[must_use]
    pub fn render(&self) -> String {
        let text = format!(
            "前台 Shell 会话已因空闲回收被终止：\n\
             - session_id：{session_id}\n\
             - 命令：{command}\n\
             - 工作目录：{cwd}\n\
             - 退出码：{exit_code}（cancelled=true）\n\
             - 回收原因：连续 {idle} 无任何 write_stdin 轮询\n\
             - 完整输出：{artifact}\n\
             该会话已终止：write_stdin 只能读取终态与保留窗口内的输出，不能再写入输入。\
             需要继续等待的长命令请重新调用 shell_command —— 预计超过空闲窗口时显式提高 \
             timeout_ms，或改用 run_in_background 后经 job_output 收取结果。",
            session_id = self.session_id,
            command = self.command,
            cwd = self.cwd,
            exit_code = self.exit_code,
            idle = humanized_duration(self.idle),
            artifact = self.artifact_ref,
        );
        if text.chars().count() <= SHELL_RECLAIM_NOTICE_MAX_CHARS {
            return text;
        }
        let head_chars = SHELL_RECLAIM_NOTICE_MAX_CHARS
            .saturating_sub(RECLAIM_TRUNCATION_MARKER.chars().count());
        format!(
            "{}{RECLAIM_TRUNCATION_MARKER}",
            text.chars().take(head_chars).collect::<String>()
        )
    }
}

/// 通知投递回调。
pub type ShellSessionReclaimHandler = Arc<dyn Fn(ShellSessionReclaim) + Send + Sync>;

/// 订阅句柄：drop 即退订。
pub struct ShellSessionReclaimSubscription {
    subscription_id: u64,
}

impl Drop for ShellSessionReclaimSubscription {
    fn drop(&mut self) {
        let mut state = reclaim_bus()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .subscribers
            .retain(|subscriber| subscriber.subscription_id != self.subscription_id);
    }
}

struct ReclaimSubscriber {
    subscription_id: u64,
    owner_id: String,
    handler: ShellSessionReclaimHandler,
}

#[derive(Default)]
struct ReclaimBusState {
    subscribers: Vec<ReclaimSubscriber>,
    /// 无订阅者时的暂存队列；同一个 run 稍后注册订阅即被冲刷。
    pending: HashMap<String, VecDeque<ShellSessionReclaim>>,
    /// 已投递通知的去重键与投递时刻，按投递顺序排列。
    delivered: VecDeque<(String, Instant)>,
    delivered_keys: HashSet<String>,
}

static RECLAIM_BUS: LazyLock<Mutex<ReclaimBusState>> =
    LazyLock::new(|| Mutex::new(ReclaimBusState::default()));
static RECLAIM_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(1);

fn reclaim_bus() -> &'static Mutex<ReclaimBusState> {
    &RECLAIM_BUS
}

/// 订阅指定 owner 的会话回收通知；订阅时冲刷该 owner 已暂存的通知。
pub fn subscribe_shell_session_reclaims(
    owner_id: &str,
    handler: ShellSessionReclaimHandler,
) -> ShellSessionReclaimSubscription {
    let subscription_id = RECLAIM_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed);
    let flushed = {
        let mut state = reclaim_bus()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.subscribers.push(ReclaimSubscriber {
            subscription_id,
            owner_id: owner_id.to_owned(),
            handler: Arc::clone(&handler),
        });
        state.pending.remove(owner_id).unwrap_or_default()
    };
    for notice in flushed {
        handler(notice);
    }
    ShellSessionReclaimSubscription { subscription_id }
}

/// 生产一条会话回收通知。
///
/// 同一 `notice_key` 只投递一次：重复生产在保留窗口内被静默忽略，
/// 因此「同一个会话只通知一次」是结构性保证而不是调用方约定。
/// 无订阅者时通知进入暂存队列（按 owner 有界），订阅方注册后立即收到。
pub(crate) fn publish_shell_session_reclaim(notice: ShellSessionReclaim) {
    let handlers = {
        let mut state = reclaim_bus()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_delivered(&mut state, Instant::now());
        if !state.delivered_keys.insert(notice.notice_key.clone()) {
            return;
        }
        state
            .delivered
            .push_back((notice.notice_key.clone(), Instant::now()));
        let handlers = state
            .subscribers
            .iter()
            .filter(|subscriber| subscriber.owner_id == notice.owner_id)
            .map(|subscriber| Arc::clone(&subscriber.handler))
            .collect::<Vec<_>>();
        if handlers.is_empty() {
            let pending = state.pending.entry(notice.owner_id.clone()).or_default();
            pending.push_back(notice);
            while pending.len() > SHELL_RECLAIM_NOTICE_MAX_PENDING_PER_OWNER {
                pending.pop_front();
            }
            return;
        }
        handlers
    };
    for handler in handlers {
        handler(notice.clone());
    }
}

/// 剪除越过保留窗口或超出容量的去重键。
fn prune_delivered(state: &mut ReclaimBusState, now: Instant) {
    while state.delivered.front().is_some_and(|(_, delivered_at)| {
        now.duration_since(*delivered_at) >= SHELL_RECLAIM_NOTICE_RETENTION
    }) {
        if let Some((key, _)) = state.delivered.pop_front() {
            state.delivered_keys.remove(&key);
        }
    }
    while state.delivered.len() > SHELL_RECLAIM_NOTICE_DEDUP_CAPACITY {
        if let Some((key, _)) = state.delivered.pop_front() {
            state.delivered_keys.remove(&key);
        }
    }
}

fn bounded_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let head = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{head}…")
}

/// 人读时长：不足一分钟按秒，其余按分钟取整（只用于通知正文）。
fn humanized_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds} 秒")
    } else {
        format!("{} 分钟", seconds / 60)
    }
}

#[cfg(test)]
#[path = "session_reclaim_tests.rs"]
mod tests;
