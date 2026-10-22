//! TUI 首字链路的有界阶段遥测。

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::event_envelope::RuntimeTransportTiming;

const FIRST_TOKEN_SAMPLE_LIMIT: usize = 128;

/// 首字链路分位数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstTokenLatencyPercentiles {
    /// 有效样本数。
    pub sample_count: u64,
    /// P50 毫秒。
    pub p50_ms: Option<u64>,
    /// P95 毫秒。
    pub p95_ms: Option<u64>,
    /// P99 毫秒。
    pub p99_ms: Option<u64>,
}

/// 单个 TUI turn 的首字阶段快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstTokenSample {
    /// Daemon 回执确认的 Primary Interaction ID。
    pub interaction_id: Option<String>,
    /// TUI submit 开始时间。
    pub submit_started_at_ms: Option<u64>,
    /// IPC submit 开始时间。
    pub ipc_submit_started_at_ms: Option<u64>,
    /// IPC submit 回执到达时间。
    pub ipc_submit_finished_at_ms: Option<u64>,
    /// 客户端收到 durable 提交回执的上界，不代表 SQLite 内部 commit 瞬间。
    pub interaction_appended_at_ms: Option<u64>,
    /// 首个有效 Provider 输出进入 wire 的时间近似值。
    pub provider_first_delta_parsed_at_ms: Option<u64>,
    /// 首个 IPC 事件到达客户端的时间。
    pub first_ipc_event_received_at_ms: Option<u64>,
    /// 首个已接受 mailbox 事件开始应用的时间。
    pub first_mailbox_event_applied_at_ms: Option<u64>,
    /// 首个可见事件完成 reducer 应用的时间。
    pub first_visible_event_applied_at_ms: Option<u64>,
    /// 首个包含可见事件的 frame 完成时间。
    pub first_render_completed_at_ms: Option<u64>,
    /// 是否未发现墙钟倒退。
    pub clock_order_valid: bool,
}

/// TUI 首字遥测快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstTokenTelemetrySnapshot {
    /// 当前进程是否启用首字遥测。
    pub enabled: bool,
    /// 固定容量的最近样本。
    pub recent_samples: Vec<FirstTokenSample>,
    /// submit 到首个渲染完成的分位数。
    pub submit_to_render: FirstTokenLatencyPercentiles,
    /// Provider 首个有效输出到首个渲染完成的分位数。
    pub provider_delta_to_render: FirstTokenLatencyPercentiles,
}

#[derive(Debug, Clone)]
pub(crate) struct FirstTokenTelemetry {
    enabled: bool,
    current: Option<FirstTokenSample>,
    samples: VecDeque<FirstTokenSample>,
}

impl FirstTokenTelemetry {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            current: None,
            samples: if enabled {
                VecDeque::with_capacity(FIRST_TOKEN_SAMPLE_LIMIT)
            } else {
                VecDeque::new()
            },
        }
    }

    pub(crate) const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn start_submit(&mut self) {
        if !self.enabled {
            return;
        }
        self.close_current();
        self.current = Some(FirstTokenSample {
            submit_started_at_ms: Some(axon_time::unix_now_ms()),
            clock_order_valid: true,
            ..FirstTokenSample::default()
        });
    }

    pub(crate) fn observe_mailbox(
        &mut self,
        timing: Option<&RuntimeTransportTiming>,
        provider_delta_at_ms: Option<u64>,
    ) {
        if !self.enabled {
            return;
        }
        let now_ms = axon_time::unix_now_ms();
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if let Some(timing) = timing {
            current.interaction_id = Some(timing.interaction_id.clone());
            current.ipc_submit_started_at_ms = Some(timing.ipc_submit_started_at_ms);
            current.ipc_submit_finished_at_ms = Some(timing.ipc_submit_finished_at_ms);
            current.interaction_appended_at_ms = Some(timing.interaction_appended_at_ms);
            current.first_ipc_event_received_at_ms = Some(timing.first_ipc_event_received_at_ms);
        }
        current
            .first_mailbox_event_applied_at_ms
            .get_or_insert(now_ms);
        if current.provider_first_delta_parsed_at_ms.is_none() {
            current.provider_first_delta_parsed_at_ms = provider_delta_at_ms;
        }
    }

    pub(crate) fn observe_visible_applied(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(current) = self.current.as_mut() {
            current
                .first_visible_event_applied_at_ms
                .get_or_insert_with(axon_time::unix_now_ms);
        }
    }

    pub(crate) fn record_frame_draw(&mut self) {
        if !self.enabled {
            return;
        }
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if current.first_visible_event_applied_at_ms.is_none() {
            return;
        }
        current.first_render_completed_at_ms = Some(axon_time::unix_now_ms());
        self.close_current();
    }

    pub(crate) fn snapshot(&self) -> FirstTokenTelemetrySnapshot {
        let mut recent_samples = self.samples.iter().cloned().collect::<Vec<_>>();
        if let Some(current) = &self.current {
            recent_samples.push(current.clone());
        }
        let submit_to_render = percentiles(recent_samples.iter().filter_map(|sample| {
            ordered_duration(
                sample.submit_started_at_ms,
                sample.first_render_completed_at_ms,
            )
        }));
        let provider_delta_to_render = percentiles(recent_samples.iter().filter_map(|sample| {
            ordered_duration(
                sample.provider_first_delta_parsed_at_ms,
                sample.first_render_completed_at_ms,
            )
        }));
        FirstTokenTelemetrySnapshot {
            enabled: self.enabled,
            recent_samples,
            submit_to_render,
            provider_delta_to_render,
        }
    }

    fn close_current(&mut self) {
        let Some(mut sample) = self.current.take() else {
            return;
        };
        sample.clock_order_valid = sample_order_is_valid(&sample);
        if self.samples.len() == FIRST_TOKEN_SAMPLE_LIMIT {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }
}

