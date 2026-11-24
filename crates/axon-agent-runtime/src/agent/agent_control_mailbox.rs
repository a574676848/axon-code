//! 子代理控制面的路径化 mailbox。

use std::collections::{HashMap, VecDeque};

use axon_domain::{AxonError, AxonResult};

use super::agent_mailbox_persistence::{
    AgentMailboxDeliveryState, AgentMailboxMessageKind, MailboxPersistOutcome,
    bounded_mailbox_content,
};
use crate::agent_communication::AgentStatus;
use crate::agent_control_runtime::{
    AGENT_CONTROL_REASON_EMPTY_MESSAGE, AGENT_CONTROL_REASON_NOT_FOUND,
    AGENT_CONTROL_REASON_RECIPIENT_UNAVAILABLE, AgentControlRuntime, AgentPath,
};

/// agent 间通信触发模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMessageTrigger {
    /// 只入队，不触发目标 turn。
    QueueOnly,
    /// 入队并标记目标可运行。
    TriggerTurn,
}

impl AgentMessageTrigger {
    /// 是否触发目标 turn。
    #[must_use]
    pub const fn should_trigger_turn(self) -> bool {
        matches!(self, Self::TriggerTurn)
    }
}

/// agent 间通信消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterAgentCommunication {
    /// 持久记录 ID；未绑定持久化的纯内存消息为 `None`。
    ///
    /// 它让「取走消息」与「读确认」能在同一步完成：没有它就无法把内存队列里的
    /// 条目对回持久记录，读态也就无从推进。
    pub message_id: Option<String>,
    /// 发送方路径。
    pub author: AgentPath,
    /// 接收方路径。
    pub recipient: AgentPath,
    /// 明文内容。
    pub content: String,
    /// 是否触发目标 turn。
    pub trigger_turn: bool,
    /// 创建时间。
    pub created_at_ms: u64,
}

/// 发送消息请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessageRequest {
    /// 发送方路径。
    pub author: AgentPath,
    /// 接收方路径。
    pub recipient: AgentPath,
    /// 消息内容。
    pub content: String,
    /// 触发策略。
    pub trigger: AgentMessageTrigger,
    /// 创建时间。
    pub created_at_ms: u64,
    /// 消息类别；决定幂等键的作用域前缀。
    pub kind: AgentMailboxMessageKind,
}

/// 发送消息报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessageReport {
    /// 发送方路径。
    pub author: AgentPath,
    /// 接收方路径。
    pub recipient: AgentPath,
    /// 是否触发目标 turn。
    pub trigger_turn: bool,
    /// 接收方 mailbox 剩余消息数。
    pub queued_count: usize,
    /// 接收方当前状态。
    pub recipient_status: AgentStatus,
    /// 本次投递结果；接收方暂时不可接收时为 `Pending`，消息留在持久表中等待重试。
    pub delivery_state: AgentMailboxDeliveryState,
    /// 是否需要调用方唤醒目标（合同 §5.3.5 的第二语义）。
    ///
    /// 目标合法但当前空闲（已终态、尚未关闭）时为 `true`：消息已经进了它的 mailbox，
    /// 但**只有宿主**能重挂会话、开启新轮次：内核不持有执行器。调用方唤醒失败时
    /// 必须把这次投递记为失败，让它退回待投递状态而不是悬置。
    pub wake_required: bool,
}

/// mailbox 快照；供控制面自身与测试观察 mailbox 内容，**不消费**消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMailboxSnapshot {
    /// 当前代理路径。
    pub agent: AgentPath,
    /// 当前待消费消息。
    pub messages: Vec<InterAgentCommunication>,
    /// 是否观察到 mailbox 活动。
    pub mailbox_activity: bool,
    /// 当前代理状态。
    pub status: AgentStatus,
}

/// 控制面 mailbox 存储。
///
/// 等待与唤醒语义不再由本存储承载：子代理的等待与终止已统一到临时 Job 生命周期
/// （`job_output` / `job_kill`），mailbox 只负责投递与读取。
#[derive(Debug, Clone, Default)]
pub struct AgentMailboxStore {
    mailboxes: HashMap<AgentPath, VecDeque<InterAgentCommunication>>,
    /// 是否已经完成过一次持久装回。
    ///
    /// 装回是「把持久表里的消息推进内存队列」的**增量**动作，重复执行会把同一条消息
    /// 推入多次；因此它必须是一次性的，标志随存储实例走（拓扑恢复会新建实例，
    /// 新实例自然回到未装回状态）。
    hydrated: bool,
}

