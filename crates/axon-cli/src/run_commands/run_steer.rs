//! 运行中用户输入的 turn-local FIFO 注入队列。

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use axon_agent_runtime::{
    AgentControlRuntime, AgentMailboxMessageKind, AgentMessageRequest, AgentMessageTrigger,
    AgentModelMessage, AgentPath, AgentRuntimeNoteKind, AgentTurnSteerInput, AgentTurnSteerSource,
    RunCancelToken,
};
use axon_domain::InteractionId;

static RUN_DEFERRED_MESSAGE_REGISTRY: OnceLock<Mutex<HashMap<String, RegisteredDeferredMessage>>> =
    OnceLock::new();
static RUN_STEER_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);

const RUN_STEER_PENDING_MESSAGE_LIMIT: usize = 64;
const RUN_STEER_PENDING_BYTE_LIMIT: usize = 256 * 1024;
const ACTIVE_PRIMARY_TURN_CONFLICT_ERROR: &str = "同一会话已存在活跃主 turn";
const ACTIVE_PRIMARY_TURN_CAPACITY_ERROR: &str = "活跃主 turn steer 队列已满";

#[derive(Clone)]
struct RegisteredDeferredMessage {
    registration_id: u64,
    inbox: RunSteerInbox,
}

#[derive(Debug, Default)]
struct RunSteerState {
    accepting: bool,
    pending: VecDeque<AgentTurnSteerInput>,
    pending_bytes: usize,
}

#[derive(Clone, Default)]
pub(super) struct RunSteerBridge {
    state: Arc<Mutex<RunSteerState>>,
}

impl RunSteerBridge {
    pub(super) fn open(&self) -> RunSteerInbox {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.clear();
        state.pending_bytes = 0;
        state.accepting = true;
        RunSteerInbox {
            state: Arc::clone(&self.state),
            subagents: SubagentChannel::default(),
            closes_on_drop: true,
        }
    }

    #[cfg(test)]
    pub(super) fn try_send(&self, text: String) -> Result<(), RunSteerSendError> {
        try_send_input(
            &self.state,
            AgentTurnSteerInput {
                message: AgentModelMessage::text("user", text),
                quoted_message: None,
            },
        )
    }
}

fn try_send_input(
    state: &Arc<Mutex<RunSteerState>>,
    input: AgentTurnSteerInput,
) -> Result<(), RunSteerSendError> {
    let mut state = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !state.accepting {
        return Err(RunSteerSendError::Inactive);
    }
    let input_bytes = steer_input_size_bytes(&input);
    let pending_bytes = state.pending_bytes.saturating_add(input_bytes);
    if state.pending.len() >= RUN_STEER_PENDING_MESSAGE_LIMIT
        || pending_bytes > RUN_STEER_PENDING_BYTE_LIMIT
    {
        return Err(RunSteerSendError::CapacityExceeded);
    }
    state.pending.push_back(input);
    state.pending_bytes = pending_bytes;
    Ok(())
}

fn steer_input_size_bytes(input: &AgentTurnSteerInput) -> usize {
    let message_bytes = serde_json::to_vec(&input.message).map_or(usize::MAX, |value| value.len());
    let quoted_bytes = input.quoted_message.as_ref().map_or(0, |quoted| {
        serde_json::to_vec(quoted).map_or(usize::MAX, |value| value.len())
    });
    message_bytes.saturating_add(quoted_bytes)
}

pub(crate) struct RunDeferredMessageRegistration {
    session_key: String,
    registration_id: u64,
}

impl Drop for RunDeferredMessageRegistration {
    fn drop(&mut self) {
        let mut registry = run_deferred_message_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registry
            .get(&self.session_key)
            .is_some_and(|entry| entry.registration_id == self.registration_id)
        {
            registry.remove(&self.session_key);
        }
    }
}

pub(crate) fn register_deferred_message_session(
    session_key: &str,
    inbox: RunSteerInbox,
) -> RunDeferredMessageRegistration {
    let registration_id = RUN_STEER_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed);
    run_deferred_message_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            session_key.to_owned(),
            RegisteredDeferredMessage {
                registration_id,
                inbox,
            },
        );
    RunDeferredMessageRegistration {
        session_key: session_key.to_owned(),
        registration_id,
    }
}

pub(crate) fn try_send_deferred_runtime_note(
    session_key: &str,
    kind: AgentRuntimeNoteKind,
    content: String,
) -> Result<(), RunSteerSendError> {
    let registry = run_deferred_message_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let inbox = &registry
        .get(session_key)
        .ok_or(RunSteerSendError::Inactive)?
        .inbox;
    inbox.try_send_input(AgentTurnSteerInput {
        message: AgentModelMessage::runtime_note(kind, content),
        quoted_message: None,
    })
}

