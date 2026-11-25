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

/// 收口时把「已结算但没取走」的入站事实交还调用方重投。
///
/// 只有守护进程装配了落点（`InteractionStore::requeue_steered`）；CLI 直跑没有
/// durable 入站表，钩子留空，收口时只留痕。
pub(crate) type SteerRequeueHook =
    Arc<dyn Fn(&InteractionId, u64) -> Result<bool, String> + Send + Sync>;

#[derive(Default)]
struct RunSteerState {
    accepting: bool,
    pending: VecDeque<AgentTurnSteerInput>,
    pending_bytes: usize,
    /// 与 `pending` 逐位对齐的入站登记：`Some` 表示这条追加来自一条已在入站侧结算成
    /// understood 的 durable Interaction。它一旦没被取走，本 turn 收口时必须回投，
    /// 否则调度器不会再领取，而服务端记着「已投递」。
    tracked: VecDeque<Option<InteractionId>>,
    /// 用户主动中断过：客户端会把未注入的追加转投下一条，服务端不得再重投，
    /// 否则同一句话会跑两轮，双倍消耗预算。
    user_interrupted: bool,
    requeue: Option<SteerRequeueHook>,
}

#[derive(Clone, Default)]
pub(super) struct RunSteerBridge {
    state: Arc<Mutex<RunSteerState>>,
}

impl RunSteerBridge {
    pub(super) fn open(&self) -> RunSteerInbox {
        self.open_with_persistent_completion_wake(false)
    }

    pub(super) fn open_with_persistent_completion_wake(
        &self,
        has_persistent_completion_wake: bool,
    ) -> RunSteerInbox {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.clear();
        state.pending_bytes = 0;
        state.tracked.clear();
        state.user_interrupted = false;
        state.accepting = true;
        RunSteerInbox {
            state: Arc::clone(&self.state),
            subagents: SubagentChannel::default(),
            closes_on_drop: true,
            has_persistent_completion_wake,
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
            None,
        )
    }
}

fn try_send_input(
    state: &Arc<Mutex<RunSteerState>>,
    input: AgentTurnSteerInput,
    tracked: Option<InteractionId>,
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
    state.tracked.push_back(tracked);
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
    inbox.try_send_input(
        AgentTurnSteerInput {
            message: AgentModelMessage::runtime_note(kind, content),
            quoted_message: None,
        },
        None,
    )
}

fn run_deferred_message_registry() -> &'static Mutex<HashMap<String, RegisteredDeferredMessage>> {
    RUN_DEFERRED_MESSAGE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[path = "run_steer_requeue.rs"]
mod requeue;

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
    has_persistent_completion_wake: bool,
}

impl Clone for RunSteerInbox {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            subagents: self.subagents.clone(),
            closes_on_drop: false,
            has_persistent_completion_wake: self.has_persistent_completion_wake,
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
            // 收口时还排着的追加没有被取走：凡是登记过入站事实的，都必须交还调用方重投。
            // 它们早已在入站侧结算成 understood，不回投就再也不会被领取。
            let leftovers: Vec<InteractionId> = state.tracked.drain(..).flatten().collect();
            let user_interrupted = state.user_interrupted;
            state.pending.clear();
            state.pending_bytes = 0;
            let hook = state.requeue.clone();
            drop(state);
            if user_interrupted {
                // 用户亲手停的：桌面端会把它们转投下一条，服务端再重投就是双份。
                if !leftovers.is_empty() {
                    axon_logger::info(&format!(
                        "用户中断：{} 条未取走的追加交由客户端转投，服务端不重投",
                        leftovers.len()
                    ));
                }
                return;
            }
            requeue::requeue_leftovers(hook.as_ref(), &leftovers);
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
    // 取走即已投递：登记一起清掉，收口时不得再回投它们。
    state.tracked.clear();
    state.pending.drain(..).collect()
}

impl RunSteerInbox {
    pub(super) const fn has_persistent_completion_wake(&self) -> bool {
        self.has_persistent_completion_wake
    }

    #[cfg(test)]
    pub(super) fn try_send_message(
        &self,
        message: AgentModelMessage,
    ) -> Result<(), RunSteerSendError> {
        self.try_send_input(
            AgentTurnSteerInput {
                message,
                quoted_message: None,
            },
            None,
        )
    }