impl AgentMailboxStore {
    /// 是否已经完成过一次持久装回。
    #[must_use]
    pub const fn is_hydrated(&self) -> bool {
        self.hydrated
    }

    /// 标记持久装回已完成。
    pub fn mark_hydrated(&mut self) {
        self.hydrated = true;
    }

    /// 确保 agent mailbox 存在。
    pub fn ensure_agent(&mut self, path: AgentPath) {
        self.mailboxes.entry(path).or_default();
    }

    /// 移除 agent mailbox。
    pub fn remove_agent(&mut self, path: &AgentPath) {
        self.mailboxes.remove(path);
    }

    /// 推入消息。
    pub fn push(&mut self, path: AgentPath, communication: InterAgentCommunication) -> usize {
        let mailbox = self.mailboxes.entry(path).or_default();
        mailbox.push_back(communication);
        mailbox.len()
    }

    /// 待消费消息数。
    #[must_use]
    pub fn len(&self, path: &AgentPath) -> usize {
        self.mailboxes.get(path).map_or(0, VecDeque::len)
    }

    /// 待消费消息快照。
    #[must_use]
    pub fn messages(&self, path: &AgentPath) -> Vec<InterAgentCommunication> {
        self.mailboxes
            .get(path)
            .map(|mailbox| mailbox.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 取走某代理的全部待读消息。
    ///
    /// **这是 mailbox 的唯一消费出口**：队列被整体 drain，因此同一批消息不可能
    /// 同时从两条通道进入上下文（M3 设计 §7.5 的明确关切）。
    pub fn drain(&mut self, path: &AgentPath) -> Vec<InterAgentCommunication> {
        self.mailboxes
            .get_mut(path)
            .map(std::mem::take)
            .map(VecDeque::into_iter)
            .map(Iterator::collect)
            .unwrap_or_default()
    }
}

impl AgentControlRuntime {
    /// 发送 agent 间消息并写入接收方 mailbox。
    ///
    /// 落盘先于投递（若绑定了持久化实现）：进程在投递前崩溃也不会丢消息，
    /// 重启后由 [`AgentControlRuntime::hydrate_mailbox`] 重新装回内存 mailbox。
    ///
    /// 目标状态分三类，语义各不相同：
    ///
    /// - **不存在**：非法目标，稳定拒绝（合同 §5.2.2 由协作工具层统一裁决，这里是兜底）；
    /// - **暂时不可接收**（已终态或已关闭）：消息落为待投递并记录一次失败，
    ///   由重试推进，而不是丢弃（合同 §5.3.2、§8.2.3）；
    /// - **可接收**：写入内存 mailbox 并把持久记录结算为已投递。
    pub fn send_message(&mut self, request: AgentMessageRequest) -> AxonResult<AgentMessageReport> {
        if request.content.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                AGENT_CONTROL_REASON_EMPTY_MESSAGE,
            ));
        }
        if !self.has_agent(&request.author) {
            return Err(AxonError::runtime_failure(AGENT_CONTROL_REASON_NOT_FOUND));
        }
        let recipient_status = self.status(&request.recipient);
        if recipient_status == AgentStatus::NotFound {
            return Err(AxonError::runtime_failure(AGENT_CONTROL_REASON_NOT_FOUND));
        }
        let content = bounded_mailbox_content(&request.content);
        let trigger_turn = request.trigger.should_trigger_turn();
        let created_at_ms = request.created_at_ms;
        let outcome = self.persist_mailbox_message(
            &request.author,
            &request.recipient,
            request.kind,
            &content,
            trigger_turn,
            created_at_ms,
        )?;

        // 幂等命中：这次投递是重复的。不追加内存 mailbox、也不记录新的失败，
        // 只如实回报既有记录的结局（合同 §5.3.1「同幂等键重复投递只执行一次」）。
        if let MailboxPersistOutcome::Duplicate(state) = outcome {
            return Ok(AgentMessageReport {
                author: request.author,
                queued_count: self.mailboxes.len(&request.recipient),
                recipient: request.recipient,
                trigger_turn,
                recipient_status,
                delivery_state: state,
                wake_required: false,
            });
        }
        let message_id = match &outcome {
            MailboxPersistOutcome::Persisted(message_id) => Some(message_id.as_str()),
            _ => None,
        };

