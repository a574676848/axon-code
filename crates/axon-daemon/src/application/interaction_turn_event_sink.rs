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

/// 后续高频流式增量的微小攒批窗口（对齐人眼不可见的 5ms）。
const MICRO_BATCH_WINDOW: Duration = Duration::from_millis(5);
/// 微小批次的最大事件数。
const MICRO_BATCH_MAX_EVENTS: usize = 8;

#[derive(Default)]
struct MicroBatchState {
    pending: Vec<RuntimeEvent>,
    started_at: Option<Instant>,
    first_event_dispatched: bool,
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
            std::mem::take(&mut state.pending)
        };
        self.commit_and_publish_slice(&pending)
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
        if !is_micro_batchable(event) {
            // 关键语义事件：先将历史微批排空，再立即同步提交当前事件，严防乱序或延迟。
            self.flush()?;
            return self.commit_and_publish_slice(std::slice::from_ref(event));
        }

        let to_commit = {
            let mut state = self
                .micro_batch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.first_event_dispatched {
                // 首个 Delta：零等待立即同步写，确保 TTFR 0 毫秒附加延迟。
                state.first_event_dispatched = true;
                Some(vec![event.clone()])
            } else {
                state.pending.push(event.clone());
                if state.started_at.is_none() {
                    state.started_at = Some(Instant::now());
                }
                let should_flush = state.pending.len() >= MICRO_BATCH_MAX_EVENTS
                    || state
                        .started_at
                        .is_some_and(|started| started.elapsed() >= MICRO_BATCH_WINDOW);
                if should_flush {
                    state.started_at = None;
                    Some(std::mem::take(&mut state.pending))
                } else {
                    None
                }
            }
        };

        if let Some(events) = to_commit {
            self.commit_and_publish_slice(&events)?;
        }
        Ok(())
    }

    fn publish_batch(&self, events: &[RuntimeEvent]) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        self.flush()?;
        self.commit_and_publish_slice(events)
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
}
