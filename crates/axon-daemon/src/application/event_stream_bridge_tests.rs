use super::*;

use axon_domain::DaemonSessionId;

fn event(session_id: &DaemonSessionId, sequence: u64, generation: &str) -> SessionEventRecord {
    SessionEventRecord {
        session_id: session_id.clone(),
        sequence,
        emitted_at_ms: sequence,
        generation: generation.to_owned(),
        event_type: "turn_delta".to_owned(),
        payload_json: format!("{{\"interactionId\":\"interaction-a\",\"sequence\":{sequence}}}"),
    }
}

fn interaction_id(value: &str) -> InteractionId {
    InteractionId::new(value).expect("Interaction ID")
}

#[test]
fn subscription_filters_resume_cursor_and_session() {
    let bridge = EventStreamBridge::new();
    let session = DaemonSessionId::new("stream-session").expect("Session ID");
    let other = DaemonSessionId::new("other-session").expect("Session ID");
    let subscription = bridge
        .subscribe(
            session.clone(),
            interaction_id("interaction-a"),
            Some(ConsumerCursor::new("generation-a", 2)),
        )
        .expect("订阅应成功");
    bridge.publish(
        &event(&session, 2, "generation-a"),
        &ConsumerCursor::new("generation-a", 2),
    );
    bridge.publish(
        &event(&other, 3, "generation-a"),
        &ConsumerCursor::new("generation-a", 3),
    );
    bridge.publish(
        &event(&session, 3, "generation-a"),
        &ConsumerCursor::new("generation-a", 3),
    );

    assert_eq!(
        subscription.recv().expect("应收到新事件"),
        StreamNotification::SessionEvent(StreamEvent {
            event: event(&session, 3, "generation-a"),
            cursor: ConsumerCursor::new("generation-a", 3),
        })
    );
}

#[test]
fn reset_terminates_old_subscription_after_queued_events_are_drained() {
    let bridge = EventStreamBridge::new();
    let session = DaemonSessionId::new("lag-session").expect("Session ID");
    let interaction = interaction_id("interaction-a");
    let subscription = bridge
        .subscribe_with_capacity(session.clone(), interaction.clone(), None, 1)
        .expect("订阅应成功");

    for sequence in 1..=3 {
        bridge.publish_for_interaction(
            &interaction,
            &event(&session, sequence, "generation-a"),
            &ConsumerCursor::new("generation-a", sequence),
        );
    }

    assert!(matches!(
        subscription.recv().expect("已入队事件必须先投影"),
        StreamNotification::SessionEvent(StreamEvent { cursor, .. }) if cursor.sequence == 1
    ));
    assert_eq!(
        subscription.recv().expect("落后订阅必须收到 reset"),
        StreamNotification::StreamReset {
            cursor: Some(ConsumerCursor::new("generation-a", 1)),
            reason: STREAM_RESET_REASON_SUBSCRIBER_LAG,
        }
    );
    assert_eq!(bridge.subscriber_count(), 0);
}

#[test]
fn generation_change_resets_from_last_confirmed_cursor() {
    let bridge = EventStreamBridge::new();
    let session = DaemonSessionId::new("generation-session").expect("Session ID");
    let subscription = bridge
        .subscribe(session.clone(), interaction_id("interaction-a"), None)
        .expect("订阅应成功");
    bridge.publish(
        &event(&session, 1, "generation-a"),
        &ConsumerCursor::new("generation-a", 1),
    );
    assert!(matches!(
        subscription.recv().expect("首个事件"),
        StreamNotification::SessionEvent(_)
    ));
    bridge.publish(
        &event(&session, 1, "generation-b"),
        &ConsumerCursor::new("generation-b", 1),
    );

    assert_eq!(
        subscription.recv().expect("generation 变化必须 reset"),
        StreamNotification::StreamReset {
            cursor: Some(ConsumerCursor::new("generation-a", 1)),
            reason: STREAM_RESET_REASON_GENERATION_CHANGE,
        }
    );
    assert_eq!(bridge.subscriber_count(), 0);
}