        let (queued_count, delivery_state, wake_required) =
            match recipient_acceptance(&recipient_status) {
                RecipientAcceptance::Immediate => {
                    let queued_count = self.deliver_into_mailbox(
                        &request.recipient,
                        &request.author,
                        content,
                        message_id,
                        trigger_turn,
                        created_at_ms,
                    )?;
                    (queued_count, AgentMailboxDeliveryState::Delivered, false)
                }
                RecipientAcceptance::Recoverable => {
                    let queued_count = self.deliver_into_mailbox(
                        &request.recipient,
                        &request.author,
                        content,
                        message_id,
                        trigger_turn,
                        created_at_ms,
                    )?;
                    (queued_count, AgentMailboxDeliveryState::Delivered, true)
                }
                RecipientAcceptance::Unavailable => {
                    if let Some(message_id) = message_id {
                        self.record_mailbox_delivery_failure(
                            message_id,
                            AGENT_CONTROL_REASON_RECIPIENT_UNAVAILABLE,
                            created_at_ms,
                        );
                    }
                    (
                        self.mailboxes.len(&request.recipient),
                        AgentMailboxDeliveryState::Pending,
                        false,
                    )
                }
            };

        Ok(AgentMessageReport {
            author: request.author,
            recipient: request.recipient,
            trigger_turn,
            queued_count,
            recipient_status,
            delivery_state,
            wake_required,
        })
    }

    /// 把消息写入接收方内存 mailbox 并把持久记录结算为已投递。
    fn deliver_into_mailbox(
        &mut self,
        recipient: &AgentPath,
        author: &AgentPath,
        content: String,
        message_id: Option<&str>,
        trigger_turn: bool,
        created_at_ms: u64,
    ) -> AxonResult<usize> {
        self.update_recipient_after_message(recipient, &content)?;
        let communication = InterAgentCommunication {
            message_id: message_id.map(str::to_owned),
            author: author.clone(),
            recipient: recipient.clone(),
            content,
            trigger_turn,
            created_at_ms,
        };
        let queued_count = self.mailboxes.push(recipient.clone(), communication);
        if let Some(message_id) = message_id {
            self.mark_mailbox_delivered(message_id, created_at_ms);
        }
        Ok(queued_count)
    }

    /// 读取 agent mailbox 快照；不消费消息。
    pub fn mailbox_snapshot(&self, agent: &AgentPath) -> AxonResult<AgentMailboxSnapshot> {
        let status = self.status(agent);
        if status == AgentStatus::NotFound {
            return Err(AxonError::runtime_failure(AGENT_CONTROL_REASON_NOT_FOUND));
        }
        let messages = self.mailboxes.messages(agent);
        Ok(AgentMailboxSnapshot {
            agent: agent.clone(),
            mailbox_activity: !messages.is_empty(),
            messages,
            status,
        })
    }

    /// 查看 mailbox 中待消费消息数。
    #[must_use]
    pub fn mailbox_len(&self, agent: &AgentPath) -> usize {
        self.mailboxes.len(agent)
    }
}

/// 接收方对本次投递的准入裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecipientAcceptance {
    /// 可即时接收：代理正在运行或待初始化。
    Immediate,
    /// 合法但当前空闲：消息可以送达，但需要调用方唤醒它开启新轮次（合同 §5.3.5）。
    Recoverable,
    /// 不可达：已关闭或已回收；消息落为待投递并等待重试（合同 §5.3.2）。
    Unavailable,
}

/// 按接收方状态裁决本次投递的准入。
///
/// 三分类是 §5.3.5 三语义的判据来源：`Immediate` 是「运行中，最近 step 边界
/// steer 注入」，`Recoverable` 是「空闲/已恢复，开启新轮次」，
/// `Unavailable` 是「不可达，降级为排队」。
pub(crate) fn recipient_acceptance(status: &AgentStatus) -> RecipientAcceptance {
    match status {
        AgentStatus::PendingInit | AgentStatus::Running => RecipientAcceptance::Immediate,
        AgentStatus::Completed(_)
        | AgentStatus::Interrupted
        | AgentStatus::Errored(_)
        | AgentStatus::Refused(_) => RecipientAcceptance::Recoverable,
        AgentStatus::Shutdown | AgentStatus::NotFound => RecipientAcceptance::Unavailable,
    }
}

/// 接收方当前是否可接收消息。
///
/// 只认「可即时接收」：重试循环没有执行器，无法唤醒空闲目标；把空闲目标也算作
/// 可接收，重试就会把消息投进一个永远不会被读取的 mailbox：那不是投递成功，
/// 而是静默丢失（合同 §5.3.2）。空闲目标的唤醒由 `send_message` 的调用方承担。
pub(crate) fn recipient_accepts_messages(status: &AgentStatus) -> bool {
    matches!(recipient_acceptance(status), RecipientAcceptance::Immediate)
}
