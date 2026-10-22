//! Stream commit tick 的 Smooth/CatchUp 自适应策略。

use std::time::{Duration, Instant};

const ENTER_QUEUE_DEPTH_LINES: usize = 8;
const ENTER_OLDEST_AGE: Duration = Duration::from_millis(120);
const EXIT_QUEUE_DEPTH_LINES: usize = 2;
const EXIT_OLDEST_AGE: Duration = Duration::from_millis(40);
const EXIT_HOLD: Duration = Duration::from_millis(250);
const REENTER_CATCH_UP_HOLD: Duration = Duration::from_millis(250);
const SEVERE_QUEUE_DEPTH_LINES: usize = 64;
const SEVERE_OLDEST_AGE: Duration = Duration::from_millis(300);
const CATCH_UP_BATCH_MAX_LINES: usize = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ChunkingMode {
    #[default]
    Smooth,
    CatchUp,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct QueueSnapshot {
    pub(crate) queued_lines: usize,
    pub(crate) oldest_age: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainPlan {
    Single,
    Batch(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkingDecision {
    pub(crate) mode: ChunkingMode,
    pub(crate) entered_catch_up: bool,
    pub(crate) drain_plan: DrainPlan,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct AdaptiveChunkingPolicy {
    mode: ChunkingMode,
    below_exit_threshold_since: Option<Instant>,
    last_catch_up_exit_at: Option<Instant>,
}

impl AdaptiveChunkingPolicy {
    pub(crate) fn reset(&mut self) {
        self.mode = ChunkingMode::Smooth;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
    }

    pub(crate) fn decide(&mut self, snapshot: QueueSnapshot, now: Instant) -> ChunkingDecision {
        if snapshot.queued_lines == 0 {
            self.note_catch_up_exit(now);
            self.mode = ChunkingMode::Smooth;
            self.below_exit_threshold_since = None;
            return ChunkingDecision {
                mode: self.mode,
                entered_catch_up: false,
                drain_plan: DrainPlan::Single,
            };
        }

        let entered_catch_up = match self.mode {
            ChunkingMode::Smooth => self.maybe_enter_catch_up(snapshot, now),
            ChunkingMode::CatchUp => {
                self.maybe_exit_catch_up(snapshot, now);
                false
            }
        };
        let drain_plan = match self.mode {
            ChunkingMode::Smooth => DrainPlan::Single,
            ChunkingMode::CatchUp => DrainPlan::Batch(catch_up_batch_lines(snapshot)),
        };

        ChunkingDecision {
            mode: self.mode,
            entered_catch_up,
            drain_plan,
        }
    }

    fn maybe_enter_catch_up(&mut self, snapshot: QueueSnapshot, now: Instant) -> bool {
        if !should_enter_catch_up(snapshot) {
            return false;
        }
        if self.reentry_hold_active(now) && !is_severe_backlog(snapshot) {
            return false;
        }
        self.mode = ChunkingMode::CatchUp;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
        true
    }

    fn maybe_exit_catch_up(&mut self, snapshot: QueueSnapshot, now: Instant) {
        if !should_exit_catch_up(snapshot) {
            self.below_exit_threshold_since = None;
            return;
        }
        match self.below_exit_threshold_since {
            Some(since) if now.saturating_duration_since(since) >= EXIT_HOLD => {
                self.mode = ChunkingMode::Smooth;
                self.below_exit_threshold_since = None;
                self.last_catch_up_exit_at = Some(now);
            }
            Some(_) => {}
            None => self.below_exit_threshold_since = Some(now),
        }
    }

    fn note_catch_up_exit(&mut self, now: Instant) {
        if self.mode == ChunkingMode::CatchUp {
            self.last_catch_up_exit_at = Some(now);
        }
    }

    fn reentry_hold_active(&self, now: Instant) -> bool {
        self.last_catch_up_exit_at
            .is_some_and(|exit| now.saturating_duration_since(exit) < REENTER_CATCH_UP_HOLD)
    }
}

fn catch_up_batch_lines(snapshot: QueueSnapshot) -> usize {
    snapshot.queued_lines.clamp(1, CATCH_UP_BATCH_MAX_LINES)
}

fn should_enter_catch_up(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines >= ENTER_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|oldest| oldest >= ENTER_OLDEST_AGE)
}

fn should_exit_catch_up(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines <= EXIT_QUEUE_DEPTH_LINES
        && snapshot
            .oldest_age
            .is_some_and(|oldest| oldest <= EXIT_OLDEST_AGE)
}

fn is_severe_backlog(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines >= SEVERE_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|oldest| oldest >= SEVERE_OLDEST_AGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(lines: usize, age_ms: u64) -> QueueSnapshot {
        QueueSnapshot {
            queued_lines: lines,
            oldest_age: Some(Duration::from_millis(age_ms)),
        }
    }

    #[test]
    fn smooth_drains_single_until_pressure_threshold() {
        let mut policy = AdaptiveChunkingPolicy::default();
        let decision = policy.decide(snapshot(2, 20), Instant::now());

        assert_eq!(decision.mode, ChunkingMode::Smooth);
        assert_eq!(decision.drain_plan, DrainPlan::Single);
    }

    #[test]
    fn catch_up_drains_current_backlog_by_depth_or_age() {
        let mut depth_policy = AdaptiveChunkingPolicy::default();
        let now = Instant::now();

        let by_depth = depth_policy.decide(snapshot(8, 20), now);
        assert_eq!(by_depth.mode, ChunkingMode::CatchUp);
        assert_eq!(by_depth.drain_plan, DrainPlan::Batch(8));

        let mut age_policy = AdaptiveChunkingPolicy::default();
        let by_age = age_policy.decide(snapshot(2, 120), now);
        assert_eq!(by_age.mode, ChunkingMode::CatchUp);
        assert_eq!(by_age.drain_plan, DrainPlan::Batch(2));
    }

    #[test]
    fn catch_up_batch_is_capped_to_frame_budget() {
        let mut policy = AdaptiveChunkingPolicy::default();
        let decision = policy.decide(snapshot(80, 400), Instant::now());

        assert_eq!(decision.mode, ChunkingMode::CatchUp);
        assert_eq!(decision.drain_plan, DrainPlan::Batch(16));
    }

    #[test]
    fn catch_up_exits_only_after_hysteresis_hold() {
        let mut policy = AdaptiveChunkingPolicy::default();
        let t0 = Instant::now();

        policy.decide(snapshot(8, 20), t0);
        let before_hold = policy.decide(snapshot(2, 40), t0 + Duration::from_millis(200));
        let after_hold = policy.decide(snapshot(2, 40), t0 + Duration::from_millis(460));

        assert_eq!(before_hold.mode, ChunkingMode::CatchUp);
        assert_eq!(after_hold.mode, ChunkingMode::Smooth);
    }
}
