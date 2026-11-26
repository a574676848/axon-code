//! 投影队列的顺序、溢出兜底与失败隔离。

use std::sync::{Arc, Mutex};

use axon_domain::{DaemonSessionId, RuntimeHostId};
use axon_eventlog::TurnMetrics;

use super::{ProjectionJob, ProjectionOutcome, ProjectionQueue, submit};
use crate::SqliteRuntimeStore;
use crate::ports::{EventLogStore as _, RuntimeStore as _, RuntimeStoreError};

fn fixture(
    name: &str,
) -> (
    SqliteRuntimeStore,
    DaemonSessionId,
    crate::domain::runtime::DaemonHostRecord,
) {
    let directory = axon_test_support::test_temp_path(name);
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化");
    let host = store
        .claim_host(
            &RuntimeHostId::new("host-1").expect("host id"),
            1,
            None,
            10,
            60_000,
        )
        .expect("claim host");
    let session_id = DaemonSessionId::new("session-projection-worker").expect("session id");
    store
        .create_session(&session_id, ".", 10)
        .expect("建立会话");
    (store, session_id, host)
}

/// 一批只有一条 `turn_metrics` 的事件，指标是累加量。
fn metrics_event(turn_id: &str, active_ms: u64) -> (String, String) {
    let metrics = TurnMetrics {
        turn_id: turn_id.to_owned(),
        active_ms,
        ..TurnMetrics::default()
    };
    (
        "turn_metrics".to_owned(),
        serde_json::to_string(&metrics).expect("序列化指标"),
    )
}

/// 队列项：只带排序用的批首序号，事件留空。
fn job(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
) -> ProjectionJob {
    ProjectionJob {
        store: store.clone(),
        session_id: session_id.clone(),
        start_sequence,
        events: Vec::new(),
        origin_channel: None,
        now_ms: 0,
        log_rewound: false,
    }
}

fn recorded(applied: &Arc<Mutex<Vec<u64>>>) -> Vec<u64> {
    applied
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// 记录执行顺序的替身队列。
fn recorder_queue(capacity: usize) -> (ProjectionQueue, Arc<Mutex<Vec<u64>>>) {
    let applied = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&applied);
    let queue = ProjectionQueue::new(
        capacity,
        Box::new(move |job: ProjectionJob| {
            recorder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(job.start_sequence);
            ProjectionOutcome::Applied
        }),
    );
    (queue, applied)
}

/// 两批投递后，两批的投影都必须读得到（累加语义：轮次数与耗时同时到位）。
#[test]
fn two_delivered_batches_are_projected_after_drain() {
    let (store, session_id, host) = fixture("projection-worker-two-batches");
    store
        .append_events(&session_id, &host, &[metrics_event("turn-1", 800)], 20)
        .expect("第一批");
    store
        .append_events(&session_id, &host, &[metrics_event("turn-2", 400)], 30)
        .expect("第二批");

    let processed = store.drain_for_tests();

    let snapshot = store
        .session_metrics(&session_id)
        .expect("读投影")
        .expect("两批之后必须有投影");
    assert_eq!(snapshot.turn_count, 2, "两批必须各算一轮");
    assert_eq!(snapshot.total_active_ms, 1_200, "两批的耗时必须累加");
    assert!(processed >= 2, "两批都必须被处理过，实际 {processed}");
}

/// 同一批重复投递不得重复累加；更大的首序号才算新的一批。
///
/// 顺带固化「单测构建下投递即执行」：这条用例读到投影时不等待任何队列。
#[test]
fn a_replayed_batch_is_projected_only_once() {
    let (store, session_id, _host) = fixture("projection-worker-replay");
    let batch = [metrics_event("turn-1", 800)];

    submit(&store, &session_id, 7, &batch, Some("desktop"), 20, false);
    let once = store
        .session_metrics(&session_id)
        .expect("读投影")
        .expect("投递之后必须立刻可读");
    assert_eq!(once.turn_count, 1);

    // 水位已停在 7：整批跳过，累计值一格都不许动。
    submit(&store, &session_id, 7, &batch, Some("desktop"), 30, false);
    store.drain_for_tests();
    let replayed = store
        .session_metrics(&session_id)
        .expect("读投影")
        .expect("投影应存在");
    assert_eq!(replayed.turn_count, 1, "同一批重放不得重复计数");
    assert_eq!(replayed.total_active_ms, 800, "同一批重放不得重复累加");

    submit(&store, &session_id, 8, &batch, Some("desktop"), 40, false);
    store.drain_for_tests();
    let advanced = store
        .session_metrics(&session_id)
        .expect("读投影")
        .expect("投影应存在");
    assert_eq!(advanced.turn_count, 2, "新的一批必须照常累加");
    assert_eq!(advanced.total_active_ms, 1_600);
}