    /// 装上收口回投落点；由守护进程的注册表在登记活跃主 turn 时装配。
    pub(crate) fn install_requeue_hook(&self, hook: Option<SteerRequeueHook>) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .requeue = hook;
    }

    /// 记下「这一轮是用户亲手停的」：收口时不再重投，交给客户端转投。
    fn mark_user_interrupt(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .user_interrupted = true;
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

    fn try_send_input(
        &self,
        input: AgentTurnSteerInput,
        tracked: Option<InteractionId>,
    ) -> Result<(), RunSteerSendError> {
        try_send_input(&self.state, input, tracked)
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
    /// 收口回投落点；守护进程装配，CLI 直跑留空。登记活跃主 turn 时装进该轮队列。
    ///
    /// 必须与 `entries` 一样按引用共享：注册表会被克隆进多个装配点（问答、网关、
    /// provider runner），装配只发生一次，克隆必须都能看到。
    requeue: Arc<Mutex<Option<SteerRequeueHook>>>,
}

impl ActivePrimaryTurnRegistry {
    /// 装上收口回投落点：只装配一次，之后登记的每一轮都继承它。
    pub(crate) fn install_requeue_hook(&self, hook: SteerRequeueHook) {
        *self
            .requeue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    pub(crate) fn register(
        &self,
        affinity_key: &str,
        interaction_id: InteractionId,
        inbox: RunSteerInbox,
        cancel_token: RunCancelToken,
    ) -> Result<ActivePrimaryTurnRegistration, String> {
        let requeue = self
            .requeue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.contains_key(affinity_key) {
            return Err(ACTIVE_PRIMARY_TURN_CONFLICT_ERROR.to_owned());
        }
        inbox.install_requeue_hook(requeue);
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
        self.try_send_message(affinity_key, AgentModelMessage::text("user", content))
    }

    /// 把一条已构造好的用户消息注入活跃轮次。
    ///
    /// 附件必须随消息一起进入轮次：只传正文会让图片与大段文本落地成一句
    /// 「已附加文件：<路径>」，模型拿不到素材也读不出二进制内容。
    pub(crate) fn try_send_message(
        &self,
        affinity_key: &str,
        message: AgentModelMessage,
    ) -> Result<Option<InteractionId>, String> {
        self.send_into_turn(affinity_key, message, None)
    }

    /// 注入一条来自 durable 入站事实的追加，并登记它的标识以便收口回投。
    ///
    /// 与 [`Self::try_send_message`] 的唯一差别是「登记」：入站侧在命中活跃 turn 时
    /// 已把该 Interaction 结算成 understood，因此本 turn 收口时若还没取走它，必须由
    /// 回投钩子重新排队，否则它再也不会被领取。
    pub(crate) fn try_send_steer(
        &self,
        affinity_key: &str,
        interaction_id: InteractionId,
        message: AgentModelMessage,
    ) -> Result<Option<InteractionId>, String> {
        self.send_into_turn(affinity_key, message, Some(interaction_id))
    }

    fn send_into_turn(
        &self,
        affinity_key: &str,
        message: AgentModelMessage,
        tracked: Option<InteractionId>,
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
        match turn.inbox.try_send_input(
            AgentTurnSteerInput {
                message,
                quoted_message: None,
            },
            tracked,
        ) {
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

    pub(crate) fn affinity_key(primary_session_id: &str) -> String {
        primary_session_id.to_owned()
    }

    /// 中断某条车道上正在运行的主 turn。
    ///
    /// 取消令牌是既有机制：置位后 provider 传输、重试退避与工具执行的所有取消点
    /// 都会立即收口，本轮按「已中断」结束。返回被中断的 Interaction 标识；
    /// `Ok(None)` 表示该车道当前没有活跃主 turn，调用方应如实回执而不是假装成功。
    pub(crate) fn interrupt(&self, affinity_key: &str) -> Result<Option<InteractionId>, String> {
        let Some(turn) = self.registered_turn(affinity_key) else {
            return Ok(None);
        };
        // 先把 entry 取出锁外再置位：取消会同步唤醒等待中的 provider 流，
        // 持锁调用会让并发的注册与注销一起阻塞。
        turn.cancel_token.cancel();
        Ok(Some(turn.interaction_id))
    }

    /// 用户主动中断（网关中断 / ESC）。
    ///
    /// 与 [`Self::interrupt`] 的唯一差别是把「客户端会接手」这一事实记进本轮队列：
    /// 桌面端在中断时会把未注入的追加转投下一条，服务端若同时重投，同一句话就会跑
    /// 两轮、双倍消耗预算。租约丢失与停机走的仍是 [`Self::interrupt`]，那两条路径
    /// 没有客户端接手，必须由服务端重投。
    pub(crate) fn interrupt_for_user(
        &self,
        affinity_key: &str,
    ) -> Result<Option<InteractionId>, String> {
        let Some(turn) = self.registered_turn(affinity_key) else {
            return Ok(None);
        };
        turn.inbox.mark_user_interrupt();
        turn.cancel_token.cancel();
        Ok(Some(turn.interaction_id))
    }

    fn registered_turn(&self, affinity_key: &str) -> Option<RegisteredPrimaryTurn> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(affinity_key)
            .cloned()
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
#[path = "run_steer_tests.rs"]
mod tests;
