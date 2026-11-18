//! 本轮生成速率的实时账本：provider 流式窗口与累计输出 token。
//!
//! 会话级速率需要一个解码窗口和一个输出 token 数。Provider 只在每条流结束时回报用量，
//! 因此账本按流记账，绝不用本地估算顶替实测值：
//!
//! - 窗口：`Σ(每条 provider 流从 StreamingStarted 到 StreamingCompleted 的实测耗时)`。
//!   工具执行、等待与未收口的失败重试都不进窗口，速率不再被它们拉低。
//! - token：`Σ(每条流的最终用量快照)`。用量帧是**该条流的累计快照**而不是增量，
//!   同一条流的快照会重复投递（共享 producer 在收口前补发），按流覆盖才不会重复计。
//!
//! 一条流边界都观测不到时，收尾口径退回整轮墙钟：宁可口径退化，也不让速率整段消失。

use std::time::Instant;

use axon_protocol::wire::{
    WIRE_LIFECYCLE_KIND_STREAMING_COMPLETED, WIRE_LIFECYCLE_KIND_STREAMING_STARTED,
};

/// 用量观测事件的枚举标签，对应 `axon_protocol::wire::WireEventKind::UsageUpdate`。
const KIND_USAGE_UPDATE: &str = "usageUpdate";
/// 轮次生命周期事件的枚举标签，对应 `axon_protocol::wire::WireEventKind::TurnLifecycle`。
const KIND_TURN_LIFECYCLE: &str = "turnLifecycle";

/// 一条流可测解码窗口的下限。
///
/// Provider 把思考憋在服务端时，第一条流常在首字到达之后立刻收口：几十毫秒的窗口
/// 配上一整段 token 会算出几千 tok/s。低于下限的流整个丢弃：窗口与 token 一起进出，
/// 速率只由窗口可测的流构成。
const MIN_DECODE_WINDOW_MS: u64 = 250;

/// 一条用量观测的原始读数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct UsageObservation {
    /// 输入 token。
    pub(super) input_tokens: u64,
    /// 缓存读取输入 token。
    pub(super) cached_input_tokens: u64,
    /// 缓存写入输入 token。
    pub(super) cache_write_input_tokens: u64,
    /// 输出 token。
    pub(super) output_tokens: u64,
    /// 推理输出 token。
    pub(super) reasoning_output_tokens: u64,
}

impl UsageObservation {
    /// 并入另一份观测。
    ///
    /// 一轮的总消耗是各条 provider 流之和：每条流都对应一次真实请求，累加才对得上
    /// 「这轮一共付了多少 prompt」。取最后一条流只会留下最后一小段。
    fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(other.cache_write_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
    }

    /// 撤掉已计入的一份观测：晚到快照按整组覆盖时用它重记，避免同一份计两次。
    fn saturating_sub(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_sub(other.input_tokens),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(other.cached_input_tokens),
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .saturating_sub(other.cache_write_input_tokens),
            output_tokens: self.output_tokens.saturating_sub(other.output_tokens),
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .saturating_sub(other.reasoning_output_tokens),
        }
    }
}

/// 一条 wire 事件对生成账本的写入意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GenerationSignal {
    /// 一条 provider 流开始。
    StreamStarted,
    /// 一条 provider 流收口（最后一个 delta 与 usage 之后）。
    StreamSettled,
    /// 用量观测。
    Usage(UsageObservation),
    /// 与生成指标无关。
    Ignored,
}

/// 从 runtime 事件载荷里读出生成信号。
///
/// 只取需要的几个数，不按整包结构反序列化：线上载荷会随版本增删字段，严格解析一旦失败
/// 就会静默丢掉速率，这类指标不值得冒这个风险。
pub(super) fn generation_signal(payload_json: &str) -> GenerationSignal {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload_json) else {
        return GenerationSignal::Ignored;
    };
    let Some(kind) = value.get("kind") else {
        return GenerationSignal::Ignored;
    };
    match kind.get("type").and_then(serde_json::Value::as_str) {
        Some(KIND_USAGE_UPDATE) => {
            let Some(payload) = kind.get("payload") else {
                return GenerationSignal::Ignored;
            };
            GenerationSignal::Usage(UsageObservation {
                input_tokens: read_u64(payload, "inputTokens"),
                cached_input_tokens: read_u64(payload, "cachedInputTokens"),
                cache_write_input_tokens: read_u64(payload, "cacheWriteInputTokens"),
                output_tokens: read_u64(payload, "outputTokens"),
                reasoning_output_tokens: read_u64(payload, "reasoningOutputTokens"),
            })
        }
        Some(KIND_TURN_LIFECYCLE) => {
            match kind
                .pointer("/payload/kind")
                .and_then(serde_json::Value::as_str)
            {
                Some(WIRE_LIFECYCLE_KIND_STREAMING_STARTED) => GenerationSignal::StreamStarted,
                Some(WIRE_LIFECYCLE_KIND_STREAMING_COMPLETED) => GenerationSignal::StreamSettled,
                _ => GenerationSignal::Ignored,
            }
        }
        _ => GenerationSignal::Ignored,
    }
}

