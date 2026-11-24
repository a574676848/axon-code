//! TUI 可观测性契约。

use std::time::Duration;

use axon_time::duration_ms;
use serde::{Deserialize, Serialize};

use super::event_envelope::RuntimeTransportTiming;
use super::first_token_telemetry::{FirstTokenTelemetry, FirstTokenTelemetrySnapshot};
use super::mailbox_telemetry::{MailboxTelemetry, MailboxTelemetrySnapshot};

const RECENT_SPAN_LIMIT: usize = 8;
const MAX_LATENCY_SAMPLES: usize = 1000;

/// TUI 性能指标快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TuiTelemetrySnapshot {
    /// 首次可交互帧耗时。
    pub ttfr_ms: Option<u64>,
    /// 最近一次绘制耗时。
    pub last_frame_latency_ms: Option<u64>,
    /// 最近一次绘制耗时（微秒）。
    pub last_frame_latency_us: Option<u64>,
    /// 最慢绘制耗时。
    pub max_frame_latency_ms: Option<u64>,
    /// 最慢绘制耗时（微秒）。
    pub max_frame_latency_us: Option<u64>,
    /// 平均绘制耗时（微秒）。
    pub avg_frame_latency_us: Option<u64>,
    /// 已绘制帧数。
    pub frame_draw_count: u64,
    /// 已合并的帧请求数。
    pub frame_coalesced_count: u64,
    /// frame channel 断开后被调度器恢复的次数。
    pub frame_disconnected_count: u64,
    /// 因连续帧稳定而跳过的绘制次数。
    pub stable_duplicate_frame_count: u64,
    /// frame coalescing 比例，0-10000 表示 0.00%-100.00%。
    pub frame_coalescing_rate_bps: u32,
    /// 超过 8ms 的慢帧数（120Hz 目标）。
    pub slow_frames_8ms: u64,
    /// 超过 16ms 的慢帧数（60Hz 目标）。
    pub slow_frames_16ms: u64,
    /// 帧耗时 P95（毫秒）。
    pub frame_latency_p95_ms: Option<u64>,
    /// 帧耗时 P95（微秒）。
    pub frame_latency_p95_us: Option<u64>,
    /// transcript 渲染缓存命中数。
    pub cache_hits: u64,
    /// transcript 渲染缓存 miss 数。
    pub cache_misses: u64,
    /// cache hit rate，0-10000 表示 0.00%-100.00%。
    pub cache_hit_rate_bps: u32,
    /// terminal resize 触发的 transcript reflow 次数。
    pub resize_reflow_count: u64,
    /// resize reflow 后保留的缓存条目数。
    pub resize_reflow_retained_entries: u64,
    /// resize reflow 淘汰的缓存条目数。
    pub resize_reflow_evicted_entries: u64,
    /// streaming delta 最近延迟。
    pub streaming_delta_latency_ms: Option<u64>,
    /// dock 动画最近耗时。
    pub dock_animation_duration_ms: Option<u64>,
    /// cell 级渲染耗时分布。
    pub render_profiler: TuiRenderProfilerSnapshot,
    /// 安全 tracing 摘要。
    pub tracing: TuiTracingSnapshot,
    /// TUI 首字链路阶段与分位数。
    pub first_token: FirstTokenTelemetrySnapshot,
    /// TUI 有界邮箱聚合指标。
    pub mailbox: MailboxTelemetrySnapshot,
}

/// TUI cell 渲染 profiler 快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TuiRenderProfilerSnapshot {
    /// 已采样 cell 次数。
    pub cell_samples: u64,
    /// 最近一次 cell 渲染耗时。
    pub last_cell_render_us: Option<u64>,
    /// 最慢 cell 渲染耗时。
    pub max_cell_render_us: Option<u64>,
    /// 小于 100 微秒的 cell 渲染次数。
    pub under_100us: u64,
    /// 100 到 999 微秒的 cell 渲染次数。
    pub under_1ms: u64,
    /// 1 到 7 毫秒的 cell 渲染次数。
    pub under_8ms: u64,
    /// 大于等于 8 毫秒的 cell 渲染次数。
    pub at_least_8ms: u64,
    /// 已渲染 cell 高度总和。
    pub rendered_cell_rows: u64,
}

/// TUI tracing 安全摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TuiTracingSnapshot {
    /// 记录的 span 数。
    pub span_count: u64,
    /// 是否发现敏感字段。
    pub contains_sensitive_fields: bool,
    /// 最近 span 名称。
    pub recent_spans: Vec<String>,
}