fn run_deferred_message_registry() -> &'static Mutex<HashMap<String, RegisteredDeferredMessage>> {
    RUN_DEFERRED_MESSAGE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunSteerSendError {
    Inactive,
    CapacityExceeded,
}

/// 子代理投递通道。
///
/// 用户在子代理视图里发的消息不是「写给当前 turn 的补充要求」，而是写给某个具体
/// 子代理的协作消息：因此它经 mailbox 投递，由目标子代理在自己的 step 边界消费。
/// 控制面句柄在装配层建立起 agent 控制面之后装上（见 [`Self::install`]），
/// 在那之前通道一律**拒绝**投递，而不是降级成主代理输入。
#[derive(Clone, Default)]
pub(crate) struct SubagentChannel {
    control: Arc<Mutex<Option<Arc<Mutex<AgentControlRuntime>>>>>,
}

/// 一次子代理投递的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubagentSendOutcome {
    /// 这条车道上没有活跃主 turn，无从投递。
    NoActiveTurn,
    /// 已进入目标 mailbox。
    Delivered {
        /// 目标当前状态标签。
        status: String,
        /// 是否需要在目标侧唤醒（空闲目标才需要，宿主暂不承担唤醒）。
        wake_required: bool,
    },
    /// 明确拒绝，附可读原因。
    Rejected(String),
}

impl SubagentChannel {
    /// 装上本轮的控制面句柄。
    pub(crate) fn install(&self, control: Arc<Mutex<AgentControlRuntime>>) {
        let mut slot = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(control);
    }

    /// 投递一条用户消息给某个子代理。
    ///
    /// 同步返回真实结论：目标不存在、目标当前不可接收、或已进入 mailbox。
    /// 空闲目标需要宿主唤醒，本入口暂不承担，因此如实标记 `wake_required`，
    /// 由调用方决定是否把这条当成「已受理」。
    fn deliver(&self, agent: &str, content: &str) -> SubagentSendOutcome {
        if content.trim().is_empty() {
            return SubagentSendOutcome::Rejected("消息内容不能为空".to_owned());
        }
        let control = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(control) = control else {
            return SubagentSendOutcome::Rejected("本轮子代理通道尚未就绪".to_owned());
        };
        let recipient = match AgentPath::parse(agent) {
            Ok(path) => path,
            Err(_) => return SubagentSendOutcome::Rejected(format!("子代理标识非法：{agent}")),
        };
        let mut control = control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match control.send_message(AgentMessageRequest {
            author: AgentPath::root(),
            recipient,
            content: content.to_owned(),
            trigger: AgentMessageTrigger::QueueOnly,
            created_at_ms: now_millis(),
            kind: AgentMailboxMessageKind::Collab,
        }) {
            Ok(report) => SubagentSendOutcome::Delivered {
                status: format!("{:?}", report.recipient_status),
                wake_required: report.wake_required,
            },
            // 内核的错误类型没有可读文案；能走到这里的只有「目标不可达」一类事实，
            // 因此给一句用户能理解、也能据此行动的结论，而不是内部调试串。
            Err(_) => SubagentSendOutcome::Rejected(
                "该子代理已结束或不在当前控制面内，无法接收消息".to_owned(),
            ),
        }
    }
}

/// 当前墙钟毫秒。
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

pub(crate) struct RunSteerInbox {
    state: Arc<Mutex<RunSteerState>>,
    /// 子代理投递通道；由装配层在本轮建立起控制面之后装上句柄。
    subagents: SubagentChannel,
    closes_on_drop: bool,
}

impl Clone for RunSteerInbox {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            subagents: self.subagents.clone(),
            closes_on_drop: false,
        }
    }
}

impl Drop for RunSteerInbox {
    fn drop(&mut self) {
        if self.closes_on_drop {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.accepting = false;
            state.pending.clear();
            state.pending_bytes = 0;
        }
    }
}

impl AgentTurnSteerSource for RunSteerInbox {
    fn drain(&self) -> Vec<AgentTurnSteerInput> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drain_pending(&mut state)
    }

    fn close_and_drain(&self) -> Vec<AgentTurnSteerInput> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting = false;
        drain_pending(&mut state)
    }

    fn reopen(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .accepting = true;
    }
}

fn drain_pending(state: &mut RunSteerState) -> Vec<AgentTurnSteerInput> {
    state.pending_bytes = 0;
    state.pending.drain(..).collect()
}

impl RunSteerInbox {
    #[cfg(test)]
    pub(super) fn try_send_message(
        &self,
        message: AgentModelMessage,
    ) -> Result<(), RunSteerSendError> {
        self.try_send_input(AgentTurnSteerInput {
            message,
            quoted_message: None,
        })
    }

