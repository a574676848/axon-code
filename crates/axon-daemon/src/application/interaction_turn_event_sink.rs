//! Primary Turn 实时事件的瞬态投递与边界落库。
//!
//! 流式增量（推理、正文片段、用量、供应商状态）只走内存实时通道：客户端收到就渲染，
//! 延迟与磁盘无关。落库只发生在语义边界：工具调用前后、流结束、轮次收口。边界上把
//! 累积的正文压成一条快照事件落库，回放据此重建整段文本，因此回放等价，而写量从
//! 「每个增量一行」降到「每个边界一行」。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axon_domain::{DaemonSessionId, InteractionId};
use axon_session::{DaemonHostRecord, RuntimeStore};
use axon_session_runtime::{
    AtomicTurnPerformanceRecorder, EventPublisher, RuntimeClock, RuntimeEvent,
    SubagentSessionSinkFactory, TurnEventSink, TurnPerformanceRecorder, TurnPerformanceStage,
};
use serde_json::Value;

use super::interaction_turn_generation_watch::{
    GenerationSignal, TurnGenerationWatch, UsageObservation, generation_signal, usage_payload,
};
use crate::application::egress_bridge::CommittedEventEgress;
use crate::application::event_stream_bridge::LiveStreamEvent;

#[path = "interaction_turn_event_frame_class.rs"]
mod frame_class;

use frame_class::{
    FrameClass, LivePartKind, annotate_live_event, assistant_part_key, classify_frame,
    live_part_text,
};

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
    /// 本轮最后一条用量更新。
    ///
    /// 流式事件经 sink 直接投递，不经过 `TurnExecutionOutcome`，因此收尾指标
    /// 只能在这里留痕；否则速率永远算不出来。
    last_usage: Mutex<Option<TurnUsageSnapshot>>,
    /// 本轮最后一条用量帧的原始载荷；收尾指标与回放副本共用。
    last_usage_payload: Mutex<Option<Value>>,
    /// 已落库的用量快照：同值不再重复写，省掉一次事务。
    last_committed_usage: Mutex<Option<Value>>,
    /// 本轮生成账本：provider 流式窗口与累计输出。
    ///
    /// 它让在跑的这一轮不必等整轮收口就进会话级速率：每收口一条 provider 流，
    /// 就按实测值补一帧实时指标。
    generation_watch: Mutex<TurnGenerationWatch>,
}

/// 收尾指标需要的用量快照。
///
/// 只留上下文与缓存读数：输出 token 由账本按流累加，不能取「最后一条观测」。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TurnUsageSnapshot {
    pub(super) input_tokens: u64,
    pub(super) cached_input_tokens: u64,
    pub(super) cache_write_input_tokens: u64,
}

