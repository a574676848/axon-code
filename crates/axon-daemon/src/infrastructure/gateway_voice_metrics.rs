//! 语音运行指标：会话计数、队列深度、丢弃计数与延迟分位数。
//!
//! 直方图用固定对数桶 + 原子数组：写入无锁、无分配、无扩容，符合 §4.8 P1/P4；
//! 读取时按桶上界折算分位数，精度足够定位抖动，不引第三方 histogram 依赖。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 直方图桶数：桶 `i` 覆盖 `[2^i, 2^(i+1))` 微秒，24 桶覆盖 1 µs ~ 16.7 s。
const BUCKETS: usize = 24;

/// 固定对数桶延迟直方图。
///
/// 每个观测只做一次 `fetch_add` 与一次 `fetch_update`，没有锁、没有分配。
#[derive(Debug)]
pub(crate) struct LatencyHistogram {
    buckets: [AtomicU64; BUCKETS],
    count: AtomicU64,
    total_micros: AtomicU64,
    max_micros: AtomicU64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            count: AtomicU64::new(0),
            total_micros: AtomicU64::new(0),
            max_micros: AtomicU64::new(0),
        }
    }
}

impl LatencyHistogram {
    /// 记一次观测，单位微秒。
    pub(crate) fn record(&self, micros: u64) {
        let bucket = bucket_of(micros);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_micros.fetch_add(micros, Ordering::Relaxed);
        let _ = self
            .max_micros
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.max(micros))
            });
    }

    /// 没有观测时输出 `null`：宁可缺字段，也不拿 0 冒充「很快」。
    fn snapshot(&self) -> serde_json::Value {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return serde_json::Value::Null;
        }
        serde_json::json!({
            "count": count,
            "meanUs": self.total_micros.load(Ordering::Relaxed) / count,
            "p50Us": self.quantile(0.50),
            "p99Us": self.quantile(0.99),
            "maxUs": self.max_micros.load(Ordering::Relaxed),
        })
    }

    /// 按累计计数落桶估计分位数，返回桶上界。
    fn quantile(&self, quantile: f64) -> u64 {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return 0;
        }
        let target = ((count as f64) * quantile).ceil() as u64;
        let mut seen = 0u64;
        for (index, bucket) in self.buckets.iter().enumerate() {
            seen += bucket.load(Ordering::Relaxed);
            if seen >= target {
                return 1u64 << (index + 1);
            }
        }
        self.max_micros.load(Ordering::Relaxed)
    }
}

/// 微秒值落到桶下标：`log2` 下取整。
fn bucket_of(micros: u64) -> usize {
    if micros == 0 {
        return 0;
    }
    let index = (u64::BITS - micros.leading_zeros() - 1) as usize;
    index.min(BUCKETS - 1)
}

#[derive(Debug, Default)]
pub(crate) struct VoiceMetrics {
    sessions_started: AtomicU64,
    sessions_active: AtomicU64,
    queue_depth: AtomicU64,
    dropped_frames: AtomicU64,
    dropped_bytes: AtomicU64,
    rtf_milli: AtomicU64,
    audio_flushed: AtomicU64,
    drain_timeouts: AtomicU64,
    last_error: Mutex<Option<String>>,
    /// 下行事件处理耗时：解码 + 重采样 + 分片下发（realtime 车道）。
    frame_latency: LatencyHistogram,
    /// 「说完 → 首片音频」耗时：`speech_stopped` 到首个 `AudioDelta` 下发。
    first_audio_latency: LatencyHistogram,
}

