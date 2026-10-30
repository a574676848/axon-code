//! 飞书进度投影的有界命令邮箱与回合收口结算。
//!
//! 硬约束（与 `remote_feishu_progress_service` 的模块文档一致）：
//! - 入队永不阻塞，满载时淘汰最旧的**可淘汰**命令；`Finish` 是收口屏障，绝不被淘汰；
//! - `Finish` 优先出队：回合收口不能被其他 chat 的投影事件排在其前面；
//! - 收口结算只能被消费一次：调用方超时弃权后，worker 不得再写飞书，否则同一正文
//!   会与 durable outbox 的补偿回复重复投递。

use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
};

use super::remote_feishu_progress_model::ProjectionEvent;
use super::remote_feishu_progress_projection::CardStatus;
use super::remote_feishu_progress_service::{ProgressTurn, TurnProjection};

/// 邮箱容量上限：满时优先淘汰最旧的未投影增量。
const PROGRESS_MAILBOX_CAPACITY: usize = 512;

/// 结算状态：尚未进入结算。
const SETTLEMENT_ACTIVE: u8 = 0;
/// 结算状态：worker 已取得写飞书的权利，调用方超时也不能再撤回。
const SETTLEMENT_CLAIMED: u8 = 1;
/// 结算状态：调用方已超时弃权，由 durable outbox 独占投递。
const SETTLEMENT_ABORTED: u8 = 2;

/// 投影邮箱命令。
pub(super) enum ProgressCommand {
    /// 已提交 wire 事件归一出的投影事件。
    Event {
        /// 回合身份。
        turn: ProgressTurn,
        /// 投影事件。
        event: ProjectionEvent,
    },
    /// 回合收口：强制 flush 后回报投影结论。
    Finish {
        /// 回合身份。
        turn: ProgressTurn,
        /// 终态阶段。
        status: CardStatus,
        /// 投影结论回执。
        ack: mpsc::Sender<TurnProjection>,
        /// 结算权。
        settlement: Arc<ProgressSettlement>,
    },
}

impl ProgressCommand {
    /// 命令优先级：数值越小越先出队。
    fn priority(&self) -> u8 {
        match self {
            Self::Finish { .. } => 0,
            Self::Event { .. } => 1,
        }
    }
}

/// 单次回合收口的结算权。
///
/// 这是 `finish` 与 worker 之间唯一的裁决点：worker 只有在取得结算权后才会写
/// 最终卡片；调用方超时弃权即让出结算权，worker 随即放弃写卡片，改为完全由
/// durable outbox 投递，因此「卡片已投影」与「outbox 已补偿」不会同时发生。
#[derive(Debug)]
pub(super) struct ProgressSettlement {
    state: AtomicU8,
}

impl ProgressSettlement {
    /// 创建处于「待结算」状态的结算权。
    pub(super) fn new() -> Self {
        Self {
            state: AtomicU8::new(SETTLEMENT_ACTIVE),
        }
    }

    /// worker 侧：取得写飞书的权利；返回 `false` 表示调用方已弃权。
    pub(super) fn begin(&self) -> bool {
        self.state
            .compare_exchange(
                SETTLEMENT_ACTIVE,
                SETTLEMENT_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// 调用方侧：超时弃权；返回 `false` 表示 worker 已开始写卡片，本次超时不可撤回。
    pub(super) fn abort(&self) -> bool {
        self.state
            .compare_exchange(
                SETTLEMENT_ACTIVE,
                SETTLEMENT_ABORTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

struct MailboxState {
    commands: VecDeque<ProgressCommand>,
    closed: bool,
}

/// 有界投影邮箱：入队永不阻塞，命令按优先级出队。
pub(super) struct ProgressMailbox {
    state: Mutex<MailboxState>,
    signal: Condvar,
}

impl ProgressMailbox {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(MailboxState {
                commands: VecDeque::new(),
                closed: false,
            }),
            signal: Condvar::new(),
        }
    }

    /// 无阻塞入队；返回是否入队成功。
    pub(super) fn try_push(&self, command: ProgressCommand) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return false;
        }
        if state.commands.len() >= PROGRESS_MAILBOX_CAPACITY {
            evict_oldest_delta(&mut state.commands);
        }
        state.commands.push_back(command);
        self.signal.notify_one();
        true
    }

    /// 取出一条最高优先级命令；邮箱封闭且已排空时返回 `None`。
    pub(super) fn pop(&self) -> Option<ProgressCommand> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(command) = take_highest_priority(&mut state.commands) {
                return Some(command);
            }
            if state.closed {
                return None;
            }
            state = self
                .signal
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// 取出优先级最高（同级按入队顺序最旧）的命令。
fn take_highest_priority(commands: &mut VecDeque<ProgressCommand>) -> Option<ProgressCommand> {
    let index = commands
        .iter()
        .enumerate()
        .min_by_key(|(index, command)| (command.priority(), *index))
        .map(|(index, _)| index)?;
    commands.remove(index)
}

/// 满载时淘汰最旧的正文增量。
///
/// 工具与思考行都是语义命令，不能丢；极端情况下队列只含这些命令时，本次入队会让
/// 队列超出容量一个单位，随后由 worker 顺序排空。
fn evict_oldest_delta(commands: &mut VecDeque<ProgressCommand>) {
    if let Some(index) = commands.iter().position(|command| {
        matches!(
            command,
            ProgressCommand::Event {
                event: ProjectionEvent::AssistantDelta { .. },
                ..
            }
        )
    }) {
        commands.remove(index);
    }
}

#[cfg(test)]
#[path = "remote_feishu_progress_mailbox_tests.rs"]
mod tests;
