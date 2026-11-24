//! mailbox 落盘、幂等、目标不可用、重启恢复与有界重试的内核侧测试
//! （合同 §5.3.1 / §5.3.2 / §5.3.5 / §8.2.5）。
//!
//! 假持久化与夹具在 [`super::test_support`]；本文件只描述契约要求的可观察行为。

use std::sync::Arc;

use super::test_support::*;
use super::{AgentMailboxDeliveryState, MailboxDeliveryReport};
use crate::agent_communication::AgentStatus;
use crate::agent_control_runtime::{AgentControlRuntime, AgentPath};
use crate::runtime_limits::MAILBOX_DELIVERY_MAX_ATTEMPTS;

#[test]
fn detached_persistence_keeps_memory_only_behaviour() {
    let mut runtime = AgentControlRuntime::default();
    let review = runtime.spawn(spawn_request("review")).expect("spawn");

    let report = runtime
        .send_message(collab_request(
            AgentPath::root(),
            review.path.clone(),
            "继续处理",
        ))
        .expect("发送");

    assert_eq!(report.delivery_state, AgentMailboxDeliveryState::Delivered);
    assert_eq!(report.queued_count, 1);
    assert_eq!(runtime.mailbox_len(&review.path), 1);
    // 未绑定持久化时重试是空操作，不产生任何副作用。
    assert_eq!(
        runtime.drain_mailbox_delivery(NOW_MS).expect("重试"),
        MailboxDeliveryReport::default()
    );
}

#[test]
fn attached_persistence_records_message_and_settles_delivery() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");

    let report = runtime
        .send_message(collab_request(
            AgentPath::root(),
            review.path.clone(),
            "继续处理",
        ))
        .expect("发送");

    assert_eq!(report.delivery_state, AgentMailboxDeliveryState::Delivered);
    let persisted = persistence.persisted();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].recipient, "/root/review");
    assert_eq!(persisted[0].author, "/root");
    assert_eq!(
        persisted[0].idempotency_key, "collab:session-1:/root:/root/review:1000",
        "幂等键必须跨进程稳定：作用域 + 两类路径 + 入队时刻，不含进程内计数"
    );
    assert_eq!(persistence.pending_count(), 0, "已投递的消息不再待投递");
}

#[test]
fn duplicate_idempotency_key_delivers_exactly_once() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    let recipient = review.path.clone();

    let first = runtime
        .send_message(collab_request(
            AgentPath::root(),
            recipient.clone(),
            "重复投递",
        ))
        .expect("首次发送");
    assert_eq!(first.queued_count, 1);
    // 同一次调用的重复投递（同作者、同接收方、同入队时刻）不得再次入队。
    let second = runtime
        .send_message(collab_request(AgentPath::root(), recipient, "重复投递"))
        .expect("重复发送");

    assert_eq!(persistence.persisted().len(), 1, "同幂等键只落一条消息");
    assert_eq!(
        second.queued_count, 1,
        "重复投递不再追加：mailbox 仍只有首次那一条"
    );
    assert_eq!(second.delivery_state, AgentMailboxDeliveryState::Delivered);
    assert_eq!(runtime.mailbox_len(&review.path), 1);
}

#[test]
fn unavailable_recipient_stays_pending_instead_of_dropping() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    mark_recipient_closed(&mut runtime, &review.path);

    let report = runtime
        .send_message(collab_request(
            AgentPath::root(),
            review.path.clone(),
            "迟到的后续指令",
        ))
        .expect("发送");

    assert_eq!(
        report.delivery_state,
        AgentMailboxDeliveryState::Pending,
        "目标不可接收时必须进入待投递而不是报错或丢弃"
    );
    assert_eq!(
        runtime.mailbox_len(&review.path),
        0,
        "未交付不进内存 mailbox"
    );
    assert_eq!(persistence.persisted().len(), 1, "消息必须已持久化");
    assert_eq!(persistence.pending_count(), 1);
    let failures = persistence.failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].0,
        persistence.persisted()[0].message_id,
        "失败记账必须指向刚落盘的那条消息"
    );
    assert_eq!(failures[0].1, "agent_control:recipient_unavailable");
}

