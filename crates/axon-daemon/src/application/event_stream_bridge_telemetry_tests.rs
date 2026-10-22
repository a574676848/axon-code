use std::time::Instant;

use axon_domain::{ConsumerCursor, DaemonSessionId, InteractionId};
use axon_session::SessionEventRecord;

use super::{EventStreamBridge, StreamNotification};

const NORMAL_DELTA_COUNT: usize = 100;
const BURST_DELTA_COUNT: usize = 1_000;
const P95_LIMIT_US: u64 = 25_000;
const P99_LIMIT_US: u64 = 50_000;

#[test]
fn normal_and_burst_publish_stay_within_projection_budget() {
    run_publish_budget_case(NORMAL_DELTA_COUNT);
    run_publish_budget_case(BURST_DELTA_COUNT);
}

fn run_publish_budget_case(delta_count: usize) {
    let bridge = EventStreamBridge::new_with_telemetry(true);
    let session =
        DaemonSessionId::new(format!("bridge-burst-{delta_count}")).expect("Session ID 应有效");
    let interaction = InteractionId::new("interaction-burst").expect("Interaction ID 应有效");
    let subscription = bridge
        .subscribe_with_capacity(session.clone(), interaction.clone(), None, delta_count)
        .expect("订阅应成功");
    let mut latencies_us = Vec::with_capacity(delta_count);

    for sequence in 1..=delta_count {
        let event = event(
            &session,
            u64::try_from(sequence).expect("sequence 应可转换"),
        );
        let cursor = ConsumerCursor::new("generation-a", event.sequence);
        let started_at = Instant::now();
        bridge.publish_for_interaction(&interaction, &event, &cursor);
        latencies_us.push(u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX));
    }
    for _ in 0..delta_count {
        assert!(matches!(
            subscription.recv().expect("应收到事件"),
            StreamNotification::SessionEvent(_)
        ));
    }

    latencies_us.sort_unstable();
    let p95 = percentile(&latencies_us, 95);
    let p99 = percentile(&latencies_us, 99);
    println!("bridge delta_count={delta_count} p95={p95}us p99={p99}us");
    assert!(p95 < P95_LIMIT_US, "Bridge publish p95={p95}us");
    assert!(p99 < P99_LIMIT_US, "Bridge publish p99={p99}us");

    let snapshot = bridge.telemetry_snapshot();
    assert_eq!(snapshot.publish_count, delta_count as u64);
    assert_eq!(snapshot.delivered_count, delta_count as u64);
    assert_eq!(snapshot.mailbox_full_prune_count, 0);
    assert_eq!(snapshot.disconnected_prune_count, 0);
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let index = sorted.len().saturating_mul(percentile) / 100;
    sorted[index.min(sorted.len().saturating_sub(1))]
}

fn event(session_id: &DaemonSessionId, sequence: u64) -> SessionEventRecord {
    SessionEventRecord {
        session_id: session_id.clone(),
        sequence,
        emitted_at_ms: sequence,
        generation: "generation-a".to_owned(),
        event_type: "turn_delta".to_owned(),
        payload_json: format!(
            "{{\"interactionId\":\"interaction-burst\",\"sequence\":{sequence}}}"
        ),
    }
}
