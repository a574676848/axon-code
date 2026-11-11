//! Primary Turn 实时事件的 durable 提交与顺序发布。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axon_domain::{DaemonSessionId, InteractionId};
use axon_session::{DaemonHostRecord, RuntimeStore};
use axon_session_runtime::{
    AtomicTurnPerformanceRecorder, EventPublisher, RuntimeClock, RuntimeEvent,
    SubagentSessionSinkFactory, TurnEventSink, TurnPerformanceRecorder, TurnPerformanceStage,
};

use super::interaction_turn_generation_watch::{
    GenerationSignal, TurnGenerationWatch, UsageObservation, generation_signal,
};
use crate::application::egress_bridge::CommittedEventEgress;

/// 后续高频流式增量的微小攒批窗口。
///
/// 窗口只作用于「首个增量之后」的帧：首帧始终同步落库保证 TTFR，之后同窗口内的
/// 碎片合并成一次事务。20ms 仍远低于一帧的观感阈值，但把 provider 逐帧下发的
/// 推理增量与用量帧合成一批，写事务与 fsync 次数随之下降一个量级：这两类帧在
/// 长上下文会话里能到每秒上百条，逐帧一事务会把 SQLite 单写者锁占满整个进程。
const MICRO_BATCH_WINDOW: Duration = Duration::from_millis(20);
/// 微小批次的最大事件数（超过即提前提交，保证单事务的有界性）。
const MICRO_BATCH_MAX_EVENTS: usize = 32;

#[derive(Default)]
struct MicroBatchState {
    pending: Vec<RuntimeEvent>,
    started_at: Option<Instant>,
    first_event_dispatched: bool,
    /// 上一次提交时刻；用于把「攒批只发生在高速流上」判定成事实而不是假设。
    last_commit_at: Option<Instant>,
}

