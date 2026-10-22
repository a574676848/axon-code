use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Clone, Default)]
pub(super) struct RealSdkLoopCounters {
    pub input_payloads: Arc<AtomicUsize>,
    pub mapped_events: Arc<AtomicUsize>,
    pub appended_events: Arc<AtomicUsize>,
    pub received_acked_events: Arc<AtomicUsize>,
    pub received_ack_failed_events: Arc<AtomicUsize>,
    pub closure_failed_events: Arc<AtomicUsize>,
}

impl RealSdkLoopCounters {
    pub fn snapshot(&self) -> RealSdkLoopCounterSnapshot {
        RealSdkLoopCounterSnapshot {
            input_payloads: self.input_payloads.load(Ordering::Relaxed),
            mapped_events: self.mapped_events.load(Ordering::Relaxed),
            appended_events: self.appended_events.load(Ordering::Relaxed),
            received_acked_events: self.received_acked_events.load(Ordering::Relaxed),
            received_ack_failed_events: self.received_ack_failed_events.load(Ordering::Relaxed),
            closure_failed_events: self.closure_failed_events.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RealSdkLoopCounterSnapshot {
    pub input_payloads: usize,
    pub mapped_events: usize,
    pub appended_events: usize,
    pub received_acked_events: usize,
    pub received_ack_failed_events: usize,
    pub closure_failed_events: usize,
}