/// TUI telemetry 收集器。
#[derive(Debug, Clone)]
pub struct TuiTelemetry {
    started_at: std::time::Instant,
    first_frame_at: Option<std::time::Instant>,
    last_frame_latency: Option<Duration>,
    max_frame_latency: Option<Duration>,
    frame_draw_count: u64,
    frame_schedule_count: u64,
    frame_emit_count: u64,
    frame_coalesced_count: u64,
    frame_disconnected_count: u64,
    stable_duplicate_frame_count: u64,
    slow_frames_8ms: u64,
    slow_frames_16ms: u64,
    frame_latencies_us: Vec<u64>,
    cache_hits: u64,
    cache_misses: u64,
    resize_reflow_count: u64,
    resize_reflow_retained_entries: u64,
    resize_reflow_evicted_entries: u64,
    streaming_delta_latency: Option<Duration>,
    dock_animation_duration: Option<Duration>,
    render_profiler: TuiRenderProfiler,
    tracing: TuiTracing,
    first_token: FirstTokenTelemetry,
    mailbox: MailboxTelemetry,
}

/// TUI tracing span 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiTraceSpanKind {
    /// UI event。
    UiEvent,
    /// UI action。
    UiAction,
    /// Runtime event。
    RuntimeEvent,
    /// Cell mutate。
    CellMutate,
    /// Frame draw。
    FrameDraw,
    /// Snapshot load。
    SnapshotLoad,
}

/// 单次 cell 渲染 profiler 增量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TuiRenderProfilerDelta {
    pub(crate) cell_samples: u64,
    pub(crate) total_render_us: u64,
    pub(crate) max_render_us: u64,
    pub(crate) under_100us: u64,
    pub(crate) under_1ms: u64,
    pub(crate) under_8ms: u64,
    pub(crate) at_least_8ms: u64,
    pub(crate) rendered_cell_rows: u64,
}

#[derive(Debug, Clone, Default)]
struct TuiRenderProfiler {
    cell_samples: u64,
    last_cell_render_us: Option<u64>,
    max_cell_render_us: Option<u64>,
    under_100us: u64,
    under_1ms: u64,
    under_8ms: u64,
    at_least_8ms: u64,
    rendered_cell_rows: u64,
}

#[derive(Debug, Clone, Default)]
struct TuiTracing {
    span_count: u64,
    contains_sensitive_fields: bool,
    recent_spans: Vec<&'static str>,
}

impl Default for TuiTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiTelemetry {
    /// 创建 telemetry 收集器。
    #[must_use]
    pub fn new() -> Self {
        Self {
            started_at: std::time::Instant::now(),
            first_frame_at: None,
            last_frame_latency: None,
            max_frame_latency: None,
            frame_draw_count: 0,
            frame_schedule_count: 0,
            frame_emit_count: 0,
            frame_coalesced_count: 0,
            frame_disconnected_count: 0,
            stable_duplicate_frame_count: 0,
            slow_frames_8ms: 0,
            slow_frames_16ms: 0,
            frame_latencies_us: Vec::new(),
            cache_hits: 0,
            cache_misses: 0,
            resize_reflow_count: 0,
            resize_reflow_retained_entries: 0,
            resize_reflow_evicted_entries: 0,
            streaming_delta_latency: None,
            dock_animation_duration: None,
            render_profiler: TuiRenderProfiler::default(),
            tracing: TuiTracing::default(),
            first_token: FirstTokenTelemetry::new(
                axon_session_runtime::first_token_telemetry_enabled(),
            ),
            mailbox: MailboxTelemetry::default(),
        }
    }

    pub(crate) fn attach_mailbox_telemetry(&mut self, telemetry: MailboxTelemetry) {
        self.mailbox = telemetry;
    }

    /// 记录帧调度次数。
    pub fn record_frame_scheduled(&mut self) {
        self.frame_schedule_count = self.frame_schedule_count.saturating_add(1);
    }

    /// 记录帧信号发出次数。
    pub fn record_frame_emitted(&mut self) {
        self.frame_emit_count = self.frame_emit_count.saturating_add(1);
    }

    /// 记录被调度器合并掉的帧请求数。
    pub fn record_frame_coalesced(&mut self, count: u64) {
        self.frame_coalesced_count = self.frame_coalesced_count.saturating_add(count);
    }

