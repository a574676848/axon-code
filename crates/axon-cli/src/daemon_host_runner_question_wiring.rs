//! 守护进程问答链路的装配产物。
//!
//! 宿主与答案服务必须共用同一个宿主实例：等待者注册表在实例内部，提问工具经主 turn
//! 端口注册等待，作答经答案服务回灌；实例不一致时答案服务找不到等待者，只会退回消息
//! 投递，提问工具只能空等到预算耗尽。
//!
//! 单独成文：这一组是「问答怎么接进守护进程」的装配细节，与运行循环本身无关。

use std::sync::Arc;

use axon_session::SqliteRuntimeStore;

use super::{ACTION_LEASE_DURATION_MS, CliDaemonHost, question_durable};

pub(super) struct QuestionWiring {
    /// 宿主单例：承载等待者注册表与问答事件发射。
    pub(super) host: Arc<crate::user_question::UserQuestionHost>,
    /// 答案服务：工作台作答入口据此回灌作答。
    pub(super) service: crate::user_question::UserQuestionAnswerService,
    /// 关停令牌：守护进程收口时唤醒仍在等待的提问工具。
    pub(super) shutdown: axon_agent_runtime::RunCancelToken,
    /// 是否已把渠道卡片 sink 并入事件扇出；装配自检用。
    pub(super) channel_sink_attached: bool,
    /// 问答事件的 durable 落点槽位；宿主起来之后填入。
    durable_events: crate::run_commands::DurableQuestionEventSlot,
}

impl QuestionWiring {
    /// 以运行态存储、活跃主 turn 注册表、飞书卡片 sink 与主 turn 唤醒端口装配问答宿主与答案服务。
    ///
    /// `feishu_sink` 是飞书卡片通道（未启用时为 `None`）。它与运行态通道组成扇出：
    /// 同一条事件既要进逐字稿，也要变成用户能看到的卡片；宿主与答案服务共用同一个
    /// 扇出实例，避免同一 `call_id` 的两条事件在实例之间重号。
    ///
    /// `wake` 是迟到作答落库后的主 turn 唤醒端口：答案落成 queued Interaction 时
    /// 由答案注入器立刻触发一次领取，续跑不再依赖维护定时器的轮询周期。
    pub(super) fn assemble(
        store: SqliteRuntimeStore,
        active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
        feishu_sink: Option<Arc<dyn crate::user_question::UserQuestionEventSink>>,
        wake: crate::user_question::lane_steerer::PrimaryTurnWakeHook,
    ) -> Self {
        let shutdown = axon_agent_runtime::RunCancelToken::default();
        let channel_sink_attached = feishu_sink.is_some();
        let (runtime_sink, durable_events) =
            crate::run_commands::production_question_event_sink_with_durable_slot();
        let event_sink: Arc<dyn crate::user_question::UserQuestionEventSink> = match feishu_sink {
            Some(feishu) => Arc::new(crate::user_question::UserQuestionFanoutSink::new(vec![
                runtime_sink,
                feishu,
            ])),
            None => runtime_sink,
        };
        let host = Arc::new(crate::user_question::UserQuestionHost::new(
            Arc::new(store.clone()),
            Arc::clone(&event_sink),
            Some(shutdown.clone()),
        ));
        let service = crate::user_question::UserQuestionAnswerService::new(
            Arc::new(store.clone()),
            Some((*host).clone()),
            event_sink,
            Arc::new(crate::user_question::LaneQuestionMessageSteerer::new(
                store,
                active_primary_turns,
                ACTION_LEASE_DURATION_MS,
                wake,
            )),
        );
        let wiring = Self {
            host,
            service,
            shutdown,
            channel_sink_attached,
            durable_events,
        };
        // 装配事实写入日志：卡片通道是否接入决定了提问会不会当场出卡，
        // 未接入时由网关后台 tick 兜底，排障时需要一眼看到是哪一种。
        if wiring.channel_sink_attached {
            axon_logger::debug("用户问答事件扇出已并入渠道卡片通道");
        } else {
            axon_logger::debug("用户问答事件扇出未接入渠道卡片通道，交由后台 tick 兜底");
        }
        wiring
    }

    /// 主 turn 用户提问端口；与答案服务共用同一个宿主实例。
    pub(super) fn primary_turn_port(
        &self,
    ) -> Option<Arc<dyn axon_agent_runtime::UserQuestionPort>> {
        Some(Arc::clone(&self.host) as Arc<dyn axon_agent_runtime::UserQuestionPort>)
    }

    /// 把 durable 落点装进问答事件 sink。
    ///
    /// 落盘要以宿主自己的 host 记录过代际围栏，并把落好的事件推给实时订阅者，
    /// 两者都只在宿主装配完成之后才存在，因此这一步必须晚于 `build_daemon_host`。
    pub(super) fn install_durable_question_events(
        &self,
        host: &CliDaemonHost,
        store: SqliteRuntimeStore,
    ) {
        let identity = match host.host_record() {
            Ok(identity) => identity,
            Err(error) => {
                axon_logger::warning(&format!(
                    "问答事件 durable 落点未装配：读取宿主身份失败 {error}"
                ));
                return;
            }
        };
        let writer =
            question_durable::durable_question_event_writer(store, identity, host.stream_bridge());
        if self.durable_events.set(writer).is_err() {
            axon_logger::warning("问答事件 durable 落点重复装配，保留先装的那一个");
            return;
        }
        axon_logger::debug("问答事件 durable 落点已装配：没有活跃 run 的问答帧将落进会话事件流");
    }
}
