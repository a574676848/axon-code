//! 作业生命周期广播测试：只验事件形状与投递语义，不碰注册表内部状态。

use std::sync::{Arc, Mutex};

use super::{
    JobCompleted, JobLifecycleEvent, JobStarted, publish_job_lifecycle, subscribe_job_lifecycle,
};
use crate::job_runtime::producer::{JobProducerKind, JobTerminalKind};

fn collect(
    owner: &'static str,
) -> (
    Arc<Mutex<Vec<JobLifecycleEvent>>>,
    super::JobLifecycleSubscription,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let subscription = subscribe_job_lifecycle(Arc::new(move |event| {
        // 事件总线是进程级的：测试并行运行时只认自己那条会话的事件。
        if event.owner_id() != owner {
            return;
        }
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.clone());
    }));
    (seen, subscription)
}

#[test]
fn started_and_completed_events_reach_subscribers_in_order() {
    let (seen, _subscription) = collect("run-session-1");
    publish_job_lifecycle(JobLifecycleEvent::Started(JobStarted {
        job_id: 7,
        owner_id: "run-session-1".to_owned(),
        producer: JobProducerKind::Shell,
        label: "cargo test".to_owned(),
        started_at_ms: 1_000,
    }));
    publish_job_lifecycle(JobLifecycleEvent::Completed(JobCompleted {
        job_id: 7,
        owner_id: "run-session-1".to_owned(),
        producer: JobProducerKind::Shell,
        terminal: JobTerminalKind::Completed,
        reason: Some("正常结束".to_owned()),
        exit_code: Some(0),
        elapsed_ms: Some(1_500),
        started_at_ms: 1_000,
        ended_at_ms: 2_500,
    }));

    let seen = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(seen.len(), 2);
    assert!(matches!(&seen[0], JobLifecycleEvent::Started(started) if started.job_id == 7));
    let JobLifecycleEvent::Completed(completed) = &seen[1] else {
        panic!("第二条必须是终态事件");
    };
    assert_eq!(completed.job_id, 7);
    assert_eq!(completed.terminal, JobTerminalKind::Completed);
    assert_eq!(completed.elapsed_ms, Some(1_500));
}

#[test]
fn unelapsed_terminal_keeps_none_instead_of_zero() {
    let event = JobLifecycleEvent::Completed(JobCompleted {
        job_id: 9,
        owner_id: "run-session-2".to_owned(),
        producer: JobProducerKind::Subagent,
        terminal: JobTerminalKind::Killed,
        reason: None,
        exit_code: None,
        elapsed_ms: None,
        started_at_ms: 10,
        ended_at_ms: 20,
    });
    assert_eq!(event.owner_id(), "run-session-2");
    let JobLifecycleEvent::Completed(completed) = event else {
        panic!("应为终态事件");
    };
    assert_eq!(completed.elapsed_ms, None);
}

#[test]
fn dropped_subscription_stops_delivery() {
    let (seen, subscription) = collect("run-session-3");
    drop(subscription);
    publish_job_lifecycle(JobLifecycleEvent::Started(JobStarted {
        job_id: 11,
        owner_id: "run-session-3".to_owned(),
        producer: JobProducerKind::Shell,
        label: "ls".to_owned(),
        started_at_ms: 1,
    }));
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "退订后不应再收到事件"
    );
}
