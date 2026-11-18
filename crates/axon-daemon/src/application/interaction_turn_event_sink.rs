//! Primary Turn 实时事件的瞬态投递与边界落库。
//!
//! 流式增量只走内存实时通道；落库只发生在语义边界（工具调用前后、流结束、轮次收口），
//! 边界上把累积正文压成一条快照事件，因此回放等价而写量从「每增量一行」降到「每边界一行」。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axon_domain::{DaemonSessionId, InteractionId};
use axon_session::{DaemonHostRecord, RuntimeStore};
use axon_session_runtime::{
    AtomicTurnPerformanceRecorder, EventPublisher, RuntimeClock, RuntimeEvent,
    SubagentSessionSinkFactory, TurnEventSink, TurnPerformanceRecorder, TurnPerformanceStage,
};
use serde_json::Value;

use super::interaction_turn_generation_watch::{TurnGenerationWatch, usage_payload};
use crate::application::egress_bridge::CommittedEventEgress;
use crate::application::event_stream_bridge::LiveStreamEvent;

#[path = "interaction_turn_event_frame_class.rs"]
mod frame_class;
#[path = "interaction_turn_event_signals.rs"]
mod signals;
#[path = "interaction_turn_event_usage.rs"]
mod usage;

use frame_class::{
    FrameClass, LivePartKind, annotate_live_event, assistant_part_key, classify_frame,
    live_part_text,
};
use signals::{compute_application_ttft_ms, is_turn_terminal};

/// 一个正在累积的正文 part。
struct LivePartState {
    kind: LivePartKind,
    /// 该 part 的首帧信封：快照只在正文位置上替换，其余字段与实时帧保持一致。
    envelope: Value,
    /// 已累积的正文。
    text: String,
}

/// 单个 Primary Turn 的实时 durable sink。
pub(super) struct RuntimeTurnEventSink<'a, S, P, C> {
    pub(super) store: &'a S,
    pub(super) event_egress: CommittedEventEgress<'a, P>,
    pub(super) publish_lock: Mutex<()>,
    /// 正在累积的正文 part；键带类型前缀，避免正文与推理的 partId 撞车。
    live_parts: Mutex<BTreeMap<String, LivePartState>>,
    pub(super) host: &'a DaemonHostRecord,
    pub(super) session_id: &'a DaemonSessionId,
    pub(super) interaction_id: &'a InteractionId,
    pub(super) lease_token: &'a str,
    pub(super) clock: &'a C,
    pub(super) performance: Option<Arc<AtomicTurnPerformanceRecorder>>,
    /// 子代理会话 sink 工厂：由宿主实现，运行侧据此把子代理逐字稿写进它自己的会话。
    pub(super) subagent_session_sinks: Arc<dyn SubagentSessionSinkFactory>,
    /// 本轮用量帧的原始载荷；收尾载荷按它保存与实时帧相同的形状。
    ///
    /// 数值不从这里取：整轮 token 由生成账本按流累加，这里只留形状与快照类字段。
    last_usage_payload: Mutex<Option<Value>>,
    /// 已落库的用量快照：同值不再重复写，省掉一次事务。
    last_committed_usage: Mutex<Option<Value>>,
    /// 本轮生成账本：provider 流式窗口与累计输出。
    ///
    /// 它让在跑的这一轮不必等整轮收口就进会话级速率：每收口一条 provider 流，
    /// 就按实测值补一帧实时指标。
    generation_watch: Mutex<TurnGenerationWatch>,
    /// 本轮起始时刻，用于计算整轮墙钟。
    started_at_ms: u64,
    /// 本轮指标是否已落事件：每轮只写一条。
    turn_metrics_emitted: std::sync::atomic::AtomicBool,
    /// 收尾时已算好的整轮读数；为空表示还没算（失败路径不经过收尾观测）。
    turn_readout: Mutex<Option<TurnFinalReadout>>,
}