    /// 记录帧调度通道断开并恢复 pending 状态。
    pub fn record_frame_disconnected(&mut self) {
        self.frame_disconnected_count = self.frame_disconnected_count.saturating_add(1);
    }

    /// 记录连续两帧输出一致时跳过的绘制。
    pub fn record_stable_duplicate_frame(&mut self) {
        self.stable_duplicate_frame_count = self.stable_duplicate_frame_count.saturating_add(1);
    }

    /// 记录绘制耗时。
    pub fn record_frame_draw(&mut self, latency: Duration) {
        let now = std::time::Instant::now();
        if self.first_frame_at.is_none() {
            self.first_frame_at = Some(now);
        }
        self.frame_draw_count = self.frame_draw_count.saturating_add(1);
        self.last_frame_latency = Some(latency);
        if self
            .max_frame_latency
            .is_none_or(|current| latency > current)
        {
            self.max_frame_latency = Some(latency);
        }

        // 记录慢帧统计
        let latency_us = duration_micros(latency);
        if latency_us >= 8_000 {
            self.slow_frames_8ms = self.slow_frames_8ms.saturating_add(1);
        }
        if latency_us >= 16_000 {
            self.slow_frames_16ms = self.slow_frames_16ms.saturating_add(1);
        }

        if self.frame_latencies_us.len() >= MAX_LATENCY_SAMPLES {
            let keep_from = MAX_LATENCY_SAMPLES / 10;
            self.frame_latencies_us.drain(0..keep_from);
        }
        self.frame_latencies_us.push(latency_us);

        self.record_trace_span(TuiTraceSpanKind::FrameDraw, &[]);
        self.first_token.record_frame_draw();
    }

    pub(crate) fn start_first_token_submit(&mut self) {
        self.first_token.start_submit();
    }

    pub(crate) const fn first_token_enabled(&self) -> bool {
        self.first_token.is_enabled()
    }

    pub(crate) fn observe_first_token_mailbox(
        &mut self,
        timing: Option<&RuntimeTransportTiming>,
        provider_delta_at_ms: Option<u64>,
    ) {
        self.first_token
            .observe_mailbox(timing, provider_delta_at_ms);
    }

    pub(crate) fn observe_first_visible_applied(&mut self) {
        self.first_token.observe_visible_applied();
    }

    /// 合并缓存统计。
    pub fn record_cache(&mut self, hits: u64, misses: u64) {
        self.cache_hits = self.cache_hits.saturating_add(hits);
        self.cache_misses = self.cache_misses.saturating_add(misses);
    }

    /// 记录 terminal resize reflow 的缓存保留与淘汰情况。
    pub fn record_resize_reflow(
        &mut self,
        count: u64,
        retained_entries: u64,
        evicted_entries: u64,
    ) {
        self.resize_reflow_count = self.resize_reflow_count.saturating_add(count);
        self.resize_reflow_retained_entries = self
            .resize_reflow_retained_entries
            .saturating_add(retained_entries);
        self.resize_reflow_evicted_entries = self
            .resize_reflow_evicted_entries
            .saturating_add(evicted_entries);
    }

    /// 合并 cell 渲染 profiler 增量。
    pub(crate) fn record_render_profiler(&mut self, delta: TuiRenderProfilerDelta) {
        self.render_profiler.record(delta);
    }

    /// 记录 streaming delta 延迟。
    pub fn record_streaming_delta_latency(&mut self, latency: Duration) {
        self.streaming_delta_latency = Some(latency);
    }

    /// 记录安全 span，只允许 span 名和字段名进入摘要。
    pub fn record_trace_span(&mut self, kind: TuiTraceSpanKind, field_names: &[&str]) {
        self.tracing.record(kind, field_names);
    }