impl VoiceMetrics {
    pub(crate) fn session_started(&self) {
        self.sessions_started.fetch_add(1, Ordering::Relaxed);
        self.sessions_active.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn session_finished(&self) {
        let _ = self
            .sessions_active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            });
        self.queue_depth.store(0, Ordering::Relaxed);
    }

    pub(crate) fn set_queue_depth(&self, depth: u64) {
        self.queue_depth.store(depth, Ordering::Relaxed);
    }

    pub(crate) fn record_drop(&self, frames: u64, bytes: u64) {
        self.dropped_frames.fetch_add(frames, Ordering::Relaxed);
        self.dropped_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn set_rtf(&self, rtf: f64) {
        if rtf.is_finite() && rtf >= 0.0 {
            let milli = (rtf * 1_000.0).round();
            self.rtf_milli
                .store(milli.min(u64::MAX as f64) as u64, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_error(&self, code: &str) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(code.to_owned());
        }
    }

    pub(crate) fn record_audio_flushed(&self) {
        self.audio_flushed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_drain_timeout(&self) {
        self.drain_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    /// 记一次下行事件处理耗时。
    pub(crate) fn record_frame_latency(&self, micros: u64) {
        self.frame_latency.record(micros);
    }

    /// 记一次「说完 → 首片音频」耗时。
    pub(crate) fn record_first_audio(&self, micros: u64) {
        self.first_audio_latency.record(micros);
    }

    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let rtf = self.rtf_milli.load(Ordering::Relaxed);
        serde_json::json!({
            "sessionsStarted": self.sessions_started.load(Ordering::Relaxed),
            "sessionsActive": self.sessions_active.load(Ordering::Relaxed),
            "queueDepth": self.queue_depth.load(Ordering::Relaxed),
            "droppedFrames": self.dropped_frames.load(Ordering::Relaxed),
            "droppedBytes": self.dropped_bytes.load(Ordering::Relaxed),
            "audioFlushed": self.audio_flushed.load(Ordering::Relaxed),
            "playbackDrainsTimedOut": self.drain_timeouts.load(Ordering::Relaxed),
            "rtfP50": (rtf > 0).then(|| rtf as f64 / 1_000.0),
            "frameLatency": self.frame_latency.snapshot(),
            "firstAudioLatency": self.first_audio_latency.snapshot(),
            "lastError": self.last_error.lock().ok().and_then(|slot| slot.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LatencyHistogram, VoiceMetrics, bucket_of};

    #[test]
    fn snapshot_reports_rates_and_last_error() {
        let metrics = VoiceMetrics::default();
        metrics.session_started();
        metrics.record_drop(2, 4_800);
        metrics.set_rtf(0.0913);
        metrics.record_error("backpressure");
        metrics.record_audio_flushed();
        metrics.record_drain_timeout();

        let value = metrics.snapshot();
        assert_eq!(value["sessionsStarted"], 1);
        assert_eq!(value["sessionsActive"], 1);
        assert_eq!(value["droppedFrames"], 2);
        assert_eq!(value["droppedBytes"], 4_800);
        assert_eq!(value["lastError"], "backpressure");
        assert_eq!(value["audioFlushed"], 1);
        assert_eq!(value["playbackDrainsTimedOut"], 1);
        let rtf = value["rtfP50"].as_f64().expect("应有 RTF");
        assert!((rtf - 0.091).abs() < 1e-6, "{rtf}");

        metrics.session_finished();
        assert_eq!(metrics.snapshot()["sessionsActive"], 0);
    }

    #[test]
    fn rtf_is_absent_until_measured() {
        let metrics = VoiceMetrics::default();
        assert!(metrics.snapshot()["rtfP50"].is_null(), "没有实测就不能给 0");
    }

    #[test]
    fn latency_is_absent_until_measured() {
        let metrics = VoiceMetrics::default();
        let value = metrics.snapshot();
        assert!(
            value["frameLatency"].is_null(),
            "无观测时不给假分位数：{value}"
        );
        assert!(
            value["firstAudioLatency"].is_null(),
            "无观测时不给假分位数：{value}"
        );
    }

    #[test]
    fn histogram_reports_p50_and_p99() {
        let histogram = LatencyHistogram::default();
        // 98 个 1.5 ms 观测 + 2 个 200 ms 观测：p99 在 100 样本里取第 99 个，
        // 1 个慢样本只会落在 max，2 个才会推进 p99，这条边界必须钉住。
        for _ in 0..98 {
            histogram.record(1_500);
        }
        for _ in 0..2 {
            histogram.record(200_000);
        }

        let value = histogram.snapshot();
        assert_eq!(value["count"], 100);
        // 1.5 ms 落桶 10（[1024,2048)），p50 返回桶上界 2048。
        assert_eq!(value["p50Us"], 2_048);
        // 200 ms 落桶 17（[131072,262144)），p99 累计到该桶，返回上界 262144。
        assert_eq!(value["p99Us"], 262_144);
        assert_eq!(value["maxUs"], 200_000);
        let mean = value["meanUs"].as_u64().expect("均值");
        // 98 × 1500 + 2 × 200000 = 547000，均值 5470。
        assert!(
            (5_400..5_500).contains(&mean),
            "均值应约 5.47 ms，实际 {mean}"
        );
    }

    #[test]
    fn bucket_index_is_log2() {
        assert_eq!(bucket_of(0), 0);
        assert_eq!(bucket_of(1), 0);
        assert_eq!(bucket_of(2), 1);
        assert_eq!(bucket_of(1_023), 9);
        assert_eq!(bucket_of(1_024), 10);
        assert_eq!(bucket_of(u64::MAX), 23, "超出量程落最后一桶");
    }
}