/// 取出一次用量帧的原始载荷。
///
/// 用量帧本身是 LiveOnly：重放历史时客户端收不到它，上下文与缓存读数就会整段消失。
/// 收尾事件把这最后一次快照一起落库，重放才有与 TUI resume snapshot 同一定位的读数。
pub(super) fn usage_payload(payload_json: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let kind = value.get("kind")?;
    if kind.get("type").and_then(serde_json::Value::as_str) != Some(KIND_USAGE_UPDATE) {
        return None;
    }
    kind.get("payload").cloned()
}

/// 读一个非负整数字段；缺失或类型不符按 0 计。
fn read_u64(value: &serde_json::Value, field: &str) -> u64 {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default()
}

/// 一条已收口 provider 流的实测样本。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StreamSample {
    /// 该条流从 StreamingStarted 到 StreamingCompleted 的实测耗时。
    duration_ms: u64,
    /// 该条流的最终用量快照。
    usage: UsageObservation,
}

/// 一轮的生成账本：provider 流式窗口与实测输出。
#[derive(Debug, Default)]
pub(super) struct TurnGenerationWatch {
    /// 当前在跑 provider 流的开始时刻；没有流在跑时为 None。
    stream_started_at: Option<Instant>,
    /// 已收口流的实测样本，按收口顺序排列。
    samples: Vec<StreamSample>,
    /// 在跑流已到达的用量快照；流收口时与窗口一起成样本。
    pending: StreamSample,
    /// 已收口流的用量累加。
    ///
    /// 本轮总消耗取这个值，不取最后一条流：一条空响应流就能把整轮读数抹成 0。
    usage_total: UsageObservation,
    /// 自上次取用以来是否有 provider 流收口。
    stream_settled: bool,
}

impl TurnGenerationWatch {
    /// 一条 provider 流开始。
    ///
    /// 重试会直接开新流：未收口的尝试不记窗口，与收尾口径的「只累计已完成的 provider 流」一致。
    pub(super) fn begin_stream(&mut self, now: Instant) {
        self.pending = StreamSample::default();
        self.stream_started_at = Some(now);
    }

    /// 一条 provider 流收口：把它的实测耗时与用量快照一起存成样本。
    pub(super) fn settle_stream(&mut self, now: Instant) {
        if let Some(started_at) = self.stream_started_at.take() {
            self.usage_total.add_assign(self.pending.usage);
            self.samples.push(StreamSample {
                duration_ms: elapsed_ms(started_at, now),
                ..self.pending
            });
            self.pending = StreamSample::default();
        }
        self.stream_settled = true;
    }

    /// 记一条用量观测：用量帧是该条流的累计快照，重复投递按覆盖处理。
    pub(super) fn record_usage(&mut self, observation: UsageObservation) {
        if self.stream_started_at.is_none()
            && let Some(last) = self.samples.last_mut()
            && last.usage.output_tokens == 0
            && last.usage.reasoning_output_tokens == 0
        {
            // 用量帧晚于流收口到达：补给它所属的那条流，不落到下一条流身上。
            // 按整组快照重记：先撤掉该流已计入的那份，再把新快照记回去，
            // 晚到的输入与缓存读数因此不会漏，也不会把同一份计两次。
            self.usage_total = self.usage_total.saturating_sub(last.usage);
            last.usage = observation;
            self.usage_total.add_assign(observation);
            return;
        }
        self.pending.usage = observation;
    }

    /// 本轮截至当前的用量累加。
    ///
    /// 已收口流全部计入；仍在跑的那条流按它最新的快照额外并入，收口瞬间也不会回落。
    pub(super) fn accumulated_usage(&self) -> UsageObservation {
        let mut usage = self.usage_total;
        if self.stream_started_at.is_some() {
            usage.add_assign(self.pending.usage);
        }
        usage
    }