    /// 生成快照。
    #[must_use]
    pub fn snapshot(&self) -> TuiTelemetrySnapshot {
        TuiTelemetrySnapshot {
            ttfr_ms: self
                .first_frame_at
                .map(|first| duration_millis(first - self.started_at)),
            last_frame_latency_ms: self.last_frame_latency.map(duration_millis),
            last_frame_latency_us: self.last_frame_latency.map(duration_micros),
            max_frame_latency_ms: self.max_frame_latency.map(duration_millis),
            max_frame_latency_us: self.max_frame_latency.map(duration_micros),
            avg_frame_latency_us: self.calculate_avg_frame_latency(),
            frame_draw_count: self.frame_draw_count,
            frame_coalesced_count: self.frame_coalesced_count,
            frame_disconnected_count: self.frame_disconnected_count,
            stable_duplicate_frame_count: self.stable_duplicate_frame_count,
            frame_coalescing_rate_bps: ratio_bps(
                self.frame_coalesced_count,
                self.frame_schedule_count,
            ),
            slow_frames_8ms: self.slow_frames_8ms,
            slow_frames_16ms: self.slow_frames_16ms,
            frame_latency_p95_ms: self.calculate_frame_latency_p95().map(us_to_rounded_ms),
            frame_latency_p95_us: self.calculate_frame_latency_p95(),
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
            cache_hit_rate_bps: ratio_bps(
                self.cache_hits,
                self.cache_hits.saturating_add(self.cache_misses),
            ),
            resize_reflow_count: self.resize_reflow_count,
            resize_reflow_retained_entries: self.resize_reflow_retained_entries,
            resize_reflow_evicted_entries: self.resize_reflow_evicted_entries,
            streaming_delta_latency_ms: self.streaming_delta_latency.map(duration_millis),
            dock_animation_duration_ms: self.dock_animation_duration.map(duration_millis),
            render_profiler: self.render_profiler.snapshot(),
            tracing: self.tracing.snapshot(),
            first_token: self.first_token.snapshot(),
            mailbox: self.mailbox.snapshot(),
        }
    }

    fn calculate_avg_frame_latency(&self) -> Option<u64> {
        let sample_count = u64::try_from(self.frame_latencies_us.len()).ok()?;
        if sample_count == 0 {
            return None;
        }
        Some(self.frame_latencies_us.iter().copied().sum::<u64>() / sample_count)
    }

    fn calculate_frame_latency_p95(&self) -> Option<u64> {
        if self.frame_latencies_us.is_empty() {
            return None;
        }
        let mut sorted = self.frame_latencies_us.clone();
        sorted.sort_unstable();
        let p95_index = sorted.len().saturating_mul(95) / 100;
        sorted.get(p95_index.min(sorted.len() - 1)).copied()
    }
}

impl TuiRenderProfiler {
    fn record(&mut self, delta: TuiRenderProfilerDelta) {
        if delta.cell_samples == 0 {
            return;
        }
        self.cell_samples = self.cell_samples.saturating_add(delta.cell_samples);
        self.last_cell_render_us = Some(delta.total_render_us / delta.cell_samples);
        self.max_cell_render_us = Some(
            self.max_cell_render_us
                .unwrap_or_default()
                .max(delta.max_render_us),
        );
        self.under_100us = self.under_100us.saturating_add(delta.under_100us);
        self.under_1ms = self.under_1ms.saturating_add(delta.under_1ms);
        self.under_8ms = self.under_8ms.saturating_add(delta.under_8ms);
        self.at_least_8ms = self.at_least_8ms.saturating_add(delta.at_least_8ms);
        self.rendered_cell_rows = self
            .rendered_cell_rows
            .saturating_add(delta.rendered_cell_rows);
    }

    fn snapshot(&self) -> TuiRenderProfilerSnapshot {
        TuiRenderProfilerSnapshot {
            cell_samples: self.cell_samples,
            last_cell_render_us: self.last_cell_render_us,
            max_cell_render_us: self.max_cell_render_us,
            under_100us: self.under_100us,
            under_1ms: self.under_1ms,
            under_8ms: self.under_8ms,
            at_least_8ms: self.at_least_8ms,
            rendered_cell_rows: self.rendered_cell_rows,
        }
    }
}

impl TuiTracing {
    fn record(&mut self, kind: TuiTraceSpanKind, field_names: &[&str]) {
        self.span_count = self.span_count.saturating_add(1);
        if field_names.iter().any(|field| is_sensitive_field(field)) {
            self.contains_sensitive_fields = true;
        }
        self.recent_spans.push(kind.as_str());
        if self.recent_spans.len() > RECENT_SPAN_LIMIT {
            let overflow = self.recent_spans.len() - RECENT_SPAN_LIMIT;
            self.recent_spans.drain(0..overflow);
        }
    }

    fn snapshot(&self) -> TuiTracingSnapshot {
        TuiTracingSnapshot {
            span_count: self.span_count,
            contains_sensitive_fields: self.contains_sensitive_fields,
            recent_spans: self
                .recent_spans
                .iter()
                .map(|span| (*span).to_owned())
                .collect(),
        }
    }
}