/// 事件日志序号回退后必须重置水位：否则这批以及之后每一批都会被闸门整批跳过。
///
/// 现场形态是事件日志被删除、修复重排或截断：控制面仍从高位分配序号，日志落盘时按更短
/// 的尾重排回小号，水位却停在旧的高号上。
#[test]
fn a_rewound_log_resets_the_watermark_before_the_gate() {
    let (store, session_id, _host) = fixture("projection-worker-rewind");
    let batch = [metrics_event("turn-1", 500)];
    submit(&store, &session_id, 42, &batch, Some("desktop"), 10, false);
    assert_eq!(
        store
            .session_metrics(&session_id)
            .expect("读投影")
            .expect("投影存在")
            .turn_count,
        1
    );

    // 重写后的日志从 1 开始：不重置水位的话 start=1 不大于 42，整批会被当成重复跳过。
    let rewritten = [metrics_event("turn-1", 500)];
    submit(
        &store,
        &session_id,
        1,
        &rewritten,
        Some("desktop"),
        20,
        true,
    );
    let metrics = store
        .session_metrics(&session_id)
        .expect("读投影")
        .expect("投影存在");
    assert_eq!(metrics.turn_count, 2, "序号回退后的批次必须重新被投影");
}

/// 重排触发的清水位必须带条件：只有批首序号不大于水位才清。
///
/// 事件面写失败后的正常补偿也会重排，那种情形水位仍低于新批首序号；无条件清会让已经
/// 投影过的历史批被重复累加。
#[test]
fn resetting_the_watermark_requires_the_rewind_to_cross_it() {
    let (store, session_id, _host) = fixture("projection-watermark-rewind-guard");
    let batch = [metrics_event("turn-1", 100)];
    submit(&store, &session_id, 10, &batch, Some("desktop"), 1, false);
    assert_eq!(
        super::super::projection_watermark::watermark(&store, &session_id),
        Some(10),
        "投递后的水位必须落在本批最后一条序号上"
    );

    assert!(
        !super::super::projection_watermark::reset_if_rewound(&store, &session_id, 11),
        "批首序号高于水位时不得清水位"
    );
    assert_eq!(
        super::super::projection_watermark::watermark(&store, &session_id),
        Some(10),
        "未跨过水位时水位必须原样保留"
    );

    assert!(
        super::super::projection_watermark::reset_if_rewound(&store, &session_id, 10),
        "批首序号等于水位说明重排压到了水位上，必须清水位"
    );
    assert_eq!(
        super::super::projection_watermark::watermark(&store, &session_id),
        None,
        "清水位之后这一批才不会被闸门整批跳过"
    );
}

/// 队列满时本会话的积压必须按投递顺序先跑，本批最后跑。
#[test]
fn overflow_runs_the_same_session_backlog_in_delivery_order() {
    let (store, session_id, _host) = fixture("projection-worker-overflow-order");
    let (queue, applied) = recorder_queue(2);

    for start_sequence in 1..=3 {
        queue.enqueue_or_run(job(&store, &session_id, start_sequence));
    }

    assert_eq!(
        recorded(&applied),
        [1, 2, 3],
        "溢出兜底也必须按投递顺序执行"
    );
    assert_eq!(queue.overflow_count(), 1, "只有第三批触发溢出");
    assert_eq!(queue.processed_count(), 3);
    assert_eq!(queue.skipped_count(), 0);
    assert_eq!(queue.failed_count(), 0);
}

/// 溢出只捞本会话的积压：别的会话留在队列里，顺序不受影响。
#[test]
fn overflow_leaves_other_sessions_queued() {
    let (store, session_a, _host) = fixture("projection-worker-overflow-session");
    let session_b = DaemonSessionId::new("session-projection-worker-other").expect("session id");
    let (queue, applied) = recorder_queue(1);

    queue.enqueue_or_run(job(&store, &session_a, 1));
    // 另一条会话的批撞上满队列：它没有积压，本批同步执行。
    queue.enqueue_or_run(job(&store, &session_b, 2));
    assert_eq!(recorded(&applied), [2], "溢出时只执行本批，不碰别的会话");

    // 被撞的会话仍在队列里，按它自己的顺序后跑。
    queue.run_pending_for_tests();
    assert_eq!(recorded(&applied), [2, 1]);
    assert_eq!(queue.overflow_count(), 1);
}

/// 单批失败只计数并留痕，后续批次照常执行。
#[test]
fn a_failing_batch_does_not_stop_the_queue() {
    const FAILING_SEQUENCE: u64 = 2;
    let (store, session_id, _host) = fixture("projection-worker-failure");
    let applied = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&applied);
    let queue = ProjectionQueue::new(
        1,
        Box::new(move |job: ProjectionJob| {
            assert_ne!(job.start_sequence, FAILING_SEQUENCE, "模拟投影执行失败");
            recorder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(job.start_sequence);
            ProjectionOutcome::Applied
        }),
    );

    queue.enqueue_or_run(job(&store, &session_id, 1));
    // 溢出：先跑积压的 1，再跑本批 2（失败）。
    queue.enqueue_or_run(job(&store, &session_id, FAILING_SEQUENCE));
    // 失败之后队列必须还能继续消费。
    queue.enqueue_or_run(job(&store, &session_id, 3));
    queue.run_pending_for_tests();

    assert_eq!(recorded(&applied), [1, 3], "失败那一批不得挡住后面的批");
    assert_eq!(queue.failed_count(), 1);
    assert_eq!(queue.processed_count(), 3);
}