    /// 子代理投递通道句柄；装配层用它装上本轮的控制面。
    pub(crate) fn subagent_channel(&self) -> SubagentChannel {
        self.subagents.clone()
    }

    /// 投递一条写给子代理的用户消息。
    ///
    /// 与主代理的 `try_send` 是两条互不影响的通道：写给子代理的话不进主代理上下文，
    /// 通道未就绪时拒绝，绝不降级。
    pub(crate) fn try_send_subagent_message(
        &self,
        agent: &str,
        content: &str,
    ) -> SubagentSendOutcome {
        if !self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .accepting
        {
            return SubagentSendOutcome::NoActiveTurn;
        }
        self.subagents.deliver(agent, content)
    }

    fn try_send_input(&self, input: AgentTurnSteerInput) -> Result<(), RunSteerSendError> {
        try_send_input(&self.state, input)
    }
}

#[derive(Clone)]
struct RegisteredPrimaryTurn {
    registration_id: u64,
    interaction_id: InteractionId,
    inbox: RunSteerInbox,
    /// 本轮 provider 执行的取消令牌；网关中断就是把它置位。
    cancel_token: RunCancelToken,
}

#[derive(Clone, Default)]
pub(crate) struct ActivePrimaryTurnRegistry {
    entries: Arc<Mutex<HashMap<String, RegisteredPrimaryTurn>>>,
}

impl ActivePrimaryTurnRegistry {
    pub(crate) fn register(
        &self,
        affinity_key: &str,
        interaction_id: InteractionId,
        inbox: RunSteerInbox,
        cancel_token: RunCancelToken,
    ) -> Result<ActivePrimaryTurnRegistration, String> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.contains_key(affinity_key) {
            return Err(ACTIVE_PRIMARY_TURN_CONFLICT_ERROR.to_owned());
        }
        let registration_id = RUN_STEER_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed);
        entries.insert(
            affinity_key.to_owned(),
            RegisteredPrimaryTurn {
                registration_id,
                interaction_id,
                inbox,
                cancel_token,
            },
        );
        Ok(ActivePrimaryTurnRegistration {
            affinity_key: affinity_key.to_owned(),
            registration_id,
            entries: Arc::clone(&self.entries),
        })
    }

    /// 当前进行中的 Primary Turn 数量；用于指标统计与诊断测试。
    pub(crate) fn active_count(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub(crate) fn try_send(
        &self,
        affinity_key: &str,
        content: String,
    ) -> Result<Option<InteractionId>, String> {
        let turn = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(affinity_key)
            .cloned();
        let Some(turn) = turn else {
            return Ok(None);
        };
        match turn.inbox.try_send_input(AgentTurnSteerInput {
            message: AgentModelMessage::text("user", content),
            quoted_message: None,
        }) {
            Ok(()) => Ok(Some(turn.interaction_id)),
            Err(RunSteerSendError::Inactive) => Ok(None),
            Err(RunSteerSendError::CapacityExceeded) => {
                Err(ACTIVE_PRIMARY_TURN_CAPACITY_ERROR.to_owned())
            }
        }
    }

    /// 向本轮某个子代理投递一条用户消息。
    ///
    /// 返回值来自通道本身：没有活跃主 turn、已进入目标 mailbox、或被明确拒绝。
    pub(crate) fn try_send_subagent(
        &self,
        affinity_key: &str,
        agent: &str,
        content: &str,
    ) -> SubagentSendOutcome {
        let turn = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(affinity_key)
            .cloned();
        let Some(turn) = turn else {
            return SubagentSendOutcome::NoActiveTurn;
        };
        turn.inbox.try_send_subagent_message(agent, content)
    }

    pub(crate) fn affinity_key(primary_lane_id: &str) -> String {
        primary_lane_id.to_owned()
    }

    /// 中断某条车道上正在运行的主 turn。
    ///
    /// 取消令牌是既有机制：置位后 provider 传输、重试退避与工具执行的所有取消点
    /// 都会立即收口，本轮按「已中断」结束。返回被中断的 Interaction 标识；
    /// `Ok(None)` 表示该车道当前没有活跃主 turn，调用方应如实回执而不是假装成功。
    pub(crate) fn interrupt(&self, affinity_key: &str) -> Result<Option<InteractionId>, String> {
        let turn = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(affinity_key)
            .cloned();
        let Some(turn) = turn else {
            return Ok(None);
        };
        // 先把 entry 取出锁外再置位：取消会同步唤醒等待中的 provider 流，
        // 持锁调用会让并发的注册与注销一起阻塞。
        turn.cancel_token.cancel();
        Ok(Some(turn.interaction_id))
    }
}

pub(crate) struct ActivePrimaryTurnRegistration {
    affinity_key: String,
    registration_id: u64,
    entries: Arc<Mutex<HashMap<String, RegisteredPrimaryTurn>>>,
}