    /// 速率读数：解码窗口与本轮实测输出。
    ///
    /// 只有**窗口可测**的流进出分子与分母：首字只从第一条流的时间里扣（思考型模型的首字
    /// 可能晚于该流收口），扣完窗口仍短于下限的流整个丢弃，否则几十毫秒的窗口配一整段
    /// token 会算出几千 tok/s，那不是速度而是测量噪声。
    ///
    /// `fallback_total_ms` 是观测不到任何流边界时的整轮墙钟回退口径，只给窗口不给 token：
    /// 那种情况下本轮没有可信的实测输出，客户端用自己的用量累计兜底。首字缺失就不给读数。
    pub(super) fn readout(
        &self,
        ttft_ms: Option<u64>,
        fallback_total_ms: Option<u64>,
    ) -> (Option<u64>, u64, u64) {
        let Some(ttft_ms) = ttft_ms else {
            return (None, 0, 0);
        };
        let mut window_ms = 0_u64;
        let mut output_tokens = 0_u64;
        let mut reasoning_output_tokens = 0_u64;
        let mut kept = 0_usize;
        for (position, sample) in self.samples.iter().enumerate() {
            let latency_ms = if position == 0 {
                ttft_ms.min(sample.duration_ms)
            } else {
                0
            };
            let decode_ms = sample.duration_ms.saturating_sub(latency_ms);
            if decode_ms < MIN_DECODE_WINDOW_MS {
                continue;
            }
            window_ms = window_ms.saturating_add(decode_ms);
            output_tokens = output_tokens.saturating_add(sample.usage.output_tokens);
            reasoning_output_tokens =
                reasoning_output_tokens.saturating_add(sample.usage.reasoning_output_tokens);
            kept = kept.saturating_add(1);
        }
        if kept == 0 {
            return (
                fallback_total_ms.map(|total| total.saturating_sub(ttft_ms)),
                0,
                0,
            );
        }
        (Some(window_ms), output_tokens, reasoning_output_tokens)
    }

    /// 取出并清掉「有 provider 流收口」的标记。
    pub(super) fn take_stream_settled(&mut self) -> bool {
        std::mem::take(&mut self.stream_settled)
    }
}