/// 闸门跳过与执行失败要分开计数：跳过不是失败，两者都不得被当成成功。
#[test]
fn queue_counts_skipped_and_failed_outcomes() {
    let (store, session_id, _host) = fixture("projection-worker-outcomes");
    let queue = ProjectionQueue::new(
        1,
        Box::new(|job: ProjectionJob| match job.start_sequence {
            1 => ProjectionOutcome::Skipped { watermark: 7 },
            2 => ProjectionOutcome::Failed {
                error: RuntimeStoreError::Storage("模拟投影写入失败".to_owned()),
            },
            _ => ProjectionOutcome::Applied,
        }),
    );

    for start_sequence in 1..=3 {
        queue.enqueue_or_run(job(&store, &session_id, start_sequence));
    }
    queue.run_pending_for_tests();

    assert_eq!(queue.processed_count(), 3);
    assert_eq!(queue.skipped_count(), 1, "闸门跳过单独计数");
    assert_eq!(queue.failed_count(), 1, "执行失败单独计数");
}

/// 重建入口：清掉派生行与水位后按日志整体重放，三张投影都要回到与日志一致的状态。
#[test]
fn rebuilding_replays_the_log_into_all_three_projections() {
    let (store, session_id, _host) = fixture("projection-rebuild");
    let interaction = "01TURNACTIVITYREBUILD0000001";
    let metrics = axon_eventlog::TurnMetrics {
        turn_id: "turn-1".to_owned(),
        active_ms: 900,
        ..axon_eventlog::TurnMetrics::default()
    };
    let records = vec![
        axon_eventlog::RuntimeEventRecord::new(
            1,
            1_000,
            axon_eventlog::RuntimeEventKind::TurnMetrics,
            serde_json::to_value(&metrics).expect("指标序列化"),
        ),
        axon_eventlog::RuntimeEventRecord::new(
            2,
            1_001,
            axon_eventlog::RuntimeEventKind::TurnCompleted,
            serde_json::json!({
                "interactionId": interaction,
                "response": "重建答案",
            }),
        ),
    ];
    axon_eventlog::append_once(&store.event_log(&session_id), 1, 1_000, &records)
        .expect("写事件日志");

    // 先把状态弄脏：水位推到末尾并留一份错误读数，重建必须把它纠正回来。
    super::projection_watermark::advance(&store, &session_id, 2, 1);
    let replayed = store
        .rebuild_session_projections(&session_id)
        .expect("重建必须成功");
    assert_eq!(replayed, 2, "两条帧都要重放");

    let metrics_row = store
        .session_metrics(&session_id)
        .expect("读指标")
        .expect("重建后必须有指标行");
    assert_eq!(metrics_row.turn_count, 1, "累计轮次来自 turn_metrics");
    assert_eq!(metrics_row.total_active_ms, 900);

    let activity = store
        .turn_activities_for(session_id.as_str(), &[interaction])
        .expect("读逐轮事实");
    let row = activity.get(interaction).expect("重建后必须有逐轮事实");
    assert_eq!(row.answer_text.as_deref(), Some("重建答案"));
}

/// 投影成功后记下日志身份；磁盘上的日志被改写后，启动对账必须能发现并重建。
#[test]
fn log_identity_is_recorded_and_reconcile_detects_a_rewrite() {
    let (store, session_id, _host) = fixture("projection-identity");
    let metrics = axon_eventlog::TurnMetrics {
        turn_id: "turn-1".to_owned(),
        active_ms: 300,
        ..axon_eventlog::TurnMetrics::default()
    };
    let records = vec![axon_eventlog::RuntimeEventRecord::new(
        1,
        1_000,
        axon_eventlog::RuntimeEventKind::TurnMetrics,
        serde_json::to_value(&metrics).expect("指标序列化"),
    )];
    axon_eventlog::append_once(&store.event_log(&session_id), 1, 1_000, &records)
        .expect("写事件日志");
    let batch = [metrics_event("turn-1", 300)];
    submit(&store, &session_id, 1, &batch, Some("desktop"), 10, false);

    let recorded: i64 = store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT log_len FROM projection_log_identity WHERE session_id=?1",
                [session_id.as_str()],
                |row| row.get(0),
            )
        })
        .expect("投影后必须记下日志身份");
    assert!(recorded > 0, "身份里的日志长度必须来自真实文件");

    // 模拟外部改写：把落库的长度改掉，等价于日志被另一份内容替换。
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "UPDATE projection_log_identity SET log_len = log_len + 1 WHERE session_id=?1",
                    [session_id.as_str()],
                )
                .map(|_| ())
        })
        .expect("改写身份");

    let rebuilt = store.reconcile_log_identities().expect("对账必须成功");
    assert_eq!(rebuilt, 1, "身份对不上必须重建该会话");
}
