use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveRuntimeStatus {
    turn_started_at: Option<Instant>,
    last_elapsed: Option<Duration>,
}

impl LiveRuntimeStatus {
    pub(super) fn reset_for_submit(&mut self, now: Instant) {
        self.turn_started_at = Some(now);
        self.last_elapsed = None;
    }

    pub(super) fn finish(&mut self, now: Instant) -> Duration {
        let elapsed = self
            .turn_started_at
            .map(|started_at| now.saturating_duration_since(started_at))
            .or(self.last_elapsed)
            .unwrap_or_default();
        self.turn_started_at = None;
        self.last_elapsed = Some(elapsed);
        elapsed
    }

    pub(super) fn clear(&mut self) {
        self.turn_started_at = None;
        self.last_elapsed = None;
    }

    pub(super) fn display_elapsed(&self, now: Instant) -> Option<Duration> {
        self.turn_started_at
            .map(|started_at| now.saturating_duration_since(started_at))
            .or(self.last_elapsed)
            .filter(|duration| !duration.is_zero())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_runtime_status_tracks_turn_elapsed_time() {
        let mut status = LiveRuntimeStatus::default();
        let now = Instant::now();

        status.reset_for_submit(now);

        assert_eq!(
            status.display_elapsed(now + Duration::from_secs(3)),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn finish_keeps_elapsed_time_for_idle_status_line() {
        let mut status = LiveRuntimeStatus::default();
        let now = Instant::now();
        status.reset_for_submit(now);

        status.finish(now + Duration::from_secs(3));

        assert_eq!(
            status.display_elapsed(now + Duration::from_secs(4)),
            Some(Duration::from_secs(3))
        );
    }
}