/// 单个 Primary Turn 的实时 durable sink。
pub(super) struct RuntimeTurnEventSink<'a, S, P, C> {
    pub(super) store: &'a S,
    pub(super) event_egress: CommittedEventEgress<'a, P>,
    pub(super) publish_lock: Mutex<()>,
    micro_batch: Mutex<MicroBatchState>,
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
    /// 流式事件经 sink 直接落库，不经过 `TurnExecutionOutcome`，因此收尾指标
    /// 只能在这里留痕；否则速率永远算不出来。
    last_usage: Mutex<Option<TurnUsageSnapshot>>,
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
            micro_batch: Mutex::new(MicroBatchState::default()),
            host,
            session_id,
            interaction_id,
            lease_token,
            clock,
            performance,
            subagent_session_sinks,
            last_usage: Mutex::new(None),
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

    /// 强制排空任何待提交的流式微批事件。
    pub(super) fn flush(&self) -> Result<(), String>
    where
        S: RuntimeStore + Send + Sync,
        P: EventPublisher + Send + Sync,
        C: RuntimeClock,
    {
        let pending = {
            let mut state = self
                .micro_batch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.started_at = None;
            let pending = std::mem::take(&mut state.pending);
            if !pending.is_empty() {
                // 排空也是一次提交：记下时刻，避免下一帧被误判成慢速流而跳过攒批。
                state.last_commit_at = Some(Instant::now());
            }
            pending
        };
        self.commit_and_publish_slice(&pending)
    }

    /// 攒批入口：`publish` 与 `publish_batch` 共用同一份批次状态。
    ///
    /// 流式帧先入缓冲，凑满窗口或条数后一次事务提交；关键语义事件（终态、工具调用、
    /// 错误）先排空缓冲再与自身一起同步提交，顺序与延迟都不变。此前 `publish_batch`
    /// 绕过缓冲直接提交，provider 逐帧下发的推理增量与用量帧因此各占一次写事务。
    fn enqueue(&self, events: &[RuntimeEvent]) -> Result<(), String>
    where
        S: RuntimeStore + Send + Sync,
        P: EventPublisher + Send + Sync,
        C: RuntimeClock,
    {
        if events.is_empty() {
            return Ok(());
        }
        let to_commit = {
            let mut state = self
                .micro_batch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            decide_batch(&mut state, events, Instant::now())
        };
        match to_commit {
            Some(batch) => self.commit_and_publish_slice(&batch),
            None => Ok(()),
        }
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
        // 生成账本在唯一提交口观测：provider 的流式事件走批处理通道（`publish_batch`），
        // 收尾与提问事件走 `publish`，两边的入口都汇聚到这里，挂在一处才不会漏帧。
        for event in events {
            self.observe_generation_event(event);
        }
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
        // 每收口一条 provider 流就补一帧实时指标：在跑的这一轮不必等整轮收口才进
        // 会话级速率，界面上的速率才能像首字那样随本轮推进。指标帧失败不影响本轮提交。
        if self.take_stream_settled() {
            super::interaction_turn_payloads::publish_stream_settled_generation(
                self,
                self.interaction_id,
                self.clock.now_ms(),
            );
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
        self.enqueue(std::slice::from_ref(event))
    }

    fn publish_batch(&self, events: &[RuntimeEvent]) -> Result<(), String> {
        self.enqueue(events)
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

fn is_micro_batchable(event: &RuntimeEvent) -> bool {
    !(event.event_type == "turn_completed"
        || event.event_type == "toolCall"
        || event.event_type == "toolResult"
        || event.event_type == "error")
}

/// 攒批决策：返回需要立即提交的批次，返回 None 表示继续留在缓冲里。
///
/// 纯函数，只依赖批次状态、事件与当前时刻，因此攒批窗口与条数上限可以脱离
/// store 与线程直接验证。
fn decide_batch(
    state: &mut MicroBatchState,
    events: &[RuntimeEvent],
    now: Instant,
) -> Option<Vec<RuntimeEvent>> {
    if events.iter().any(|event| !is_micro_batchable(event)) {
        // 关键语义事件：先排空历史缓冲，再与自身一起提交，顺序不变。
        state.started_at = None;
        state.last_commit_at = Some(now);
        let mut batch = std::mem::take(&mut state.pending);
        batch.extend_from_slice(events);
        return Some(batch);
    }
    if !state.first_event_dispatched {
        // 首个流式帧：零等待同步写，确保 TTFR 不因攒批而被推迟。
        state.first_event_dispatched = true;
        state.last_commit_at = Some(now);
        return Some(events.to_vec());
    }
    // 慢速流不攒批：距上一次提交已经超过一个窗口，说明下一帧要等的时间自己就够长，
    // 再压进缓冲只会让每个增量都多等一个 token 间隔（首字之后的观感延迟翻倍）。
    // 高速流才会落到下面的缓冲分支。
    if state.pending.is_empty()
        && state
            .last_commit_at
            .is_some_and(|at| now.duration_since(at) >= MICRO_BATCH_WINDOW)
    {
        state.last_commit_at = Some(now);
        return Some(events.to_vec());
    }
    state.pending.extend_from_slice(events);
    if state.started_at.is_none() {
        state.started_at = Some(now);
    }
    let should_flush = state.pending.len() >= MICRO_BATCH_MAX_EVENTS
        || state
            .started_at
            .is_some_and(|started| now.duration_since(started) >= MICRO_BATCH_WINDOW);
    if should_flush {
        state.started_at = None;
        state.last_commit_at = Some(now);
        return Some(std::mem::take(&mut state.pending));
    }
    None
}

fn annotate_live_event(
    payload_json: &str,
    interaction_id: &InteractionId,
) -> Result<String, String> {
    let mut payload: serde_json::Value =
        serde_json::from_str(payload_json).map_err(|error| error.to_string())?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "Runtime 事件 payload 必须是 JSON 对象".to_owned())?;
    object.insert(
        "interactionId".to_owned(),
        serde_json::Value::String(interaction_id.as_str().to_owned()),
    );
    serde_json::to_string(&payload).map_err(|error| error.to_string())
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
        }
        let now = Instant::now();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interaction_id() -> InteractionId {
        InteractionId::new("interaction-projection-test").expect("Interaction ID 应合法")
    }

    fn stream_event(payload: &str) -> RuntimeEvent {
        RuntimeEvent {
            event_type: "wire_event".to_owned(),
            payload_json: payload.to_owned(),
        }
    }

    fn critical_event() -> RuntimeEvent {
        RuntimeEvent {
            event_type: "turn_completed".to_owned(),
            payload_json: "{}".to_owned(),
        }
    }

    #[test]
    fn stream_event_annotation_adds_only_interaction_id() {
        let payload = annotate_live_event(
            r#"{"kind":{"type":"assistantMessage","payload":{}}}"#,
            &interaction_id(),
        )
        .expect("流事件应可标注");
        let payload: serde_json::Value = serde_json::from_str(&payload).expect("payload 应合法");

        assert_eq!(payload["interactionId"], "interaction-projection-test");
        assert!(payload.get("transportContext").is_none());
    }

    #[test]
    fn first_stream_frame_commits_alone_then_window_batches_the_rest() {
        let mut state = MicroBatchState::default();
        let started = Instant::now();

        // 首帧零等待落库：TTFR 不能因为攒批被推迟。
        let first = decide_batch(&mut state, &[stream_event("a")], started).expect("首帧必须提交");
        assert_eq!(first.len(), 1);

        // 窗口内的后续帧留在缓冲，不产生写事务。
        assert!(decide_batch(&mut state, &[stream_event("b")], started).is_none());
        assert!(
            decide_batch(
                &mut state,
                &[stream_event("c")],
                started + Duration::from_millis(10)
            )
            .is_none()
        );

        // 越过窗口后，缓冲里的三帧合成一次提交。
        let batch = decide_batch(
            &mut state,
            &[stream_event("d")],
            started + MICRO_BATCH_WINDOW + Duration::from_millis(1),
        )
        .expect("越过窗口必须提交");
        assert_eq!(
            batch
                .iter()
                .map(|event| event.payload_json.as_str())
                .collect::<Vec<_>>(),
            ["b", "c", "d"]
        );
    }

    #[test]
    fn critical_event_flushes_pending_before_itself() {
        let mut state = MicroBatchState::default();
        let started = Instant::now();
        let _ = decide_batch(&mut state, &[stream_event("a")], started);
        assert!(decide_batch(&mut state, &[stream_event("b")], started).is_none());

        let batch =
            decide_batch(&mut state, &[critical_event()], started).expect("关键事件必须立即提交");
        let kinds = batch
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            ["wire_event", "turn_completed"],
            "关键事件不得越过更早的增量"
        );
    }

    #[test]
    fn slow_stream_is_never_delayed_by_the_batch_window() {
        let mut state = MicroBatchState::default();
        let started = Instant::now();
        let _ = decide_batch(&mut state, &[stream_event("a")], started);

        // 间隔已经超过窗口：每一帧都必须当场提交，不能等下一帧到达才把它带出去。
        let second = decide_batch(
            &mut state,
            &[stream_event("b")],
            started + Duration::from_millis(200),
        )
        .expect("慢速流必须立即提交");
        assert_eq!(second.len(), 1);

        let third = decide_batch(
            &mut state,
            &[stream_event("c")],
            started + Duration::from_millis(400),
        )
        .expect("慢速流必须立即提交");
        assert_eq!(third.len(), 1);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn pending_batch_is_bounded_by_max_events() {
        let mut state = MicroBatchState::default();
        let started = Instant::now();
        let _ = decide_batch(&mut state, &[stream_event("0")], started);
        let mut committed = 0_usize;
        // 窗口始终未到（每条只推进 1ms），因此唯一的提交触发点就是条数上限。
        for index in 1..=MICRO_BATCH_MAX_EVENTS {
            if let Some(batch) = decide_batch(
                &mut state,
                &[stream_event(&index.to_string())],
                started + Duration::from_millis(1),
            ) {
                committed = committed.saturating_add(batch.len());
            }
        }
        assert_eq!(
            committed, MICRO_BATCH_MAX_EVENTS,
            "条数上限必须先于窗口生效，缓冲不能被无限推高"
        );
        assert!(state.pending.is_empty());
    }
}