/// 收尾指标需要的用量快照。
///
/// 五个字段都是整轮按 provider 流累加的结果，不是「最后一条观测」：一轮里每条流都是
/// 一次真实请求，最后一条往往只是一段空响应。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TurnUsageSnapshot {
    pub(super) input_tokens: u64,
    pub(super) cached_input_tokens: u64,
    pub(super) cache_write_input_tokens: u64,
    /// 整轮生成输出：被速率口径丢弃的短窗口流也计在这里。
    pub(super) output_tokens: u64,
    /// 整轮推理输出。
    pub(super) reasoning_output_tokens: u64,
}

/// 收尾与实时帧共用的生成读数。
///
/// 只留速率所需的两项：token 一律从用量账本取累加值，与输入、缓存同源。
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TurnGenerationReadout {
    /// 首字耗时；未测得时为 None。
    pub(super) ttft_ms: Option<u64>,
    /// 解码窗口；没有可测样本时为 None。
    pub(super) decode_ms: Option<u64>,
}

/// 一轮收口时的终态读数。
///
/// `turn_metrics` 与 `turn_completed.generation` 讲的是同一轮的两份读数，必须同源：
/// 各自再算一次会给出互相矛盾的结论（实测同一轮一处写 10.6K 输出，另一处只写 214），
/// 而 `session_metrics` 由前者派生，错的那一刻就固化进会话累计。
#[derive(Debug, Clone, Copy)]
pub(super) struct TurnFinalReadout {
    /// 整轮墙钟耗时：从这一轮真正开始跑算到收口，不是事件接收器的建造时刻。
    pub(super) active_ms: u64,
    /// 首字延迟。
    pub(super) ttft_ms: Option<u64>,
}

