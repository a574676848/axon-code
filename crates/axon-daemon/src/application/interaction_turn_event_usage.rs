//! 本轮生成账本与用量快照的读写。
//!
//! 与事件分发分开：这里只回答「本轮最后一条用量是什么、provider 流是否收口」，
//! 不参与帧的落库与广播。

use axon_session_runtime::RuntimeEvent;
use serde_json::Value;

use super::super::interaction_turn_generation_watch::{
    GenerationSignal, TurnGenerationWatch, UsageObservation, generation_signal, usage_payload,
};
use super::RuntimeTurnEventSink;
use super::TurnUsageSnapshot;

impl<'a, S, P, C> RuntimeTurnEventSink<'a, S, P, C> {
    /// 记一条生成事件：用量观测与 provider 流边界都写进本轮账本。
    pub(super) fn observe_generation_event(&self, event: &RuntimeEvent) {
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
    pub(super) fn record_last_usage(&self, observation: UsageObservation) {
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
    pub(super) fn record_last_usage_payload(&self, payload_json: &str) {
        let Some(payload) = usage_payload(payload_json) else {
            return;
        };
        *self
            .last_usage_payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(payload);
    }

    /// 本轮生成账本。
    pub(super) fn generation_watch(&self) -> std::sync::MutexGuard<'_, TurnGenerationWatch> {
        self.generation_watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 取出「刚有 provider 流收口」的标记。
    pub(super) fn take_stream_settled(&self) -> bool {
        self.generation_watch().take_stream_settled()
    }

    /// 本轮最后一条用量更新。
    pub(in crate::application) fn last_usage(&self) -> Option<TurnUsageSnapshot> {
        *self
            .last_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 本轮最后一条用量帧的原始载荷。
    pub(in crate::application) fn last_usage_payload(&self) -> Option<Value> {
        self.last_usage_payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