#[test]
fn unknown_recipient_is_still_rejected_without_persisting() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    let mut runtime = attached_runtime(&persistence);
    let unknown = AgentPath::parse("/root/ghost").expect("路径");

    let error = runtime
        .send_message(collab_request(AgentPath::root(), unknown, "给不存在的目标"))
        .expect_err("不存在的目标必须稳定拒绝");

    assert!(format!("{error:?}").contains("agent_control:not_found"));
    assert!(
        persistence.persisted().is_empty(),
        "非法目标不得落盘：它是拒绝而不是待投递"
    );
}

#[test]
fn hydrate_restores_pending_messages_after_restart() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    persistence.seed_pending(seed_request(
        "message-from-previous-process",
        "上一进程遗留的指令",
    ));

    // 新进程语义：控制面重建后 mailbox 为空，恢复动作把它装回来。
    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    assert_eq!(runtime.mailbox_len(&review.path), 0);

    let restored = runtime.hydrate_mailbox(1_200).expect("恢复");

    assert_eq!(restored, 1);
    assert_eq!(runtime.mailbox_len(&review.path), 1);
    let snapshot = runtime.mailbox_snapshot(&review.path).expect("快照");
    assert_eq!(snapshot.messages[0].author, AgentPath::root());
    assert_eq!(snapshot.messages[0].content, "上一进程遗留的指令");
    assert_eq!(snapshot.messages[0].created_at_ms, NOW_MS);
    // 恢复即视为交付：同一条消息不会在下次启动被重复装回。
    assert_eq!(persistence.pending_count(), 0);
}

#[test]
fn hydrate_is_a_noop_without_persistence() {
    let mut runtime = AgentControlRuntime::default();
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    assert_eq!(runtime.hydrate_mailbox(NOW_MS).expect("恢复"), 0);
    assert_eq!(runtime.mailbox_len(&review.path), 0);
}

#[test]
fn retry_delivers_pending_messages_once_the_recipient_is_available() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    persistence.seed_pending(seed_request("message-1", "等待重试的指令"));

    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    mark_recipient_unavailable(&mut runtime, &review.path);
    assert_eq!(runtime.mailbox_len(&review.path), 0);

    // 目标仍不可接收：本次重试不投递，但必须留下可观察的重试记账。
    let blocked = runtime.drain_mailbox_delivery(NOW_MS).expect("重试");
    assert_eq!(blocked.retried, 1);
    assert_eq!(blocked.delivered, 0);
    assert_eq!(blocked.still_pending, 1);
    assert_eq!(persistence.attempt_count("message-1"), 1);
    assert_eq!(runtime.mailbox_len(&review.path), 0);

    // 目标恢复可接收后，同一条持久记录被重新投递：消息没有丢。
    // 推进时刻必须跨过内核实际下发的退避窗口，否则记录尚未到期。
    runtime
        .update_status(&review.path, AgentStatus::Running)
        .expect("目标恢复");
    let delivered = runtime
        .drain_mailbox_delivery(NOW_MS + production_first_backoff_ms())
        .expect("重试");
    assert_eq!(delivered.retried, 1);
    assert_eq!(delivered.delivered, 1);
    assert_eq!(delivered.still_pending, 0);
    assert_eq!(runtime.mailbox_len(&review.path), 1);
    assert_eq!(persistence.pending_count(), 0);
    assert_eq!(
        persistence.persisted().len(),
        1,
        "重试复用同一条持久记录，不产生新消息"
    );
}

#[test]
fn retry_scan_honours_the_backoff_window() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    persistence.seed_pending(seed_request("message-1", "退避中的指令"));

    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    mark_recipient_unavailable(&mut runtime, &review.path);

    let first = runtime.drain_mailbox_delivery(NOW_MS).expect("首次重试");
    assert_eq!(first.still_pending, 1);
    assert_eq!(persistence.attempt_count("message-1"), 1);

    // 退避窗口内不再重试：到期集合为空，而不是重复记账。
    let immediate = runtime
        .drain_mailbox_delivery(NOW_MS + 1)
        .expect("窗口内重试");
    assert_eq!(immediate.retried, 0);
    assert_eq!(persistence.attempt_count("message-1"), 1);
}