impl<'a, S, P, C> RuntimeTurnEventSink<'a, S, P, C> {
    pub(super) fn new(
        store: &'a S,
        event_egress: CommittedEventEgress<'a, P>,
        host: &'a DaemonHostRecord,
        session_id: &'a DaemonSessionId,
        interaction_id: &'a InteractionId,
        lease_token: &'a str,
        clock: &'a C,
        performance: Option<Arc<AtomicTurnPerformanceRecorder>>,
        subagent_session_sinks: Arc<dyn SubagentSessionSinkFactory>,
    ) -> Self {
        Self {
            store,
            event_egress,
            publish_lock: Mutex::new(()),
            live_parts: Mutex::new(BTreeMap::new()),
            host,
            session_id,
            interaction_id,
            lease_token,
            clock,
            performance,
            subagent_session_sinks,
            last_usage_payload: Mutex::new(None),
            last_committed_usage: Mutex::new(None),
            generation_watch: Mutex::new(TurnGenerationWatch::default()),
            started_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| {
                    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
                }),
            turn_metrics_emitted: std::sync::atomic::AtomicBool::new(false),
            turn_readout: Mutex::new(None),
        }
    }

    /// 记下这一轮收口时的整轮读数，供 `turn_metrics` 与终态载荷共用同一份事实。
    ///
    /// 只有收尾观测那一步知道整轮到底跑了多久、生成了多少 token；`turn_metrics` 在
    /// 终态批次里生成时复用它，两处不再各算一遍。
    pub(super) fn record_final_readout(&self, readout: TurnFinalReadout) {
        *self
            .turn_readout
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(readout);
    }

    /// 发布本轮的收尾观测帧。
    ///
    /// 必须早于终态事件发布：客户端在轮次终态之后会丢弃迟到的观测帧，先发指标再收口，
    /// 才不会让首字与速率在界面上永远空着。
    pub(super) fn publish_turn_observation(
        &self,
        frame: &axon_domain::RuntimeObservationFrame,
    ) -> Result<(), String>
    where
        P: EventPublisher + Send + Sync,
    {
        self.event_egress.publish_observation(frame)
    }

    /// 本轮的应用层首字延迟；未测得时为 None。
    pub(super) fn application_ttft_ms(&self) -> Option<u64> {
        self.performance
            .as_deref()
            .and_then(compute_application_ttft_ms)
    }

    /// 本轮生成指标读数：首字、解码窗口与本轮实测输出。
    ///
    /// 窗口只由**窗口可测**的 provider 流构成，实测输出与它同进同出；
    /// `fallback_total_ms` 是观测不到任何流边界时的整轮墙钟回退口径，实时帧不给回退。
    pub(super) fn generation_readout(
        &self,
        fallback_total_ms: Option<u64>,
    ) -> TurnGenerationReadout {
        let ttft_ms = self.application_ttft_ms();
        // 输出 token 不从窗口样本取：短窗口流会被速率口径丢弃，而它们的输出确实发生了。
        let (decode_ms, _, _) = self.generation_watch().readout(ttft_ms, fallback_total_ms);
        TurnGenerationReadout { ttft_ms, decode_ms }
    }

    /// 组装本轮指标事件；每轮只产出一次。
    ///
    /// 指标是终态事件的派生事实，因此不单独开一批写入：调用方把它并进终态批次，
    /// 每轮省掉一次控制面事务与一次 fsync，也不会出现「终态已落盘而指标丢失」。
    /// 序列化失败按派生事实处理：告警并返回 `None`，不阻断本轮投递。
    fn turn_metrics_event(&self) -> Option<(String, String)>
    where
        C: RuntimeClock,
    {
        if self
            .turn_metrics_emitted
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return None;
        }
        let now_ms = self.clock.now_ms();
        let readout = self.generation_readout(None);
        // 整轮 token 一律取账本累加：一条空响应流就能把「最后一条」的读数抹成 0，
        // 而这一轮实际跑过的每一条流都付过 prompt。输入、缓存、输出、推理同源，
        // 不按流的解码窗口长短做二次筛选。
        let usage = self.usage_snapshot();
        /*
         * 整轮读数优先取收尾观测已定稿的那一份。
         *
         * 现算一遍的结果取决于此刻样本落齐到什么程度，而收尾观测是这一轮最后的
         * 机会；两者不一致时，界面上 `耗时` 与 `累计` 会各说一套。墙钟同理：事件
         * 接收器的建造时刻不是这一轮的开始时刻，进程重启或重派之后它会晚一大截。
         */
        let final_readout = *self
            .turn_readout
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active_ms = final_readout.map_or_else(
            || now_ms.saturating_sub(self.started_at_ms),
            |final_| final_.active_ms,
        );
        let ttft_ms = final_readout
            .and_then(|final_| final_.ttft_ms)
            .or(readout.ttft_ms);
        let generate_ms = ttft_ms
            .unwrap_or(0)
            .saturating_add(readout.decode_ms.unwrap_or(0));
        let generated_tokens = usage
            .output_tokens
            .saturating_add(usage.reasoning_output_tokens);
        let speed = if readout.decode_ms.unwrap_or(0) > 0 && generated_tokens > 0 {
            Some((generated_tokens as f64) * 1_000.0 / (readout.decode_ms.unwrap_or(1) as f64))
        } else {
            None
        };
        let metrics = axon_eventlog::TurnMetrics {
            turn_id: self.interaction_id.as_str().to_owned(),
            ttft_ms,
            active_ms,
            provider_duration_ms: generate_ms,
            tool_ms: active_ms.saturating_sub(generate_ms),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_output_tokens,
            cache_read_tokens: usage.cached_input_tokens,
            cache_write_tokens: usage.cache_write_input_tokens,
            generation_speed_tps: speed,
            background_jobs_started: 0,
            background_jobs_running: 0,
            outcome: axon_eventlog::TurnOutcome::Succeeded,
            tool_calls: 0,
        };
        match serde_json::to_string(&metrics) {
            Ok(payload) => Some(("turn_metrics".to_owned(), payload)),
            Err(error) => {
                axon_logger::warning(&format!("turn_metrics 序列化失败：{error}"));
                None
            }
        }
    }
    /// 边界收口：把累积的正文压成快照事件落库。
    ///
    /// 空排空是常态（没有累积正文），因此它不会产生任何写入。
    pub(super) fn flush(&self) -> Result<(), String>
    where
        S: RuntimeStore + Send + Sync,
        C: RuntimeClock,
    {
        self.flush_live_parts()
    }

    /// 逐帧分派：实时帧立即投递，语义事件先落边界快照再落库。
    fn dispatch(&self, events: &[RuntimeEvent]) -> Result<(), String>
    where
        S: RuntimeStore + Send + Sync,
        P: EventPublisher + Send + Sync,
        C: RuntimeClock,
    {
        for event in events {
            self.observe_generation_event(event);
            match classify_frame(&event.payload_json) {
                FrameClass::LivePart { key, kind } => {
                    self.publish_live_frame(event, Some((key, kind)))?;
                }
                FrameClass::LiveOnly { remember } => {
                    let payload_json = self.publish_live_frame(event, None)?;
                    if remember {
                        self.remember_live_status(event, payload_json);
                    }
                }
                FrameClass::Snapshot => {
                    // 实时侧先看增量，再补一份可回放的副本：两条通道不是同一份投递。
                    // 复用已标注的那一份载荷，同一帧不必再解析一次 JSON。
                    let payload_json = self.publish_live_frame(event, None)?;
                    self.commit_replay_copy(event, &payload_json);
                }
                FrameClass::Durable => {
                    // 边界：先用快照把已累积的正文落库，再提交这个语义事件。
                    self.retire_part_for(event);
                    self.flush_live_parts()?;
                    let terminal = is_turn_terminal(event);
                    // 终态与它派生的指标同批落盘：一次控制面事务、一次 fsync。
                    let metrics = if terminal {
                        self.turn_metrics_event()
                    } else {
                        None
                    };
                    self.commit_and_publish_slice(std::slice::from_ref(event), metrics)?;
                    if terminal {
                        // 轮次已收口：可补发的读数到此作废，留着只会被下一次订阅
                        // 补发出去，把上一轮的阶段当成新一轮的现状。
                        self.event_egress
                            .forget_live(self.session_id, self.interaction_id);
                    }
                }
            }
            if self.take_stream_settled() {
                // 每收口一条 provider 流就补一帧实时指标，指标帧失败不影响本轮投递。
                super::interaction_turn_payloads::publish_stream_settled_generation(
                    self,
                    self.interaction_id,
                    self.clock.now_ms(),
                );
            }
        }
        Ok(())
    }

    /// 投递一帧实时事件；正文增量同时进入累积缓冲。
    ///
    /// 返回带 `interactionId` 标注的载荷：瞬态读数要在投递之后再留一份补发副本，
    /// 复用同一个字符串就不必为它重新解析一遍 JSON。
    fn publish_live_frame(
        &self,
        event: &RuntimeEvent,
        part: Option<(String, LivePartKind)>,
    ) -> Result<String, String>
    where
        P: EventPublisher + Send + Sync,
    {
        let payload_json = annotate_live_event(&event.payload_json, self.interaction_id)?;
        self.event_egress.publish_live(&LiveStreamEvent {
            session_id: self.session_id.clone(),
            interaction_id: self.interaction_id.clone(),
            event_type: event.event_type.clone(),
            payload_json: payload_json.clone(),
        });
        if let Some((key, kind)) = part {
            self.accumulate_part(key, kind, &payload_json)?;
        }
        if let Some(performance) = &self.performance
            && !performance.has_recorded(TurnPerformanceStage::FirstEventPublished)
        {
            performance.record(TurnPerformanceStage::FirstEventPublished);
        }
        Ok(payload_json)
    }

    /// 留一份可补发的瞬态读数，供之后建立的订阅补看。
    ///
    /// 只有 provider 状态这类低频读数走到这里（每轮几十帧）；正文增量与用量
    /// 一律不记账，所以它不会挤进每 token 一帧的热路径。
    fn remember_live_status(&self, event: &RuntimeEvent, payload_json: String)
    where
        P: EventPublisher + Send + Sync,
        C: RuntimeClock,
    {
        self.event_egress.remember_live(
            &LiveStreamEvent {
                session_id: self.session_id.clone(),
                interaction_id: self.interaction_id.clone(),
                event_type: event.event_type.clone(),
                payload_json,
            },
            self.clock.now_ms(),
        );
    }

    /// 把一帧正文计入累积缓冲。
    fn accumulate_part(
        &self,
        key: String,
        kind: LivePartKind,
        payload_json: &str,
    ) -> Result<(), String> {
        let envelope: Value =
            serde_json::from_str(payload_json).map_err(|error| error.to_string())?;
        let text = live_part_text(&envelope, kind).unwrap_or_default();
        let mut parts = self
            .live_parts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match parts.get_mut(&key) {
            Some(state) => state.text.push_str(&text),
            None => {
                parts.insert(
                    key,
                    LivePartState {
                        kind,
                        envelope,
                        text,
                    },
                );
            }
        }
        Ok(())
    }

    /// 把累积正文合成一条快照事件落库（不投递实时帧：实时侧已经看过增量）。
    fn flush_live_parts(&self) -> Result<(), String>
    where
        S: RuntimeStore + Send + Sync,
        C: RuntimeClock,
    {
        let parts = {
            let mut accumulated = self
                .live_parts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if accumulated.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *accumulated)
        };
        let mut events = Vec::with_capacity(parts.len());
        for state in parts.into_values() {
            if state.text.is_empty() {
                continue;
            }
            let mut envelope = state.envelope;
            match state.kind {
                LivePartKind::Assistant => {
                    // 快照是「该 part 到此为止的全部正文」，不是增量：按替换语义落库，
                    // 回放直接把整段写进单元格，也不会让已经收过实时增量的渠道再追加一次。
                    envelope["kind"]["payload"]["content"] = Value::String(state.text);
                    envelope["kind"]["payload"]["delta"] = Value::Bool(false);
                }
                LivePartKind::Reasoning => {
                    envelope["kind"]["payload"]["delta"] = Value::String(state.text);
                }
            }
            events.push(RuntimeEvent {
                event_type: "wire_event".to_owned(),
                payload_json: serde_json::to_string(&envelope)
                    .map_err(|error| error.to_string())?,
            });
        }
        if events.is_empty() {
            return Ok(());
        }
        self.commit_durable_only(&events)
    }

    /// 只落库、不投递实时帧：回放读得到，实时订阅者已经看过同一段增量。
    fn commit_durable_only(&self, events: &[RuntimeEvent]) -> Result<(), String>
    where
        S: RuntimeStore + Send + Sync,
        C: RuntimeClock,
    {
        let payloads = events
            .iter()
            .map(|event| (event.event_type.clone(), event.payload_json.clone()))
            .collect::<Vec<_>>();
        self.store
            .append_interaction_events(
                self.session_id,
                self.host,
                self.interaction_id,
                self.lease_token,
                &payloads,
                self.clock.now_ms(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// 给可回放的过程读数补一份持久副本。
    ///
    /// 载荷由调用方复用实时投递那一份（已带 `interactionId`），这里不再解析第二遍。
    /// 不走正文 part 的累积与边界切分；同值快照不重复写；写失败只告警。
    fn commit_replay_copy(&self, event: &RuntimeEvent, payload: &str)
    where
        S: RuntimeStore + Send + Sync,
        C: RuntimeClock,
    {
        let body = usage_payload(payload);
        if let Some(body) = &body {
            let last = self
                .last_committed_usage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.as_ref() == Some(body) {
                return;
            }
        }
        match self.store.append_interaction_events(
            self.session_id,
            self.host,
            self.interaction_id,
            self.lease_token,
            &[(event.event_type.clone(), payload.to_owned())],
            self.clock.now_ms(),
        ) {
            Ok(_) => {
                // 只有落库成功才记下这份快照：写失败时后续同值帧仍然值得再试一次。
                if let Some(body) = body {
                    *self
                        .last_committed_usage
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(body);
                }
            }
            Err(error) => {
                axon_logger::warning(&format!("用量帧持久化失败（不影响本轮）: {error:?}"));
            }
        }
    }

    /// 终态正文到达后丢掉同名累积：它的全文已经由这条事件承载，不必再写快照。
    fn retire_part_for(&self, event: &RuntimeEvent) {
        let Ok(envelope) = serde_json::from_str::<Value>(&event.payload_json) else {
            return;
        };
        let Some(kind) = envelope.get("kind") else {
            return;
        };
        if kind.get("type").and_then(Value::as_str) != Some("assistantMessage") {
            return;
        }
        let payload = kind.get("payload");
        if payload
            .and_then(|payload| payload.get("delta"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            return;
        }
        let Some(part_id) = payload
            .and_then(|payload| payload.get("partId"))
            .and_then(Value::as_str)
        else {
            return;
        };
        self.live_parts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&assistant_part_key(part_id));
    }

    /// 提交一批语义事件，并把其中可投递的部分推给订阅者。
    ///
    /// `durable_only` 是只落库、不投递实时通道的附加事件（终态轮次的 `turn_metrics`）：
    /// 它与同一批事件共享控制面事务与一次 fsync，但不对订阅者补发。
    fn commit_and_publish_slice(
        &self,
        events: &[RuntimeEvent],
        durable_only: Option<(String, String)>,
    ) -> Result<(), String>
    where
        S: RuntimeStore + Send + Sync,
        P: EventPublisher + Send + Sync,
        C: RuntimeClock,
    {
        if events.is_empty() {
            return Ok(());
        }
        let _publish_guard = self
            .publish_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut payloads = events
            .iter()
            .map(|event| {
                annotate_live_event(&event.payload_json, self.interaction_id)
                    .map(|payload_json| (event.event_type.clone(), payload_json))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let publishable = payloads.len();
        if let Some(durable) = durable_only {
            payloads.push(durable);
        }
        let persisted = self
            .store
            .append_interaction_events(
                self.session_id,
                self.host,
                self.interaction_id,
                self.lease_token,
                &payloads,
                self.clock.now_ms(),
            )
            .map_err(|error| error.to_string())?;
        if let Some(performance) = &self.performance
            && performance.has_recorded(TurnPerformanceStage::ProviderFirstDeltaParsed)
            && !performance.has_recorded(TurnPerformanceStage::FirstEventCommitted)
        {
            performance.record(TurnPerformanceStage::FirstEventCommitted);
        }
        // 只投递可投递的前缀：附加事件已经落库，但它不属于实时通道的帧序列。
        for event in persisted.iter().take(publishable) {
            self.event_egress.publish_for_interaction(
                self.interaction_id,
                event,
                &event.cursor(),
            )?;
        }
        if let Some(performance) = &self.performance
            && performance.has_recorded(TurnPerformanceStage::FirstEventCommitted)
            && !performance.has_recorded(TurnPerformanceStage::FirstEventPublished)
        {
            performance.record(TurnPerformanceStage::FirstEventPublished);
            let ttft_ms = compute_application_ttft_ms(performance);
            let lsp_stats = axon_agent_runtime::LspClientPoolRuntime::stats_snapshot();
            let frame = axon_domain::RuntimeObservationFrame::new(
                self.clock.now_ms(),
                self.session_id.as_str(),
                self.interaction_id.as_str(),
            )
            .with_generation(axon_domain::GenerationMetrics {
                ttft_ms,
                ..Default::default()
            })
            .with_lsp(axon_domain::LspMetricsSnapshot {
                active_clients: lsp_stats.active_clients,
                pending_requests: 0,
                diagnostics_count: 0,
                last_latency_ms: 0,
            });
            let _ = self.event_egress.publish_observation(&frame);
        }
        Ok(())
    }
}

impl<S, P, C> TurnEventSink for RuntimeTurnEventSink<'_, S, P, C>
where
    S: RuntimeStore + Send + Sync,
    P: EventPublisher + Send + Sync,
    C: RuntimeClock,
{
    fn publish(&self, event: &RuntimeEvent) -> Result<(), String> {
        self.dispatch(std::slice::from_ref(event))
    }

    fn publish_batch(&self, events: &[RuntimeEvent]) -> Result<(), String> {
        self.dispatch(events)
    }

    fn performance_recorder(&self) -> Option<Arc<dyn TurnPerformanceRecorder>> {
        self.performance
            .as_ref()
            .map(|performance| Arc::clone(performance) as Arc<dyn TurnPerformanceRecorder>)
    }

    fn publish_observation(
        &self,
        frame: &axon_domain::RuntimeObservationFrame,
    ) -> Result<(), String> {
        self.event_egress.publish_observation(frame)
    }

    fn subagent_session_sinks(&self) -> Option<Arc<dyn SubagentSessionSinkFactory>> {
        Some(Arc::clone(&self.subagent_session_sinks))
    }
}

#[cfg(test)]
#[path = "interaction_turn_event_sink_tests.rs"]
mod tests;