#[test]
fn typed_publish_does_not_parse_payload_json() {
    let bridge = EventStreamBridge::new();
    let session = DaemonSessionId::new("typed-session").expect("Session ID");
    let interaction = interaction_id("interaction-a");
    let subscription = bridge
        .subscribe(session.clone(), interaction.clone(), None)
        .expect("订阅应成功");
    let mut malformed = event(&session, 1, "generation-a");
    malformed.payload_json = "{malformed".to_owned();

    bridge.publish_for_interaction(
        &interaction,
        &malformed,
        &ConsumerCursor::new("generation-a", 1),
    );

    assert!(matches!(
        subscription.recv().expect("类型化发布应投递"),
        StreamNotification::SessionEvent(StreamEvent { cursor, .. }) if cursor.sequence == 1
    ));
}

#[test]
fn subscriber_limit_is_bounded() {
    let bridge = EventStreamBridge::new();
    let session = DaemonSessionId::new("bounded-session").expect("Session ID");
    let subscriptions = (0..MAX_EVENT_STREAM_SUBSCRIBERS)
        .map(|_| {
            bridge
                .subscribe(
                    session.clone(),
                    interaction_id("interaction-over-limit"),
                    None,
                )
                .expect("限额内订阅")
        })
        .collect::<Vec<_>>();

    assert!(
        bridge
            .subscribe(session, interaction_id("interaction-over-limit"), None)
            .is_err()
    );
    assert_eq!(subscriptions.len(), MAX_EVENT_STREAM_SUBSCRIBERS);
}

/// 一次发布批次的实测画像：锁等待均值与峰值、整批墙钟时间、每条事件的实际扇出。
///
/// 单位均为微秒（扇出除外）。
struct PublishProfile {
    lock_avg_us: u64,
    lock_max_us: u64,
    batch_us: u64,
    fan_out_per_publish: u64,
}

/// 订阅数固定为 1 或 2 的对照批次。
///
/// mailbox 容量取远大于批次的大小：默认容量只有 256，批次跑到一半订阅者就会因
/// 满载被淘汰，之后根本没有扇出可比：那样测出来的差异会被噪声淹没。
const PROFILE_BATCH: u64 = 2_000;
const PROFILE_MAILBOX_CAPACITY: usize = 16_384;

/// 用固定批次发布测出 publish 路径的实测画像。
fn publish_profile(
    bridge: &EventStreamBridge,
    session: &DaemonSessionId,
    interaction: &InteractionId,
) -> PublishProfile {
    let before = bridge.telemetry_snapshot();
    let started = std::time::Instant::now();
    for sequence in 0..PROFILE_BATCH {
        bridge.publish_for_interaction(
            interaction,
            &event(session, sequence, "generation-a"),
            &ConsumerCursor::new("generation-a", sequence),
        );
    }
    let batch_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let after = bridge.telemetry_snapshot();
    let count = after.lock_wait_count.saturating_sub(before.lock_wait_count);
    let wait = after
        .lock_wait_total_us
        .saturating_sub(before.lock_wait_total_us);
    PublishProfile {
        lock_avg_us: wait.checked_div(count).unwrap_or(0),
        lock_max_us: after.lock_wait_max_us,
        batch_us,
        fan_out_per_publish: after
            .matched_fan_out_count
            .saturating_sub(before.matched_fan_out_count)
            .checked_div(PROFILE_BATCH)
            .unwrap_or(0),
    }
}

