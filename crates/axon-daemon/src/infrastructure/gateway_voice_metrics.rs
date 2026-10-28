//! 语音运行态指标：会话、队列、丢帧、实测 RTF 与最近失败。
//!
//! 只做聚合与快照，不含判定；`/api/v1/voice/status` 取这里作为唯一事实来源。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 语音运行态指标。
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
}

impl VoiceMetrics {
    /// 记录一次会话开始。
    pub(crate) fn session_started(&self) {
        self.sessions_started.fetch_add(1, Ordering::Relaxed);
        self.sessions_active.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次会话结束。
    pub(crate) fn session_finished(&self) {
        let _ = self
            .sessions_active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            });
        self.queue_depth.store(0, Ordering::Relaxed);
    }

    /// 更新上行队列深度。
    pub(crate) fn set_queue_depth(&self, depth: u64) {
        self.queue_depth.store(depth, Ordering::Relaxed);
    }

    /// 累加丢帧。
    pub(crate) fn record_drop(&self, frames: u64, bytes: u64) {
        self.dropped_frames.fetch_add(frames, Ordering::Relaxed);
        self.dropped_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// 记录提供方实测 RTF。
    pub(crate) fn set_rtf(&self, rtf: f64) {
        if rtf.is_finite() && rtf >= 0.0 {
            let milli = (rtf * 1_000.0).round();
            self.rtf_milli
                .store(milli.min(u64::MAX as f64) as u64, Ordering::Relaxed);
        }
    }

    /// 记录最近一次失败码。
    pub(crate) fn record_error(&self, code: &str) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(code.to_owned());
        }
    }

    /// 记录一次 `playbackDrained` 回执：只有回执到达才算真正清空。
    pub(crate) fn record_audio_flushed(&self) {
        self.audio_flushed.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次停播回执超时的强制清理。
    pub(crate) fn record_drain_timeout(&self) {
        self.drain_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    /// 快照。
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
            "lastError": self.last_error.lock().ok().and_then(|slot| slot.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::VoiceMetrics;

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
        assert!(metrics.snapshot()["rtfP50"].is_null());
        metrics.set_rtf(f64::NAN);
        assert!(metrics.snapshot()["rtfP50"].is_null(), "非法值不得写入");
    }
}
