//! 用户问答事件的会话级路由与生产 sink（UQ-004）。
//!
//! 问答宿主是进程级单例（等待者注册表与答案服务共享同一实例），因此它无法直接持有
//! 某一轮 run 的 [`RunProgressSink`]（该 sink 由 `Rc` 绑定在单轮 run 内）。这里用与
//! 子代理进度总线同一套做法把两者接起来：每轮 run 在开工时用会话车道身份登记一条
//! 事件通道，宿主的 sink 按车道把已构造好的 wire 事件投进该通道，run 收口即摘除。
//!
//! 事件构造本身收敛在 [`super::run_progress`] 的问答载荷构造函数，全端只有一处形状。

use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

use axon_domain::{UserAnswerItem, UserQuestionItem};
use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireSchemaVersion, WireSessionRef,
};

use super::run_progress::{RunProgressWireEvent, question_correlation, question_wire_payload};
use crate::user_question::UserQuestionEventSink;

/// 单车道问答事件通道：把一条已构造的 wire 事件写进该车道当前运行的事件流。
pub(crate) type QuestionEventBus =
    Arc<dyn Fn(RunProgressWireEvent) -> Result<(), String> + Send + Sync>;

/// 会话车道问答事件路由注册表；进程内共享一份，随宿主生命周期存活。
#[derive(Clone, Default)]
pub(crate) struct QuestionEventRouter {
    buses: Arc<Mutex<HashMap<String, RegisteredQuestionBus>>>,
}

/// 已登记的事件通道；登记序号用于避免旧 run 的 RAII 注销误摘新 run 的通道。
struct RegisteredQuestionBus {
    registration_id: u64,
    bus: QuestionEventBus,
}

/// 问答事件登记序号；同一车道先后两轮 run 据此区分归属。
static QUESTION_BUS_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);

/// 进程级问答事件路由。
///
/// 问答宿主与答案服务都是进程级单例，各轮 run 通过这一份路由对接，因此不需要把
/// 路由器逐层穿进运行装配；车道键与 RAII 注销保证不同会话互不串扰。
static QUESTION_EVENT_ROUTER: OnceLock<QuestionEventRouter> = OnceLock::new();

/// 取进程级问答事件路由（首次调用即创建）。
pub(crate) fn global_question_event_router() -> QuestionEventRouter {
    QUESTION_EVENT_ROUTER
        .get_or_init(QuestionEventRouter::default)
        .clone()
}

impl QuestionEventRouter {
    /// 登记某车道的问答事件通道，返回 RAII 注销句柄。
    pub(crate) fn register(
        &self,
        lane_id: &str,
        bus: QuestionEventBus,
    ) -> QuestionEventRegistration {
        let registration_id = QUESTION_BUS_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed);
        self.buses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                lane_id.to_owned(),
                RegisteredQuestionBus {
                    registration_id,
                    bus,
                },
            );
        QuestionEventRegistration {
            lane_id: lane_id.to_owned(),
            registration_id,
            buses: Arc::clone(&self.buses),
        }
    }

    /// 把一条问答事件投进该车道的运行通道；车道当前没有运行中的 turn 时如实报错。
    fn publish(&self, lane_id: &str, event: RunProgressWireEvent) -> Result<(), String> {
        let bus = self
            .buses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(lane_id)
            .map(|registered| Arc::clone(&registered.bus));
        let Some(bus) = bus else {
            return Err(format!("车道 {lane_id} 当前没有运行中的会话"));
        };
        bus(event)
    }
}

/// 问答事件通道的 RAII 注销句柄；run 收口即摘除本车道通道。
pub(crate) struct QuestionEventRegistration {
    lane_id: String,
    registration_id: u64,
    buses: Arc<Mutex<HashMap<String, RegisteredQuestionBus>>>,
}

impl Drop for QuestionEventRegistration {
    fn drop(&mut self) {
        let mut buses = self
            .buses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if buses
            .get(&self.lane_id)
            .is_some_and(|registered| registered.registration_id == self.registration_id)
        {
            buses.remove(&self.lane_id);
        }
    }
}

/// 生产问答事件 sink：把问答状态变化发成 `WireEventKind::UserQuestion` 并路由到车道。
pub(crate) struct RoutedUserQuestionEventSink {
    router: QuestionEventRouter,
    /// 没有活跃 run 时的 durable 落点；由守护进程在宿主起来之后填入。
    durable: DurableQuestionEventSlot,
}

/// 没有活跃 run 时的 durable 落点：把已经构造好的 wire 事件写进会话事件流。
///
/// 返回 `Err` 表示这一帧既没有实时投递也没有落盘，调用方据此留痕。
pub(crate) type DurableQuestionEventWriter =
    Arc<dyn Fn(&str, &WireEventEnvelope) -> Result<(), String> + Send + Sync>;