#[test]
fn retry_expires_messages_that_reach_the_attempt_limit() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    persistence.seed_pending(seed_request("message-1", "终将失效的指令"));

    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    mark_recipient_unavailable(&mut runtime, &review.path);

    // 目标始终不可接收：每次跨过退避上限重试，直至达到尝试上限。
    let mut now = NOW_MS;
    for _ in 0..u64::from(MAILBOX_DELIVERY_MAX_ATTEMPTS) {
        let report = runtime.drain_mailbox_delivery(now).expect("重试");
        assert_eq!(report.delivered, 0);
        now = now.saturating_add(production_max_backoff_ms());
    }

    assert_eq!(
        persistence.attempt_count("message-1"),
        MAILBOX_DELIVERY_MAX_ATTEMPTS
    );
    assert_eq!(
        persistence.pending_count(),
        0,
        "达到尝试上限后必须退出待投递，而不是无限重试"
    );
    // 终态之后不再有可重试的记录。
    let settled = runtime
        .drain_mailbox_delivery(now + production_max_backoff_ms())
        .expect("重试");
    assert_eq!(settled.retried, 0);
}

#[test]
fn retry_settles_messages_past_the_retention_window() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    let mut request = seed_request("message-1", "过期指令");
    request.expires_at_ms = NOW_MS + 10;
    persistence.seed_pending(request);

    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    runtime
        .update_status(&review.path, AgentStatus::Running)
        .expect("目标可接收");

    let report = runtime.drain_mailbox_delivery(NOW_MS + 10).expect("重试");
    assert_eq!(report.expired, 1, "越过保留窗口必须被结算为失效");
    assert_eq!(report.retried, 0, "失效消息不再参与重试");
    assert_eq!(report.delivered, 0);
    assert_eq!(persistence.pending_count(), 0);
}

#[test]
fn idle_recipient_takes_delivery_and_asks_the_host_to_wake_it() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    mark_recipient_unavailable(&mut runtime, &review.path);

    let report = runtime
        .send_message(collab_request(
            AgentPath::root(),
            review.path.clone(),
            "继续推进",
        ))
        .expect("发送");

    assert_eq!(
        report.delivery_state,
        AgentMailboxDeliveryState::Delivered,
        "空闲（已终态、未关闭）是合法可送达目标，不是待投递"
    );
    assert!(
        report.wake_required,
        "空闲目标必须请求唤醒：只有宿主能重挂会话、开新一轮"
    );
    assert_eq!(runtime.mailbox_len(&review.path), 1);
}

#[test]
fn consumed_messages_are_read_confirmed_and_not_injected_twice() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");
    runtime
        .send_message(collab_request(
            AgentPath::root(),
            review.path.clone(),
            "第一次协作",
        ))
        .expect("发送");

    let first = runtime
        .take_mailbox_messages(&review.path, 1_100)
        .expect("消费");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].content, "第一次协作");
    assert!(first[0].message_id.is_some(), "持久消息必须携带记录 ID");
    assert!(
        persistence.delivered_unread().is_empty(),
        "消费即读确认：已投递未读集合随之清空"
    );
    assert!(
        runtime
            .take_mailbox_messages(&review.path, 1_200)
            .expect("二次消费")
            .is_empty(),
        "唯一出口：第二次取走必须为空，否则同一批消息会被注入两次"
    );
}

#[test]
fn hydrate_restores_delivered_unread_messages_after_restart() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    persistence.seed_delivered(seed_request("delivered-unread", "上一进程已送达未读"));

    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");

    let restored = runtime.hydrate_mailbox(1_200).expect("恢复");

    assert_eq!(restored, 1, "Delivered 不是终态，必须装回");
    let snapshot = runtime.mailbox_snapshot(&review.path).expect("快照");
    assert_eq!(snapshot.messages[0].content, "上一进程已送达未读");
    assert!(
        !persistence.delivered_unread().is_empty(),
        "装回不改变投递与读状态：未读仍然是未读"
    );
}

/// 一次性装回自带幂等保护：重复调用不得把同一条持久消息推入内存多次。
#[test]
fn hydrate_once_is_idempotent() {
    let persistence = Arc::new(FakeMailboxPersistence::default());
    persistence.seed_delivered(seed_request("delivered-unread", "上一进程已送达未读"));

    let mut runtime = attached_runtime(&persistence);
    let review = runtime.spawn(spawn_request("review")).expect("spawn");

    assert_eq!(runtime.hydrate_mailbox_once(1_200).expect("首次装回"), 1);
    assert_eq!(
        runtime.hydrate_mailbox_once(1_300).expect("再次装回"),
        0,
        "重复装回必须是空操作，否则同一批消息会被注入两次"
    );
    assert_eq!(
        runtime.mailbox_len(&review.path),
        1,
        "内存 mailbox 不得出现重复副本"
    );
}