#[test]
fn publish_lock_wait_does_not_grow_with_the_subscriber_count() {
    // 单连接多路订阅让「同一条会话上有两个订阅者」成为常态，因此 publish 路径的
    // registry lock 等待不能随订阅数出现数量级变化。这里用「订阅数 1」与「订阅数 2」
    // 的同一批发布对照，并把实测数字打进输出，供交付说明直接引用。
    //
    // 负载敏感：整批时间按「多次采样取最小值 + 宽松上限」收敛，与仓库既有做法一致。
    const SAMPLES: usize = 5;

    let session = DaemonSessionId::new("lock-wait-session").expect("Session ID");
    let interaction = interaction_id("interaction-a");

    let single = {
        let bridge = EventStreamBridge::new_with_telemetry(true);
        let subscription = bridge
            .subscribe_with_capacity(
                session.clone(),
                interaction.clone(),
                None,
                PROFILE_MAILBOX_CAPACITY,
            )
            .expect("单订阅");
        let profile = best_of(SAMPLES, || publish_profile(&bridge, &session, &interaction));
        assert_eq!(
            profile.fan_out_per_publish, 1,
            "单订阅对照必须真的每条事件都扇出一次，否则测的不是锁竞争"
        );
        drop(subscription);
        assert_eq!(bridge.subscriber_count(), 0, "订阅释放后订阅数必须回落");
        profile
    };

    let paired = {
        let bridge = EventStreamBridge::new_with_telemetry(true);
        let lane = bridge
            .subscribe_with_capacity(
                session.clone(),
                interaction.clone(),
                None,
                PROFILE_MAILBOX_CAPACITY,
            )
            .expect("车道订阅");
        let conversation = bridge
            .subscribe_session_with_capacity(session.clone(), None, PROFILE_MAILBOX_CAPACITY)
            .expect("会话级订阅应与车道订阅并存于同一会话");
        assert_eq!(bridge.subscriber_count(), 2, "两路订阅必须真的并存");
        let profile = best_of(SAMPLES, || publish_profile(&bridge, &session, &interaction));
        assert_eq!(
            profile.fan_out_per_publish, 2,
            "双订阅对照必须真的每条事件扇出两次（车道那一路命中，会话那一路全收）"
        );
        // 只释放其中一路：另一路必须继续在册。
        drop(lane);
        assert_eq!(
            bridge.subscriber_count(),
            1,
            "只释放一路订阅时，另一路不得被连带摘除"
        );
        drop(conversation);
        assert_eq!(bridge.subscriber_count(), 0);
        profile
    };

    println!(
        "{{\"metric\":\"publish_path\",\"batch\":{PROFILE_BATCH},\"samples\":{SAMPLES},\
\"single_subscriber\":{{\"fan_out\":{},\"lock_avg_us\":{},\"lock_max_us\":{},\"batch_us\":{}}},\
\"two_subscribers\":{{\"fan_out\":{},\"lock_avg_us\":{},\"lock_max_us\":{},\"batch_us\":{}}}}}",
        single.fan_out_per_publish,
        single.lock_avg_us,
        single.lock_max_us,
        single.batch_us,
        paired.fan_out_per_publish,
        paired.lock_avg_us,
        paired.lock_max_us,
        paired.batch_us
    );
    assert!(
        paired.lock_avg_us <= single.lock_avg_us.saturating_mul(2) + 10,
        "订阅数 1 → 2 时 publish 锁等待不得出现数量级变化：单订阅均值 {}us / 峰值 {}us，双订阅均值 {}us / 峰值 {}us",
        single.lock_avg_us,
        single.lock_max_us,
        paired.lock_avg_us,
        paired.lock_max_us
    );
    assert!(
        paired.batch_us <= single.batch_us.saturating_mul(3) + 500,
        "订阅数 1 → 2 时整批发布墙钟时间不得出现数量级变化：单订阅 {}us，双订阅 {}us",
        single.batch_us,
        paired.batch_us
    );
}

/// 多次采样逐项取最优：单次调度抖动不会把结论带偏。
///
/// 三个时间读数各取各的最优：锁等待是这条路径自己的开销，整批墙钟则会把机器负载
/// 算进来。整批墙钟若只跟着锁峰值挑样本，一次被抢占的采样就能让断言读成「多了一路
/// 订阅就慢一个量级」，而实际锁等待始终是微秒级。
fn best_of(samples: usize, mut measure: impl FnMut() -> PublishProfile) -> PublishProfile {
    let mut best = measure();
    for _ in 1..samples {
        let candidate = measure();
        best.lock_max_us = best.lock_max_us.min(candidate.lock_max_us);
        best.lock_avg_us = best.lock_avg_us.min(candidate.lock_avg_us);
        best.batch_us = best.batch_us.min(candidate.batch_us);
    }
    best
}