fn sample_order_is_valid(sample: &FirstTokenSample) -> bool {
    let mut previous = None;
    for current in [
        sample.submit_started_at_ms,
        sample.ipc_submit_started_at_ms,
        sample.ipc_submit_finished_at_ms,
        sample.interaction_appended_at_ms,
        sample.first_ipc_event_received_at_ms,
        sample.first_mailbox_event_applied_at_ms,
        sample.first_visible_event_applied_at_ms,
        sample.first_render_completed_at_ms,
    ]
    .into_iter()
    .flatten()
    {
        if previous.is_some_and(|previous| current < previous) {
            return false;
        }
        previous = Some(current);
    }
    match (
        sample.provider_first_delta_parsed_at_ms,
        sample.first_render_completed_at_ms,
    ) {
        (Some(start), Some(end)) => end >= start,
        _ => true,
    }
}

fn ordered_duration(start: Option<u64>, end: Option<u64>) -> Option<u64> {
    let (Some(start), Some(end)) = (start, end) else {
        return None;
    };
    end.checked_sub(start)
}

fn percentiles(samples: impl Iterator<Item = u64>) -> FirstTokenLatencyPercentiles {
    let mut samples = samples.collect::<Vec<_>>();
    samples.sort_unstable();
    FirstTokenLatencyPercentiles {
        sample_count: u64::try_from(samples.len()).unwrap_or(u64::MAX),
        p50_ms: percentile(&samples, 50),
        p95_ms: percentile(&samples, 95),
        p99_ms: percentile(&samples, 99),
    }
}

fn percentile(samples: &[u64], percentile: usize) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let rank = samples.len().saturating_mul(percentile).saturating_add(99) / 100;
    samples.get(rank.saturating_sub(1)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_nearest_rank() {
        let values = percentiles((1..=100).map(|value| value as u64));
        assert_eq!(values.p50_ms, Some(50));
        assert_eq!(values.p95_ms, Some(95));
        assert_eq!(values.p99_ms, Some(99));
    }

    #[test]
    fn sample_capacity_is_bounded() {
        let mut telemetry = FirstTokenTelemetry::new(true);
        for _ in 0..=FIRST_TOKEN_SAMPLE_LIMIT {
            telemetry.start_submit();
        }
        assert_eq!(telemetry.samples.len(), FIRST_TOKEN_SAMPLE_LIMIT);
    }

    #[test]
    fn reversed_clock_does_not_produce_latency() {
        assert_eq!(ordered_duration(Some(20), Some(10)), None);
        let sample = FirstTokenSample {
            submit_started_at_ms: Some(20),
            first_render_completed_at_ms: Some(10),
            clock_order_valid: true,
            ..FirstTokenSample::default()
        };
        assert!(!sample_order_is_valid(&sample));
    }

    #[test]
    fn first_visible_render_is_recorded_once() {
        let mut telemetry = FirstTokenTelemetry::new(true);
        telemetry.start_submit();
        telemetry.observe_visible_applied();
        telemetry.record_frame_draw();
        telemetry.record_frame_draw();

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.recent_samples.len(), 1);
        assert!(
            snapshot.recent_samples[0]
                .first_render_completed_at_ms
                .is_some()
        );
    }

    #[test]
    fn disabled_collector_does_not_create_samples() {
        let mut telemetry = FirstTokenTelemetry::new(false);
        telemetry.start_submit();
        telemetry.observe_visible_applied();
        telemetry.record_frame_draw();

        assert!(telemetry.snapshot().recent_samples.is_empty());
    }
}