impl Drop for ActivePrimaryTurnRegistration {
    fn drop(&mut self) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(&self.affinity_key)
            .is_some_and(|entry| entry.registration_id == self.registration_id)
        {
            entries.remove(&self.affinity_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_and_drain_atomically_rejects_late_input_until_reopened() {
        let bridge = RunSteerBridge::default();
        let inbox = bridge.open();
        bridge.try_send("第一条".to_owned()).unwrap();

        let drained = inbox.close_and_drain();

        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].message.content, "第一条");
        assert_eq!(
            bridge.try_send("过晚输入".to_owned()),
            Err(RunSteerSendError::Inactive)
        );
        inbox.reopen();
        bridge.try_send("第二条".to_owned()).unwrap();
        assert_eq!(inbox.drain()[0].message.content, "第二条");
    }

    #[test]
    fn cloned_inbox_does_not_extend_or_close_active_lifecycle() {
        let bridge = RunSteerBridge::default();
        let inbox = bridge.open();
        let observer = inbox.clone();

        drop(observer);
        bridge.try_send("仍在运行".to_owned()).unwrap();
        drop(inbox);

        assert_eq!(
            bridge.try_send("运行已结束".to_owned()),
            Err(RunSteerSendError::Inactive)
        );
    }

    #[test]
    fn capacity_overflow_is_rejected_and_drain_releases_budget() {
        let bridge = RunSteerBridge::default();
        let inbox = bridge.open();

        assert_eq!(
            bridge.try_send("x".repeat(RUN_STEER_PENDING_BYTE_LIMIT)),
            Err(RunSteerSendError::CapacityExceeded)
        );
        for index in 0..RUN_STEER_PENDING_MESSAGE_LIMIT {
            bridge.try_send(format!("追加消息 {index}")).unwrap();
        }
        assert_eq!(
            bridge.try_send("超限消息".to_owned()),
            Err(RunSteerSendError::CapacityExceeded)
        );

        assert_eq!(inbox.drain().len(), RUN_STEER_PENDING_MESSAGE_LIMIT);
        bridge.try_send("释放后可继续".to_owned()).unwrap();
        assert_eq!(inbox.drain()[0].message.content, "释放后可继续");
    }

    #[test]
    fn deferred_runtime_note_uses_internal_message_and_registration_lifecycle() {
        let session_key = format!("deferred-diagnostics-{}", std::process::id());
        let bridge = RunSteerBridge::default();
        let inbox = bridge.open();
        let registration = register_deferred_message_session(&session_key, inbox.clone());

        try_send_deferred_runtime_note(
            &session_key,
            AgentRuntimeNoteKind::DeferredDiagnostics,
            "late diagnostics".to_owned(),
        )
        .unwrap();

        let input = inbox.drain().pop().unwrap();
        assert!(input.message.cache_excluded);
        assert!(input.message.content.contains("late diagnostics"));
        drop(registration);
        assert_eq!(
            try_send_deferred_runtime_note(
                &session_key,
                AgentRuntimeNoteKind::DeferredDiagnostics,
                "too late".to_owned(),
            ),
            Err(RunSteerSendError::Inactive)
        );
    }

    #[test]
    fn active_primary_turn_registry_routes_by_workspace_and_unregisters() {
        let registry = ActivePrimaryTurnRegistry::default();
        let bridge = RunSteerBridge::default();
        let inbox = bridge.open();
        let interaction_id = InteractionId::new("interaction-a").expect("Interaction ID");
        let affinity_key = ActivePrimaryTurnRegistry::affinity_key("session-lane-a");
        let registration = registry
            .register(
                &affinity_key,
                interaction_id.clone(),
                inbox.clone(),
                RunCancelToken::default(),
            )
            .expect("注册活跃主 turn");
        assert_eq!(registry.active_count(), 1);

        assert!(
            registry
                .try_send(&affinity_key, "补充约束".to_owned())
                .expect("steer 成功")
                .is_some_and(|value| value == interaction_id)
        );
        assert_eq!(inbox.drain()[0].message.content, "补充约束");
        assert!(
            !registry
                .try_send(
                    &ActivePrimaryTurnRegistry::affinity_key("session-lane-b"),
                    "其他工作区".to_owned(),
                )
                .expect("其他工作区应排队")
                .is_some()
        );
        assert!(
            !registry
                .try_send(
                    &ActivePrimaryTurnRegistry::affinity_key("session-lane-a-feishu"),
                    "其他渠道".to_owned(),
                )
                .expect("其他渠道应隔离")
                .is_some()
        );

        drop(registration);
        assert_eq!(registry.active_count(), 0);
        assert!(
            !registry
                .try_send(&affinity_key, "过晚输入".to_owned())
                .expect("注册释放后应排队")
                .is_some()
        );
    }
}