/// 两个时刻之间的整毫秒耗时。
fn elapsed_ms(started_at: Instant, now: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(started_at).as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 造一条 wire 事件载荷。
    fn wire_payload(kind: serde_json::Value) -> String {
        serde_json::json!({ "kind": kind, "interactionId": "interaction-1" }).to_string()
    }

    fn usage_payload(output: u64, reasoning: u64) -> String {
        wire_payload(serde_json::json!({
            "type": "usageUpdate",
            "payload": {
                "inputTokens": 100,
                "cachedInputTokens": 40,
                "cacheWriteInputTokens": 5,
                "outputTokens": output,
                "reasoningOutputTokens": reasoning,
            },
        }))
    }

    /// 记一条用量观测。
    fn usage(watch: &mut TurnGenerationWatch, output: u64, reasoning: u64) {
        watch.record_usage(UsageObservation {
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            ..UsageObservation::default()
        });
    }

    #[test]
    fn stream_boundaries_and_usage_are_classified() {
        let started = wire_payload(serde_json::json!({
            "type": "turnLifecycle",
            "payload": { "kind": "StreamingStarted" },
        }));
        let completed = wire_payload(serde_json::json!({
            "type": "turnLifecycle",
            "payload": { "kind": "StreamingCompleted" },
        }));

        assert_eq!(generation_signal(&started), GenerationSignal::StreamStarted);
        assert_eq!(
            generation_signal(&completed),
            GenerationSignal::StreamSettled
        );
        assert_eq!(
            generation_signal(&usage_payload(12, 3)),
            GenerationSignal::Usage(UsageObservation {
                input_tokens: 100,
                cached_input_tokens: 40,
                cache_write_input_tokens: 5,
                output_tokens: 12,
                reasoning_output_tokens: 3,
            })
        );
    }

    #[test]
    fn unrelated_and_malformed_payloads_are_ignored() {
        assert_eq!(generation_signal("not json"), GenerationSignal::Ignored);
        assert_eq!(generation_signal("{}"), GenerationSignal::Ignored);
        assert_eq!(
            generation_signal(&wire_payload(serde_json::json!({
                "type": "assistantDelta",
                "payload": { "content": "hello" },
            }))),
            GenerationSignal::Ignored
        );
        // 其它生命周期阶段（连接、重试、整轮终态）不是 provider 流边界。
        assert_eq!(
            generation_signal(&wire_payload(serde_json::json!({
                "type": "turnLifecycle",
                "payload": { "kind": "ProviderRetry" },
            }))),
            GenerationSignal::Ignored
        );
        // 用量载荷缺字段时按 0 计，不整条丢弃。
        assert_eq!(
            generation_signal(&wire_payload(serde_json::json!({
                "type": "usageUpdate",
                "payload": { "outputTokens": 7 },
            }))),
            GenerationSignal::Usage(UsageObservation {
                output_tokens: 7,
                ..UsageObservation::default()
            })
        );
    }

    #[test]
    fn windows_sum_streams_without_the_tool_gap() {
        let mut watch = TurnGenerationWatch::default();
        let t0 = Instant::now();
        // 第一条流 3s，工具执行 20s，第二条流 2s。
        watch.begin_stream(t0);
        usage(&mut watch, 100, 20);
        watch.settle_stream(t0 + Duration::from_secs(3));
        watch.begin_stream(t0 + Duration::from_secs(23));
        usage(&mut watch, 7, 0);
        watch.settle_stream(t0 + Duration::from_secs(25));

        // 首字 0.5s 只从第一条流的时间里扣；token 与窗口同进同出。
        assert_eq!(watch.readout(Some(500), None), (Some(4_500), 107, 20));
    }

    #[test]
    fn streams_with_unmeasurable_windows_are_dropped_with_their_tokens() {
        let mut watch = TurnGenerationWatch::default();
        let t0 = Instant::now();
        // Provider 把思考憋在服务端：第一条流在首字到达后 30ms 就收口，却带回 186 token。
        watch.begin_stream(t0);
        usage(&mut watch, 0, 186);
        watch.settle_stream(t0 + Duration::from_millis(30));

        // 没有可测样本就不给速率：30ms 窗口配 186 token 会算出 6200 tok/s 的假速度。
        assert_eq!(watch.readout(Some(30), None), (None, 0, 0));
        // 收尾口径退回整轮墙钟，且不带 token（客户端用自己的用量累计兜底）。
        assert_eq!(watch.readout(Some(30), Some(10_000)), (Some(9_970), 0, 0));

        // 第二条流窗口可测：它自己带着 token 进出样本。
        watch.begin_stream(t0 + Duration::from_secs(1));
        usage(&mut watch, 0, 400);
        watch.settle_stream(t0 + Duration::from_secs(3));

        assert_eq!(watch.readout(Some(30), Some(10_000)), (Some(2_000), 0, 400));
    }

    #[test]
    fn unsettled_retry_attempts_are_not_samples() {
        let mut watch = TurnGenerationWatch::default();
        let t0 = Instant::now();
        // 第一次尝试流了 10s 后失败重试，只有收口的那条流成样本。
        watch.begin_stream(t0);
        watch.begin_stream(t0 + Duration::from_secs(10));
        usage(&mut watch, 5, 0);
        watch.settle_stream(t0 + Duration::from_secs(12));

        assert_eq!(watch.readout(Some(0), None), (Some(2_000), 5, 0));
    }

    #[test]
    fn fallback_needs_a_first_token_and_no_measurable_stream() {
        let mut watch = TurnGenerationWatch::default();
        // 没有首字就没有可减的起点：整轮墙钟回退也不给读数。
        assert_eq!(watch.readout(None, Some(30_000)), (None, 0, 0));
        assert_eq!(watch.readout(Some(800), Some(30_000)), (Some(29_200), 0, 0));

        let t0 = Instant::now();
        watch.begin_stream(t0);
        watch.settle_stream(t0 + Duration::from_secs(1));
        // 已有可测样本就不再回退：整轮墙钟不能冒充解码窗口。
        assert_eq!(watch.readout(Some(0), Some(30_000)), (Some(1_000), 0, 0));
    }

    #[test]
    fn late_first_output_latency_is_charged_only_to_the_first_stream() {
        let mut watch = TurnGenerationWatch::default();
        let t0 = Instant::now();
        // 服务端缓冲：首字（首个可见输出）在 44.75s 才到，第一条流没有任何可见增量。
        watch.begin_stream(t0);
        watch.settle_stream(t0 + Duration::from_millis(15_700));
        watch.begin_stream(t0 + Duration::from_millis(15_700));
        usage(&mut watch, 0, 575);
        watch.settle_stream(t0 + Duration::from_millis(44_800));
        watch.begin_stream(t0 + Duration::from_millis(44_900));
        watch.settle_stream(t0 + Duration::from_millis(56_900));

        // 首字 44.75s 只扣第一条流的 15.7s（扣完成 0 窗口，连同它的 token 一起丢弃），
        // 后续两条流各按自身耗时进样本。
        assert_eq!(watch.readout(Some(44_750), None), (Some(41_100), 0, 575));
    }

    #[test]
    fn usage_frames_arriving_after_the_boundary_go_to_that_stream() {
        let mut watch = TurnGenerationWatch::default();
        let t0 = Instant::now();
        watch.begin_stream(t0);
        watch.settle_stream(t0 + Duration::from_secs(2));
        // 用量帧晚于收口到达：补给刚收口的那条流，不落到下一条流身上。
        usage(&mut watch, 5, 6);

        assert_eq!(watch.readout(Some(0), None), (Some(2_000), 5, 6));
    }

    #[test]
    fn repeated_usage_frames_of_one_stream_are_counted_once() {
        let mut watch = TurnGenerationWatch::default();
        let t0 = Instant::now();
        // 流内实时帧与收尾补发是同一份快照（数值相同）：按覆盖处理。
        watch.begin_stream(t0);
        usage(&mut watch, 265, 1_012);
        usage(&mut watch, 265, 1_012);
        watch.settle_stream(t0 + Duration::from_secs(2));
        watch.begin_stream(t0 + Duration::from_secs(3));
        usage(&mut watch, 40, 403);
        watch.settle_stream(t0 + Duration::from_secs(5));

        assert_eq!(watch.readout(Some(0), None), (Some(4_000), 305, 1_415));
    }

    #[test]
    fn stream_settled_flag_is_taken_once_per_boundary() {
        let mut watch = TurnGenerationWatch::default();
        let t0 = Instant::now();
        assert!(!watch.take_stream_settled());

        watch.begin_stream(t0);
        watch.settle_stream(t0 + Duration::from_millis(10));
        assert!(watch.take_stream_settled());
        assert!(!watch.take_stream_settled(), "同一帧实时指标只补发一次");
    }

    #[test]
    fn turn_usage_accumulates_every_stream_not_the_last_one() {
        // 取自 2026-11-18 那轮的真实帧序列：11 条 provider 流，最后一条是空响应
        // （input 为整份上下文、不带缓存明细）。逐条取「最后一份快照」会被它抹成 0，
        // 按流累加才是这一轮真实付过的量。
        let mut watch = TurnGenerationWatch::default();
        for (input, cached, output, reasoning) in [
            (507_313, 0, 362, 2_502),
            (42_963, 507_314, 506, 901),
            (23_996, 550_278, 417, 805),
            (24_571, 574_275, 390, 1_526),
            (20_347, 598_847, 348, 887),
            (20_463, 619_195, 452, 1_018),
            (683_693, 0, 434, 383),
            (42_749, 683_694, 503, 1_739),
            (48_964, 726_444, 390, 416),
            (29_796, 775_409, 1_810, 1_657),
            (796_889, 0, 0, 0),
        ] {
            let t0 = Instant::now();
            watch.begin_stream(t0);
            watch.record_usage(UsageObservation {
                input_tokens: input,
                cached_input_tokens: cached,
                output_tokens: output,
                reasoning_output_tokens: reasoning,
                ..UsageObservation::default()
            });
            watch.settle_stream(t0);
        }

        let usage = watch.accumulated_usage();
        assert_eq!(usage.cached_input_tokens, 5_035_456, "缓存读取按流累加");
        assert_eq!(usage.input_tokens, 2_241_744);
        assert_eq!(usage.output_tokens, 5_612);
        assert_eq!(usage.reasoning_output_tokens, 11_834);
    }

    #[test]
    fn in_flight_stream_counts_before_it_settles() {
        // 收口瞬间读数不能回落：最后一条流还没收口时，也要算进本轮累计。
        let mut watch = TurnGenerationWatch::default();
        watch.begin_stream(Instant::now());
        watch.record_usage(UsageObservation {
            input_tokens: 10,
            cached_input_tokens: 90,
            ..UsageObservation::default()
        });

        let usage = watch.accumulated_usage();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.cached_input_tokens, 90);
    }
}
