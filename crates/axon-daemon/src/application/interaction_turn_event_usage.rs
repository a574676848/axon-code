//! 本轮生成账本与用量快照的读写。
//!
//! 与事件分发分开：这里只回答「本轮最后一条用量是什么、provider 流是否收口」，
//! 不参与帧的落库与广播。

use axon_session_runtime::RuntimeEvent;
use serde_json::Value;

use super::super::interaction_turn_generation_watch::{
    GenerationSignal, TurnGenerationWatch, generation_signal, usage_payload,
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
        if matches!(signal, GenerationSignal::Usage(_)) {
            // 数值统一交给账本按流累加；这里只留一份原始形状供收尾载荷落库。
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

    /// 本轮用量累加快照：整轮 token 一律取账本累加值，不取最后一条流。
    ///
    /// 一轮里每条 provider 流都是一次真实请求，整轮消耗是它们之和。取最后一条会
    /// 被一条空响应流抹成 0，而 `session_metrics` 由它派生，错的那一刻就固化进累计。
    pub(in crate::application) fn usage_snapshot(&self) -> TurnUsageSnapshot {
        let usage = self.generation_watch().accumulated_usage();
        TurnUsageSnapshot {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_write_input_tokens: usage.cache_write_input_tokens,
        }
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

    /// 本轮收口载荷里的用量：形状与实时 `usageUpdate` 帧一致，数值换成整轮累加。
    ///
    /// 客户端用一个 `applyUsage` 同时吃实时帧与收口载荷，因此键名必须保持同形；
    /// 上下文占用与构成是快照语义，仍沿用最后一条帧的值。
    pub(in crate::application) fn last_usage_payload(&self) -> Option<Value> {
        let usage = self.generation_watch().accumulated_usage();
        let mut payload = self
            .last_usage_payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        // 载荷非对象时原样返回：形状异常不该把收口载荷整段吞掉。
        if let Some(object) = payload.as_object_mut() {
            object.insert("inputTokens".to_owned(), Value::from(usage.input_tokens));
            object.insert(
                "cachedInputTokens".to_owned(),
                Value::from(usage.cached_input_tokens),
            );
            object.insert(
                "cacheWriteInputTokens".to_owned(),
                Value::from(usage.cache_write_input_tokens),
            );
            object.insert("outputTokens".to_owned(), Value::from(usage.output_tokens));
            object.insert(
                "reasoningOutputTokens".to_owned(),
                Value::from(usage.reasoning_output_tokens),
            );
        }
        Some(payload)
    }
}