impl TuiTraceSpanKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::UiEvent => "UiEvent",
            Self::UiAction => "UiAction",
            Self::RuntimeEvent => "RuntimeEvent",
            Self::CellMutate => "CellMutate",
            Self::FrameDraw => "FrameDraw",
            Self::SnapshotLoad => "SnapshotLoad",
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration_ms(duration)
}

pub(crate) fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn us_to_rounded_ms(micros: u64) -> u64 {
    micros.saturating_add(500) / 1000
}

fn ratio_bps(numerator: u64, denominator: u64) -> u32 {
    if denominator == 0 {
        return 0;
    }
    let scaled = numerator.saturating_mul(10_000) / denominator;
    u32::try_from(scaled).unwrap_or(10_000).min(10_000)
}

fn is_sensitive_field(field: &str) -> bool {
    let normalized = field.to_ascii_lowercase();
    [
        "prompt", "token", "secret", "password", "api_key", "apikey", "env",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_snapshot_projects_frame_and_cache_rates() {
        let mut telemetry = TuiTelemetry::new();

        telemetry.record_frame_scheduled();
        telemetry.record_frame_scheduled();
        telemetry.record_frame_scheduled();
        telemetry.record_frame_emitted();
        telemetry.record_frame_coalesced(2);
        telemetry.record_frame_disconnected();
        telemetry.record_stable_duplicate_frame();
        telemetry.record_frame_draw(Duration::from_millis(7));
        telemetry.record_frame_draw(Duration::from_micros(8_500));
        telemetry.record_frame_draw(Duration::from_micros(16_500));
        telemetry.record_cache(3, 1);
        telemetry.record_resize_reflow(2, 5, 1);

        let snapshot = telemetry.snapshot();

        assert_eq!(snapshot.frame_draw_count, 3);
        assert_eq!(snapshot.frame_coalesced_count, 2);
        assert_eq!(snapshot.frame_disconnected_count, 1);
        assert_eq!(snapshot.stable_duplicate_frame_count, 1);
        assert_eq!(snapshot.frame_coalescing_rate_bps, 6_666);
        assert_eq!(snapshot.cache_hit_rate_bps, 7_500);
        assert_eq!(snapshot.resize_reflow_count, 2);
        assert_eq!(snapshot.resize_reflow_retained_entries, 5);
        assert_eq!(snapshot.resize_reflow_evicted_entries, 1);
        assert_eq!(snapshot.last_frame_latency_ms, Some(16));
        assert_eq!(snapshot.last_frame_latency_us, Some(16_500));
        assert_eq!(snapshot.max_frame_latency_ms, Some(16));
        assert_eq!(snapshot.max_frame_latency_us, Some(16_500));
        assert_eq!(snapshot.avg_frame_latency_us, Some(10_666));
        assert_eq!(snapshot.frame_latency_p95_ms, Some(17));
        assert_eq!(snapshot.frame_latency_p95_us, Some(16_500));
        assert_eq!(snapshot.slow_frames_8ms, 2);
        assert_eq!(snapshot.slow_frames_16ms, 1);
    }

    #[test]
    fn tracing_snapshot_flags_sensitive_field_names_without_storing_values() {
        let mut telemetry = TuiTelemetry::new();

        telemetry.record_trace_span(TuiTraceSpanKind::UiEvent, &["prompt_text"]);
        telemetry.record_trace_span(TuiTraceSpanKind::RuntimeEvent, &["event_id"]);

        let snapshot = telemetry.snapshot();

        assert!(snapshot.tracing.contains_sensitive_fields);
        assert_eq!(snapshot.tracing.span_count, 2);
        assert_eq!(
            snapshot.tracing.recent_spans,
            vec!["UiEvent".to_owned(), "RuntimeEvent".to_owned()]
        );
    }

    #[test]
    fn render_profiler_projects_cell_latency_distribution() {
        let mut telemetry = TuiTelemetry::new();

        telemetry.record_render_profiler(TuiRenderProfilerDelta {
            cell_samples: 4,
            total_render_us: 12_300,
            max_render_us: 9_000,
            under_100us: 1,
            under_1ms: 1,
            under_8ms: 1,
            at_least_8ms: 1,
            rendered_cell_rows: 12,
        });

        let profiler = telemetry.snapshot().render_profiler;

        assert_eq!(profiler.cell_samples, 4);
        assert_eq!(profiler.last_cell_render_us, Some(3_075));
        assert_eq!(profiler.max_cell_render_us, Some(9_000));
        assert_eq!(profiler.at_least_8ms, 1);
        assert_eq!(profiler.rendered_cell_rows, 12);
    }
}