/// durable 落点的装配槽位。
///
/// 落盘要用宿主自己的 host 记录过代际围栏，还要把落好的事件推给实时订阅者；
/// 这两样都只在宿主装配完成之后才存在，而问答 sink 必须在那之前建好（宿主自己
/// 依赖问答端口）。因此这里先留一个槽位，宿主起来后由守护进程填进去。
pub(crate) type DurableQuestionEventSlot = Arc<OnceLock<DurableQuestionEventWriter>>;

/// 问答事件序号：**进程级共享**。
///
/// 宿主（挂起/延长）与答案服务（结算）各持一个 sink 实例，若序号按实例自增，
/// 同一个 `call_id` 的 pending 与 settled 会得到相同的 `event_id`，按 event_id
/// 去重的客户端会丢帧。序号因此必须是进程级唯一。
static QUESTION_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// 取下一个问答事件序号。
fn next_question_event_sequence() -> u64 {
    QUESTION_EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

impl RoutedUserQuestionEventSink {
    /// 以路由注册表与 durable 落点槽位构造 sink。
    pub(crate) fn new(router: QuestionEventRouter, durable: DurableQuestionEventSlot) -> Self {
        Self { router, durable }
    }

    /// 实时投递不可用时的兜底：把这一帧落进会话事件流。
    ///
    /// 落盘失败才是真的丢帧，因此只有那一种情况按告警留痕；成功走 debug，
    /// 与其它投递明细同一档。
    fn persist_question_event(
        &self,
        lane_id: &str,
        call_id: &str,
        status: &str,
        envelope: &WireEventEnvelope,
        realtime: &str,
    ) {
        let Some(write) = self.durable.get() else {
            // 槽位为空说明这个进程没有装配 durable 落点（例如只做投递的装配）：
            // 如实留痕，不静默丢弃。
            axon_logger::warning(&format!(
                "用户问答事件未实时投递，且本进程没有 durable 落点: lane={lane_id} call={call_id} status={status} error={realtime}"
            ));
            return;
        };
        match write(lane_id, envelope) {
            Ok(()) => axon_logger::debug(&format!(
                "用户问答事件已落进会话事件流: lane={lane_id} call={call_id} status={status}"
            )),
            Err(durable) => axon_logger::warning(&format!(
                "用户问答事件既未实时投递也未落盘，等待客户端对账: lane={lane_id} call={call_id} status={status} realtime={realtime} durable={durable}"
            )),
        }
    }
}

impl UserQuestionEventSink for RoutedUserQuestionEventSink {
    fn emit_question_event(
        &self,
        lane_id: &str,
        part_id: &str,
        call_id: &str,
        status: &str,
        questions: &[UserQuestionItem],
        answers: &[UserAnswerItem],
        handoff: Option<&str>,
        deadline_at_ms: i64,
    ) {
        let sequence = next_question_event_sequence();
        let envelope = WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!("question:{call_id}:{sequence}")),
            correlation_id: WireCorrelationId(question_correlation(call_id)),
            session: WireSessionRef {
                session_id: lane_id.to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            sequence,
            recorded_at_ms: i64::try_from(super::now_ms()).unwrap_or(i64::MAX),
            kind: WireEventKind::UserQuestion(question_wire_payload(
                part_id,
                call_id,
                status,
                questions,
                answers,
                handoff,
                deadline_at_ms,
            )),
        };
        /*
         * 有活跃 run 时事件由那一轮的记录器写库；没有活跃 run 时（迟到作答就发生在
         * 回合之外）不能就地丢弃：它是这次交互的终局事实，丢掉的直接后果是任何按
         * 历史回放的客户端都会把已经答完的提问一直显示成待答。
         */
        let realtime = self.router.publish(
            lane_id,
            RunProgressWireEvent {
                event: envelope.clone(),
            },
        );
        if let Err(error) = realtime {
            self.persist_question_event(lane_id, call_id, status, &envelope, &error);
        }
    }
}

/// 本轮 run 的会话车道身份：优先取 Interaction 的复合车道，缺失时退回运行会话。
///
/// Daemon 主 turn 的车道就是它的运行会话身份（`primaryLaneId`）；问答记录、事件路由与
/// 答案消息注入必须用同一个身份，否则客户端的作答会因车道不符被拒。
pub(crate) fn run_lane_id(
    transport_context: Option<&serde_json::Value>,
    session_id: &str,
) -> String {
    transport_context
        .and_then(|context| context.get("primaryLaneId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(session_id)
        .to_owned()
}

/// 生产问答事件 sink 与它的 durable 落点槽位。
///
/// 槽位交给装配方在宿主起来之后填入（见 `daemon_host_runner` 的安装步骤）；
/// 未填时 sink 仍可用，只是没有活跃 run 的事件只能留痕。
pub(crate) fn production_question_event_sink_with_durable_slot()
-> (Arc<dyn UserQuestionEventSink>, DurableQuestionEventSlot) {
    let durable: DurableQuestionEventSlot = Arc::new(OnceLock::new());
    let sink: Arc<dyn UserQuestionEventSink> = Arc::new(RoutedUserQuestionEventSink::new(
        global_question_event_router(),
        Arc::clone(&durable),
    ));
    (sink, durable)
}