/// 收尾与实时帧共用的生成指标读数。
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TurnGenerationReadout {
    /// 首字耗时；未测得时为 None。
    pub(super) ttft_ms: Option<u64>,
    /// 解码窗口；没有可测样本时为 None。
    pub(super) decode_ms: Option<u64>,
    /// 本轮实测输出 token；没有可测样本时为 0。
    pub(super) output_tokens: u64,
    /// 本轮实测推理输出 token；没有可测样本时为 0。
    pub(super) reasoning_output_tokens: u64,
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
            last_usage: Mutex::new(None),
            last_usage_payload: Mutex::new(None),
            last_committed_usage: Mutex::new(None),
            generation_watch: Mutex::new(TurnGenerationWatch::default()),
        }
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
        let (decode_ms, output_tokens, reasoning_output_tokens) =
            self.generation_watch().readout(ttft_ms, fallback_total_ms);
        TurnGenerationReadout {
            ttft_ms,
            decode_ms,
            output_tokens,
            reasoning_output_tokens,
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
                    self.commit_and_publish_slice(std::slice::from_ref(event))?;
                    if is_turn_terminal(event) {
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

    fn commit_and_publish_slice(&self, events: &[RuntimeEvent]) -> Result<(), String>
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
        let payloads = events
            .iter()
            .map(|event| {
                annotate_live_event(&event.payload_json, self.interaction_id)
                    .map(|payload_json| (event.event_type.clone(), payload_json))
            })
            .collect::<Result<Vec<_>, _>>()?;
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
        for event in &persisted {
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

fn compute_application_ttft_ms(recorder: &AtomicTurnPerformanceRecorder) -> Option<u64> {
    let snapshot = recorder.snapshot();
    let start = snapshot
        .stages
        .iter()
        .find(|s| s.stage == TurnPerformanceStage::ProviderRequestStarted)?;
    let first = snapshot
        .stages
        .iter()
        .find(|s| s.stage == TurnPerformanceStage::ProviderFirstDeltaParsed)?;
    if first.elapsed_us >= start.elapsed_us {
        Some((first.elapsed_us - start.elapsed_us) / 1000)
    } else {
        None
    }
}

/// 这一帧是否代表本轮收口。
///
/// 只有 `Durable` 帧会走到这里（每轮几十条），因此可以再解析一次载荷；
/// 热路径上的正文增量与用量永远不经过本函数。
fn is_turn_terminal(event: &RuntimeEvent) -> bool {
    const TURN_COMPLETED: &str = "turn_completed";
    if event.event_type == TURN_COMPLETED {
        return true;
    }
    let Ok(envelope) = serde_json::from_str::<Value>(&event.payload_json) else {
        return false;
    };
    let Some(kind) = envelope.get("kind") else {
        return false;
    };
    if kind.get("type").and_then(Value::as_str) != Some("turnLifecycle") {
        return false;
    }
    matches!(
        kind.get("payload")
            .and_then(|payload| payload.get("status"))
            .and_then(Value::as_str),
        Some("completed" | "failed" | "cancelled" | "aborted")
    )
}

impl<'a, S, P, C> RuntimeTurnEventSink<'a, S, P, C> {
    /// 记一条生成事件：用量观测与 provider 流边界都写进本轮账本。
    fn observe_generation_event(&self, event: &RuntimeEvent) {
        if event.event_type != "wire_event" {
            return;
        }
        let signal = generation_signal(&event.payload_json);
        if let GenerationSignal::Usage(observation) = signal {
            self.record_last_usage(observation);
            // 同一帧既进收尾指标，也留一份原始形状供收尾事件落库。
            self.record_last_usage_payload(&event.payload_json);
        }
        let now = std::time::Instant::now();
        let mut watch = self.generation_watch();
        match signal {
            GenerationSignal::StreamStarted => watch.begin_stream(now),
            GenerationSignal::StreamSettled => watch.settle_stream(now),
            GenerationSignal::Usage(observation) => watch.record_usage(observation),
            GenerationSignal::Ignored => {}
        }
    }

    /// 记录一条用量观测。
    fn record_last_usage(&self, observation: UsageObservation) {
        *self
            .last_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TurnUsageSnapshot {
            input_tokens: observation.input_tokens,
            cached_input_tokens: observation.cached_input_tokens,
            cache_write_input_tokens: observation.cache_write_input_tokens,
        });
    }

    /// 留一条用量帧的原始载荷：只在它能解析成用量帧时覆盖。
    fn record_last_usage_payload(&self, payload_json: &str) {
        let Some(payload) = usage_payload(payload_json) else {
            return;
        };
        *self
            .last_usage_payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(payload);
    }

    /// 本轮生成账本。
    fn generation_watch(&self) -> std::sync::MutexGuard<'_, TurnGenerationWatch> {
        self.generation_watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 取出「刚有 provider 流收口」的标记。
    fn take_stream_settled(&self) -> bool {
        self.generation_watch().take_stream_settled()
    }

    /// 本轮最后一条用量更新。
    pub(super) fn last_usage(&self) -> Option<TurnUsageSnapshot> {
        *self
            .last_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 本轮最后一条用量帧的原始载荷。
    pub(super) fn last_usage_payload(&self) -> Option<Value> {
        self.last_usage_payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
#[path = "interaction_turn_event_sink_tests.rs"]
mod tests;
