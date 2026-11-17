//! `session_metrics` 投影的累加语义。

use std::sync::atomic::{AtomicU64, Ordering};

use axon_domain::{DaemonSessionId, RuntimeHostId};
use axon_eventlog::{TurnMetrics, TurnOutcome};

use super::SqliteRuntimeStore;
use crate::ports::{EventLogStore as _, RuntimeStore as _};

/// 夹具目录序号：同一测试线程内多个用例共用一个线程临时根，路径必须逐用例区分。
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fixture() -> (SqliteRuntimeStore, DaemonSessionId) {
    // 目录由测试线程根统一回收，整个用例期间必须一直存在：连接池按路径复用连接，目录
    // 一旦提前消失，Linux 上后续新建的连接会在原地长出一个空库（Windows 上删除仍被打开
    // 的文件会失败，所以本机看不到这个问题），嵌套取连接的投影写入随即 `no such table`。
    let directory = axon_test_support::test_temp_path(&format!(
        "session-metrics-{}",
        FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化");
    let host_id = RuntimeHostId::new("host-1").expect("host id");
    let _host = store
        .claim_host(&host_id, 1, None, 10, 60_000)
        .expect("claim host");
    let session_id = DaemonSessionId::new("session-session-metrics").expect("session id");
    store
        .create_session(&session_id, ".", 10)
        .expect("create session");
    (store, session_id)
}

fn host(store: &SqliteRuntimeStore) -> crate::domain::runtime::DaemonHostRecord {
    store
        .claim_host(
            &RuntimeHostId::new("host-1").expect("host id"),
            1,
            None,
            10,
            60_000,
        )
        .expect("claim host")
}

fn metrics(turn_id: &str, ttft_ms: Option<u64>, active_ms: u64, tool_calls: u32) -> TurnMetrics {
    TurnMetrics {
        turn_id: turn_id.to_owned(),
        ttft_ms,
        active_ms,
        provider_duration_ms: active_ms / 2,
        tool_ms: active_ms / 4,
        input_tokens: 100,
        output_tokens: 40,
        reasoning_tokens: 10,
        cache_read_tokens: 5,
        cache_write_tokens: 3,
        generation_speed_tps: Some(40.5),
        background_jobs_started: 0,
        background_jobs_running: 0,
        outcome: TurnOutcome::Succeeded,
        tool_calls,
    }
}

/// 夹具的库文件必须在整个用例期间留在磁盘上。
///
/// 这是 Linux CI 上三个投影用例失败的根因：夹具曾经在返回时就把临时目录删掉了。Windows
/// 上删除仍被打开的 SQLite 文件会失败，于是同一份代码在本机一直是绿的；Linux 上目录真的
/// 消失，连接池为嵌套取用新建的连接会在原地长出一个空库，投影写入随即撞上
/// `no such table: session_metrics`。本用例固化「夹具目录与 Store 同寿命」这一判据。
#[test]
fn fixture_keeps_its_database_files_on_disk_for_the_whole_case() {
    let (store, _session_id) = fixture();
    assert!(
        store.path().is_file() && store.outbox_path().is_file(),
        "夹具的库文件必须在用例期间存在：{:?} / {:?}",
        store.path(),
        store.outbox_path()
    );
    // 目录活着，同路径的新实例才能读到既有 schema，而不是一个刚长出来的空库。
    let reopened = SqliteRuntimeStore::new(store.path());
    reopened
        .initialize()
        .expect("同路径重新初始化必须命中既有 schema");
}

#[test]
fn turn_metrics_accumulate_into_session_projection() {
    let (store, session_id) = fixture();
    let host_record = host(&store);

    let first = serde_json::to_string(&metrics("t1", Some(120), 800, 2)).expect("序列化");
    store
        .append_events(
            &session_id,
            &host_record,
            &[("turn_metrics".to_owned(), first)],
            20,
        )
        .expect("append first");

    let second = serde_json::to_string(&metrics("t2", None, 1_600, 1)).expect("序列化");
    store
        .append_events(
            &session_id,
            &host_record,
            &[("turn_metrics".to_owned(), second)],
            30,
        )
        .expect("append second");

    let snapshot = store
        .session_metrics(&session_id)
        .expect("读取投影")
        .expect("投影应存在");
    assert_eq!(snapshot.turn_count, 2);
    assert_eq!(snapshot.turn_succeeded, 2);
    assert_eq!(snapshot.total_active_ms, 2_400);
    assert_eq!(snapshot.max_turn_ms, 1_600);
    assert_eq!(snapshot.ttft_samples, 1, "未测得首字不计入样本");
    assert_eq!(snapshot.ttft_sum_ms, 120);
    assert_eq!(snapshot.total_tool_calls, 3);
    assert_eq!(snapshot.total_input_tokens, 200);
    assert_eq!(snapshot.ttft_histogram.len(), 16);
    assert_eq!(snapshot.turn_histogram.len(), 16);
}

#[test]
fn failed_turns_are_counted_separately() {
    let (store, session_id) = fixture();
    let host_record = host(&store);
    let mut failing = metrics("t1", Some(90), 500, 0);
    failing.outcome = TurnOutcome::Failed;
    let payload = serde_json::to_string(&failing).expect("序列化");
    store
        .append_events(
            &session_id,
            &host_record,
            &[("turn_metrics".to_owned(), payload)],
            20,
        )
        .expect("append");

    let snapshot = store
        .session_metrics(&session_id)
        .expect("读取投影")
        .expect("投影应存在");
    assert_eq!(snapshot.turn_count, 1);
    assert_eq!(snapshot.turn_failed, 1);
    assert_eq!(snapshot.turn_succeeded, 0);
}
#[test]
fn job_deltas_track_started_completed_and_running() {
    let (store, session_id) = fixture();
    let repo = super::session_metrics_repository::apply_job_delta;

    repo(&store, &session_id, true, false, 100).expect("start one");
    repo(&store, &session_id, true, false, 110).expect("start two");
    repo(&store, &session_id, false, false, 200).expect("complete one");
    repo(&store, &session_id, false, true, 210).expect("fail one");

    let snapshot = store
        .session_metrics(&session_id)
        .expect("读取投影")
        .expect("投影应存在");
    assert_eq!(snapshot.jobs_started, 2);
    assert_eq!(snapshot.jobs_completed, 2);
    assert_eq!(snapshot.jobs_failed, 1);
    assert_eq!(snapshot.jobs_running, 0, "两个都收口后不应为负");
}

#[test]
fn running_jobs_never_go_negative() {
    let (store, session_id) = fixture();
    let repo = super::session_metrics_repository::apply_job_delta;
    repo(&store, &session_id, false, false, 100).expect("complete without start");
    let snapshot = store
        .session_metrics(&session_id)
        .expect("读取投影")
        .expect("投影应存在");
    assert_eq!(snapshot.jobs_running, 0);
    assert_eq!(snapshot.jobs_completed, 1);
}

#[test]
fn projection_records_the_inbound_channel_instead_of_internal() {
    let (store, session_id) = fixture();
    super::session_metrics_repository::apply_turn_metrics(
        &store,
        &session_id,
        &metrics("t1", None, 10, 0),
        Some("tui"),
        20,
    )
    .expect("写入投影");
    let snapshot = store
        .session_metrics(&session_id)
        .expect("读取投影")
        .expect("投影应存在");
    assert_eq!(snapshot.channel_mix.get("tui"), Some(&1));
    assert!(
        !snapshot.channel_mix.contains_key("internal"),
        "有渠道来源的轮次不得记成 internal：{:?}",
        snapshot.channel_mix
    );
}

#[test]
fn rebuild_replays_the_event_log_and_restores_channel_mix() {
    let (store, session_id) = fixture();
    let mut records = Vec::new();
    for (index, metrics) in [
        metrics("t1", Some(50), 700, 1),
        metrics("t2", None, 1_300, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let sequence = u64::try_from(index).expect("序号");
        let record = axon_eventlog::RuntimeEventRecord::turn_metrics(sequence, 1_000, &metrics)
            .expect("构造事件")
            .with_origin(Some("desktop"), axon_eventlog::TurnOrigin::External);
        records.push(record);
    }
    axon_eventlog::append_once(&store.event_log(&session_id), 0, 1_000, &records)
        .expect("写入事件日志");

    // 投影缺失（模拟写入失败）时，重放必须把它补回来。
    assert!(
        store
            .session_metrics(&session_id)
            .expect("读取投影")
            .is_none()
    );
    let snapshot = store
        .rebuild_session_metrics(&session_id, 2_000)
        .expect("重建投影");
    assert_eq!(snapshot.turn_count, 2);
    assert_eq!(snapshot.total_active_ms, 2_000);
    assert_eq!(snapshot.max_turn_ms, 1_300);
    assert_eq!(snapshot.ttft_samples, 1);
    assert_eq!(snapshot.channel_mix.get("desktop"), Some(&2));
    // 重复重建是幂等的：先删旧行再整体重放。
    let again = store
        .rebuild_session_metrics(&session_id, 3_000)
        .expect("再次重建");
    assert_eq!(again.turn_count, 2);
    assert_eq!(again.total_active_ms, 2_000);
}
