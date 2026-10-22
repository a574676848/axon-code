use axon_session::{NotificationDeliveryState, RuntimeStore};

use crate::cron_commands::runtime_cleanup_families::{
    expired_daemon_hosts_selection_sql, expired_decisions_selection_sql,
    expired_events_selection_sql, expired_session_events_selection_sql,
    expired_upgrade_admissions_selection_sql,
    settled_completion_deliveries_selection_sql_for_state,
    settled_completion_notifications_selection_sql_for_state, terminal_goals_selection_sql,
    terminal_interactions_selection_sql,
};

use super::*;

const HOT_SESSION_ID: &str = "s-hot";
const HOT_SESSION_EVENT_COUNT: i64 = 20_000;
const HOT_SESSION_RECENT_ACTIVITY_MS: i64 = 1_000;
const OLD_EVENT_EMITTED_AT_MS: i64 = 1;
const CLEANUP_CUTOFF_MS: u64 = 100;
const TEST_ROLLOUT_DATABASE_FILE: &str = "rollout.sqlite3";
const RETAINED_ARTIFACT_CONTENT: &[u8] = b"retained-session-artifact\n";
const CLEANUP_PERFORMANCE_SAMPLE_COUNT: usize = 20;
const CLEANUP_PERFORMANCE_P99_BUDGET_US: u128 = 2_000_000;

fn open_fixture(root: &Path) -> Connection {
    let path = root
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(RUNTIME_DATABASE_FILE);
    std::fs::create_dir_all(path.parent().expect("路径应含父目录")).expect("测试目录应可创建");
    axon_session::SqliteRuntimeStore::from_root(root.join(axon_config::AXON_RUNTIME_DATA_DIR))
        .initialize()
        .expect("测试 schema 应可初始化");
    Connection::open(&path).expect("测试数据库应可打开")
}

fn insert_hot_session_history(connection: &Connection) {
    connection
        .execute(
            "INSERT INTO session_runtimes
             VALUES(?1,?1,?1,'/w','running',NULL,NULL,0,0,0,?2,?3,1,1)",
            rusqlite::params![
                HOT_SESSION_ID,
                HOT_SESSION_EVENT_COUNT,
                HOT_SESSION_RECENT_ACTIVITY_MS
            ],
        )
        .expect("高频 Session 应可写入");
    connection
        .execute(
            "WITH RECURSIVE seq(n) AS (
                 SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1
             )
             INSERT INTO session_events(
                 session_id, sequence, generation, event_type, payload_json, emitted_at_ms
             )
             SELECT ?2, n, 'g', 'wire_event', '{}', ?3 FROM seq",
            rusqlite::params![
                HOT_SESSION_EVENT_COUNT,
                HOT_SESSION_ID,
                OLD_EVENT_EMITTED_AT_MS
            ],
        )
        .expect("高频 Session 历史应可写入");
}

fn write_retained_session_artifacts(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let encoded_id = HOT_SESSION_ID
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let session_store = axon_session::session_store_path(root);
    let paths = [
        axon_session::JsonlThreadEventStore::path(root).join(format!("{encoded_id}.000000.jsonl")),
        session_store.join(TEST_ROLLOUT_DATABASE_FILE),
        session_store
            .join(HOT_SESSION_ID)
            .join(axon_agent_runtime::SESSION_NOTES_FILE_NAME),
        axon_jsonl::ShardedJsonlStore::physical_shard_path(
            session_store
                .join(HOT_SESSION_ID)
                .join(axon_agent_runtime::ARCHIVED_HISTORY_DIR_NAME)
                .join("window_1.jsonl"),
            0,
        ),
    ];
    paths
        .into_iter()
        .map(|path| {
            std::fs::create_dir_all(path.parent().expect("关联文件必须有父目录"))
                .expect("关联目录应可创建");
            std::fs::write(&path, RETAINED_ARTIFACT_CONTENT).expect("关联文件应可写入");
            (path, RETAINED_ARTIFACT_CONTENT.to_vec())
        })
        .collect()
}

#[test]
fn cleanup_covers_r1_to_r7_runtime_tables() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "INSERT INTO session_runtimes VALUES('s-old','s-old','s-old','/w','completed',NULL,NULL,0,0,0,0,1,1,1);
             INSERT INTO session_events VALUES('s-old',1,'g','turn','{}',1);
             INSERT INTO session_checkpoints VALUES('s-old',1,'a',0,0,NULL,NULL,NULL,NULL,NULL,1);
             INSERT INTO runtime_schedules VALUES('sch-1','s-old','interval','1m','{}','none','created',1,NULL,NULL,0,NULL,NULL);
             INSERT INTO jarvis_interactions(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,updated_at_ms,attachments_json)
                 VALUES('i-old','tui','m-1','{}','{}','{}','hi','understood',1,1,'[]');
             INSERT INTO jarvis_goals(goal_id,scope_json,parent_goal_id,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms)
                 VALUES('t-old','{}',NULL,'旧任务','completed',1,'{}','{}',NULL,NULL,NULL,0,NULL,NULL,NULL,NULL,1,1);
             INSERT INTO jarvis_executions(execution_id,goal_id,session_id,kind,payload_json,goal_revision,context_version,attempt,priority,dedupe_key,status,lease_json,claimed_by,claimed_generation,lease_expires_at_ms,started_at_ms,committed_at_ms,cancelled_at_ms,retry_at_ms,checkpoint_json,cancellation_requested_at_ms,cancellation_reason,created_at_ms,updated_at_ms)
                 VALUES('e-old','t-old',NULL,'turn','{}',NULL,NULL,1,0,NULL,'completed',NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL,NULL,NULL,1,1);
             INSERT INTO jarvis_results VALUES('r-old','t-old','e-old',1,'done','[]','[]',NULL,1);
             INSERT INTO jarvis_result_reads VALUES('r-old',1,'read');
             INSERT INTO jarvis_decisions VALUES('d-old','{}','user','allow',1,1,1);
             INSERT INTO jarvis_events VALUES('ev-old',1,'goal.completed','k-1',NULL,'{}',1);
             INSERT INTO daemon_hosts VALUES('h-old','g-old',1,NULL,'stopped',1,1,1);
             INSERT INTO daemon_upgrade_admissions VALUES('a-old','tok','v1','h-old','g-old',1,1,1,'claimed',NULL,NULL);
             INSERT INTO jarvis_client_presence VALUES('client-old','workspace-old','tui','dest-old',1,1);
             INSERT INTO jarvis_completion_notifications VALUES('n-old','workspace-old','t-old','e-old','完成提示','主 turn 汇总','[]','delivered',1,1,1);
             INSERT INTO jarvis_completion_deliveries VALUES('n-old','tui','dest-old','delivered',1,1);
             INSERT INTO jarvis_agent_messages(message_id,idempotency_key,owner_id,author,recipient,kind,content,trigger_turn,continuation_id,state,attempt_count,last_attempt_at_ms,retry_at_ms,last_error,delivered_at_ms,created_at_ms,expires_at_ms)
                 VALUES('m-old','k-m-old','s-old','/root','/root/review','collab','hi',0,NULL,'delivered',1,1,NULL,NULL,1,1,1);
             INSERT INTO jarvis_continuations(continuation_id,idempotency_key,owner_id,parent_session_id,child_session_id,agent_path,job_ref,execution_ref,terminal_kind,exit_reason,summary,artifact_path,token_input,token_output,token_reasoning,state,consumed_at_ms,consumed_by,attempt_count,last_attempt_at_ms,retry_at_ms,last_error,created_at_ms,expires_at_ms)
                 VALUES('c-old','k-c-old','s-old','s-old','child-old','/root/review',NULL,NULL,'completed','子代理正常结束','摘要',NULL,1,1,0,'consumed',1,'run',1,NULL,NULL,NULL,1,1);",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let mut counts = cleanup_runtime_session_families(root.path(), &["s-old".to_owned()])
        .expect("Session family 清理应成功");
    counts.merge(cleanup_runtime_data(root.path(), 100).expect("清理应成功"));

    assert_eq!(counts.sessions, 1);
    assert_eq!(counts.session_events, 1);
    assert_eq!(counts.session_checkpoints, 1);
    assert_eq!(counts.runtime_schedules, 1);
    assert_eq!(counts.interactions, 1);
    assert_eq!(counts.goals, 1);
    assert_eq!(counts.executions, 1);
    assert_eq!(counts.results, 1);
    assert_eq!(counts.result_reads, 1);
    assert_eq!(counts.decisions, 1);
    // 事件保底保留最新 1000 条：样本量低于保底时即使超期也不回收。
    assert_eq!(counts.events, 0);
    assert_eq!(counts.upgrade_admissions, 1);
    // 准入记录先删，随后旧 Host 不再被其引用而可回收。
    assert_eq!(counts.daemon_hosts, 1);
    assert_eq!(counts.client_presence, 1);
    assert_eq!(counts.completion_notifications, 1);
    assert_eq!(counts.completion_deliveries, 1);
    assert_eq!(counts.agent_messages, 1);
    assert_eq!(counts.continuations, 1);

    let remaining = count_expired_rows(
        &Connection::open(
            root.path()
                .join(axon_config::AXON_RUNTIME_DATA_DIR)
                .join(RUNTIME_DATABASE_FILE),
        )
        .expect("测试数据库应可重开"),
        100,
    )
    .expect("清理后应可统计");
    assert_eq!(remaining, RuntimeCleanupCounts::default());
}

#[test]
fn cleanup_protects_active_and_unread_records() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "INSERT INTO session_runtimes VALUES('s-run','s-run','s-run','/w','running',NULL,NULL,0,0,0,0,1,1,1);
             INSERT INTO jarvis_goals(goal_id,scope_json,parent_goal_id,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms)
                 VALUES('t-done','{}',NULL,'完成任务','completed',1,'{}','{}',NULL,NULL,NULL,0,NULL,NULL,NULL,NULL,1,1);
             INSERT INTO jarvis_executions(execution_id,goal_id,session_id,kind,payload_json,goal_revision,context_version,attempt,priority,dedupe_key,status,lease_json,claimed_by,claimed_generation,lease_expires_at_ms,started_at_ms,committed_at_ms,cancelled_at_ms,retry_at_ms,checkpoint_json,cancellation_requested_at_ms,cancellation_reason,created_at_ms,updated_at_ms)
                 VALUES('e-done','t-done',NULL,'turn','{}',NULL,NULL,1,0,NULL,'completed',NULL,NULL,NULL,NULL,1,NULL,NULL,NULL,NULL,NULL,NULL,1,1);
             INSERT INTO jarvis_results VALUES('r-unread','t-done','e-done',1,'done','[]','[]',NULL,1);
             INSERT INTO jarvis_interactions(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,updated_at_ms,attachments_json)
                 VALUES('i-live','tui','m-2','{}','{}','{}','hi','received',1,1,'[]');",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let dry_run = count_runtime_cleanup(root.path(), 100).expect("dry-run 应成功");
    assert_eq!(dry_run, RuntimeCleanupCounts::default());

    let counts = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(counts, RuntimeCleanupCounts::default());
}

#[test]
fn explicit_session_family_cleanup_rechecks_active_state_and_execution_references() {
    for (status, with_execution) in [("running", false), ("idle", true)] {
        let root = axon_test_support::test_temp_dir();
        let connection = open_fixture(root.path());
        connection
            .execute(
                "INSERT INTO session_runtimes VALUES('protected','protected','protected','/w',?1,NULL,NULL,0,0,0,0,1,1,1)",
                [status],
            )
            .expect("受保护 Session 应可写入");
        if with_execution {
            connection
                .execute(
                    "INSERT INTO jarvis_executions(
                        execution_id,goal_id,session_id,payload_json,attempt,priority,status,created_at_ms,updated_at_ms
                     ) VALUES('execution','task','protected','{}',1,0,'completed',1,1)",
                    [],
                )
                .expect("Execution 引用应可写入");
        }
        drop(connection);

        cleanup_runtime_session_families(root.path(), &["protected".to_owned()])
            .expect_err("事务内复核必须拒绝受保护 Session");

        let connection = Connection::open(
            root.path()
                .join(axon_config::AXON_RUNTIME_DATA_DIR)
                .join(RUNTIME_DATABASE_FILE),
        )
        .expect("runtime 应可重开");
        let retained: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM session_runtimes WHERE session_id='protected'",
                [],
                |row| row.get(0),
            )
            .expect("受保护 Session 应可查询");
        assert_eq!(retained, 1);
    }
}

#[test]
fn stale_family_dry_run_subtracts_rolling_event_overlap() {
    const EVENT_COUNT: i64 = SESSION_EVENT_MIN_RETAINED + 1;
    let root = axon_test_support::test_temp_dir();
    let mut connection = open_fixture(root.path());
    connection
        .execute(
            "INSERT INTO session_runtimes VALUES('stale','stale','stale','/w','idle',NULL,NULL,0,0,0,?1,1,1,1)",
            [EVENT_COUNT],
        )
        .expect("stale Session 应可写入");
    let transaction = connection.transaction().expect("事件事务应可创建");
    for sequence in 1..=EVENT_COUNT {
        transaction
            .execute(
                "INSERT INTO session_events VALUES('stale',?1,'g','turn','{}',1)",
                [sequence],
            )
            .expect("stale event 应可写入");
    }
    transaction.commit().expect("事件事务应可提交");

    let family = count_runtime_session_families(root.path(), &["stale".to_owned()])
        .expect("family 应可统计");
    let rolling = count_runtime_cleanup(root.path(), 100).expect("rolling 应可统计");
    let overlap = count_runtime_session_event_overlap(root.path(), &["stale".to_owned()], 100)
        .expect("重叠应可统计");

    assert_eq!(family.session_events, usize::try_from(EVENT_COUNT).unwrap());
    assert_eq!(rolling.session_events, 1);
    assert_eq!(overlap, 1);
    assert_eq!(
        family
            .session_events
            .saturating_add(rolling.session_events)
            .saturating_sub(overlap),
        usize::try_from(EVENT_COUNT).unwrap()
    );
}

#[test]
fn dry_run_counts_match_real_deletion() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "INSERT INTO jarvis_interactions(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,updated_at_ms,attachments_json)
                 VALUES('i-1','tui','m-1','{}','{}','{}','hi','rejected',1,1,'[]');
             INSERT INTO jarvis_decisions VALUES('d-1','{}','user','allow',1,1,1);",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let dry_run = count_runtime_cleanup(root.path(), 100).expect("dry-run 应成功");
    assert_eq!(dry_run.interactions, 1);
    assert_eq!(dry_run.decisions, 1);

    let real = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(real, dry_run);
}

#[test]
fn expired_events_beyond_floor_are_removed() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 1001)
             INSERT INTO jarvis_events(event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms)
             SELECT 'ev-'||n, n, 'goal.completed', 'k-'||n, NULL, '{}', 1 FROM seq;",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let counts = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(counts.events, 1);

    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(RUNTIME_DATABASE_FILE),
    )
    .expect("测试数据库应可重开");
    let remaining: i64 = connection
        .query_row("SELECT COUNT(*) FROM jarvis_events", [], |row| row.get(0))
        .expect("统计应成功");
    assert_eq!(remaining, 1_000);
    let lowest: i64 = connection
        .query_row("SELECT MIN(sequence) FROM jarvis_events", [], |row| {
            row.get(0)
        })
        .expect("统计应成功");
    assert_eq!(lowest, 2);
}

/// 单轮预算耗尽时必须停止并保留剩余数据，由下一轮清理幂等续跑。
#[test]
fn cleanup_stops_at_time_budget_and_resumes_next_run() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 1000)
             INSERT INTO jarvis_interactions
                 (interaction_id, source_channel, source_message_id, scope_json, audience_json,
                  reply_route_json, content, status, received_at_ms, lease_token,
                  lease_expires_at_ms, updated_at_ms)
             SELECT 'i-'||n, 'tui', 'm-'||n, '{}', '{}', '{}', 'hi', 'rejected', 1, NULL, NULL, 1
             FROM seq;",
        )
        .expect("测试数据应可写入");
    drop(connection);

    let first = run_runtime_cleanup_with_budget(root.path(), 100, false, Duration::ZERO)
        .expect("首轮清理应成功");
    assert_eq!(first.interactions, RUNTIME_CLEANUP_BATCH_LIMIT);

    let second = run_runtime_cleanup_with_budget(root.path(), 100, false, Duration::from_secs(30))
        .expect("续跑清理应成功");
    assert_eq!(second.interactions, 1_000 - RUNTIME_CLEANUP_BATCH_LIMIT);

    let remaining = count_runtime_cleanup(root.path(), 100).expect("dry-run 应成功");
    assert_eq!(remaining.interactions, 0);
}

/// 容量验收：大规模历史数据下清理必须按批次线性推进，不因缺索引而退化。
#[test]
#[ignore = "容量验收：构造大规模运行态数据验证清理耗时"]
fn cleanup_large_runtime_history_is_bounded() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "BEGIN;
             WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 50000)
             INSERT INTO jarvis_interactions
                 (interaction_id, source_channel, source_message_id, scope_json, audience_json,
                  reply_route_json, content, status, received_at_ms, lease_token,
                  lease_expires_at_ms, updated_at_ms)
             SELECT 'i-'||n, 'tui', 'm-'||n, '{}', '{}', '{}', 'hi', 'understood', 1, NULL, NULL, 1
             FROM seq;
             COMMIT;",
        )
        .expect("交互数据应可写入");
    connection
        .execute_batch(
            "BEGIN;
             WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 200000)
             INSERT INTO jarvis_events
                 (event_id, sequence, event_type, idempotency_key, cause_event_id, payload_json, occurred_at_ms)
             SELECT 'ev-'||n, n, 'goal.completed', 'k-'||n, NULL, '{}', 1 FROM seq;
             COMMIT;",
        )
        .expect("事件数据应可写入");
    drop(connection);

    let started = std::time::Instant::now();
    let counts = run_runtime_cleanup_with_budget(root.path(), 100, false, Duration::from_secs(600))
        .expect("容量清理应成功");
    let elapsed = started.elapsed();
    println!("容量清理：{counts:?} elapsed={elapsed:?}");

    assert_eq!(counts.interactions, 50_000);
    // 20 万事件仅回收超出保底的最新 1000 条之外的历史部分。
    assert_eq!(counts.events, 199_000);
    assert!(elapsed.as_secs() < 60, "大规模清理必须在有界时间内完成");
}

/// 并发验收：清理与 Daemon 写入并存时，写入延迟必须保持在毫秒级。
///
/// 断言口径是「清理不得让单次并发写入等待超过 2 秒」：批量 256 行 + 单批
/// IMMEDIATE 事务的写锁持有时间远低于该阈值，一旦退化为长事务即会失败。
#[test]
#[ignore = "并发验收：清理进行中持续写入，测量写入延迟"]
fn cleanup_does_not_stall_concurrent_writes() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };

    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "BEGIN;
             WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 20000)
             INSERT INTO jarvis_interactions
                 (interaction_id, source_channel, source_message_id, scope_json, audience_json,
                  reply_route_json, content, status, received_at_ms, lease_token,
                  lease_expires_at_ms, updated_at_ms)
             SELECT 'i-'||n, 'tui', 'm-'||n, '{}', '{}', '{}', 'hi', 'rejected', 1, NULL, NULL, 1
             FROM seq;
             COMMIT;",
        )
        .expect("交互数据应可写入");
    drop(connection);

    let writer_path = root
        .path()
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(RUNTIME_DATABASE_FILE);
    let started = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let max_write_us = Arc::new(AtomicU64::new(0));
    let samples = Arc::new(AtomicU64::new(0));

    let writer = {
        let started = Arc::clone(&started);
        let stop = Arc::clone(&stop);
        let max_write_us = Arc::clone(&max_write_us);
        let samples = Arc::clone(&samples);
        std::thread::spawn(move || {
            let connection = Connection::open(&writer_path).expect("写入连接应可打开");
            connection
                .busy_timeout(Duration::from_secs(5))
                .expect("写入连接应可配置");
            while !started.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
            let mut sequence = 1_000_000_i64;
            while !stop.load(Ordering::Relaxed) {
                let write_started = std::time::Instant::now();
                connection
                    .execute(
                        "INSERT INTO jarvis_events
                             (event_id, sequence, event_type, idempotency_key, cause_event_id,
                              payload_json, occurred_at_ms)
                         VALUES (?1, ?2, 'turn.started', ?3, NULL, '{}', ?4)",
                        rusqlite::params![
                            format!("w-{sequence}"),
                            sequence,
                            format!("wk-{sequence}"),
                            i64::MAX,
                        ],
                    )
                    .expect("并发写入应成功");
                let elapsed_us =
                    u64::try_from(write_started.elapsed().as_micros()).unwrap_or(u64::MAX);
                max_write_us.fetch_max(elapsed_us, Ordering::Relaxed);
                samples.fetch_add(1, Ordering::Relaxed);
                sequence += 1;
            }
        })
    };

    started.store(true, Ordering::Relaxed);
    let counts = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    stop.store(true, Ordering::Relaxed);
    writer.join().expect("写入线程应正常结束");

    let write_count = samples.load(Ordering::Relaxed);
    let max_write_ms = max_write_us.load(Ordering::Relaxed) / 1_000;
    println!(
        "并发写入 {write_count} 次，最大写延迟 {max_write_ms}ms，清理删除 {} 条 Interaction",
        counts.interactions
    );

    assert_eq!(counts.interactions, 20_000);
    assert!(write_count > 0, "并发写入必须发生");
    assert!(
        max_write_ms < 2_000,
        "清理不得让并发写入等待超过 2 秒，实测 {max_write_ms}ms"
    );
}

#[test]
fn running_session_event_history_is_trimmed_without_removing_related_data() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    insert_hot_session_history(&connection);
    drop(connection);
    let artifacts = write_retained_session_artifacts(root.path());
    let expected_deleted = HOT_SESSION_EVENT_COUNT - super::SESSION_EVENT_MIN_RETAINED;

    let dry_run = count_runtime_cleanup(root.path(), CLEANUP_CUTOFF_MS).expect("dry-run 应成功");
    assert_eq!(
        i64::try_from(dry_run.session_events).expect("计数应可转换"),
        expected_deleted
    );
    let counts = cleanup_runtime_data(root.path(), CLEANUP_CUTOFF_MS).expect("清理应成功");
    assert_eq!(
        i64::try_from(counts.session_events).expect("计数应可转换"),
        expected_deleted
    );

    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(RUNTIME_DATABASE_FILE),
    )
    .expect("测试数据库应可重开");
    let (remaining, first_sequence, last_sequence): (i64, i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), MIN(sequence), MAX(sequence)
             FROM session_events WHERE session_id = ?1",
            [HOT_SESSION_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("事件水位统计应成功");
    assert_eq!(remaining, super::SESSION_EVENT_MIN_RETAINED);
    assert_eq!(first_sequence, expected_deleted + 1);
    assert_eq!(last_sequence, HOT_SESSION_EVENT_COUNT);
    let remaining_sessions: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_runtimes WHERE session_id = ?1",
            [HOT_SESSION_ID],
            |row| row.get(0),
        )
        .expect("Session 统计应成功");
    assert_eq!(remaining_sessions, 1);
    for (path, before) in artifacts {
        assert_eq!(std::fs::read(path).expect("关联文件应可读取"), before);
    }
}

#[test]
#[ignore = "性能验收：20 轮隔离 20000 条 retained Session 事件清理"]
fn retained_session_event_cleanup_reports_percentiles() {
    let expected_deleted =
        usize::try_from(HOT_SESSION_EVENT_COUNT - super::SESSION_EVENT_MIN_RETAINED)
            .expect("删除计数应可转换");
    let mut samples_us = Vec::with_capacity(CLEANUP_PERFORMANCE_SAMPLE_COUNT);
    for _ in 0..CLEANUP_PERFORMANCE_SAMPLE_COUNT {
        let root = axon_test_support::test_temp_dir();
        let connection = open_fixture(root.path());
        insert_hot_session_history(&connection);
        drop(connection);

        let started = std::time::Instant::now();
        let counts = cleanup_runtime_data(root.path(), CLEANUP_CUTOFF_MS).expect("清理应成功");
        samples_us.push(started.elapsed().as_micros());
        assert_eq!(counts.session_events, expected_deleted);
    }
    samples_us.sort_unstable();
    let p50 = percentile(&samples_us, 50);
    let p95 = percentile(&samples_us, 95);
    let p99 = percentile(&samples_us, 99);
    println!(
        "retained_session_event_cleanup samples={} p50={}us p95={}us p99={}us",
        samples_us.len(),
        p50,
        p95,
        p99
    );
    assert!(
        p99 < CLEANUP_PERFORMANCE_P99_BUDGET_US,
        "滚动事件清理 p99 超出预算：{p99}us"
    );
}

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    let scaled = samples.len().saturating_mul(percentile);
    let index = scaled.div_ceil(100).saturating_sub(1);
    samples[index.min(samples.len().saturating_sub(1))]
}

/// 执行计划审计：七个清理族的批次选择必须全部走索引范围扫描。
///
/// 每批 256 行意味着「全表扫描 + 临时排序」会把单轮成本放大到 O(N²/批大小)，
/// 因此索引退化必须在单元测试阶段被拦住，而不是等真机出现写锁抖动。
#[test]
fn cleanup_selection_plans_use_indexes_without_full_scan_or_temp_sort() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    let selections = [
        (
            "upgrade_admissions",
            expired_upgrade_admissions_selection_sql(),
        ),
        ("session_events", expired_session_events_selection_sql()),
        ("terminal_goals", terminal_goals_selection_sql()),
        (
            "terminal_interactions",
            terminal_interactions_selection_sql(),
        ),
        ("expired_decisions", expired_decisions_selection_sql()),
        ("expired_events", expired_events_selection_sql()),
        ("expired_daemon_hosts", expired_daemon_hosts_selection_sql()),
        (
            "settled_completion_deliveries_delivered",
            settled_completion_deliveries_selection_sql_for_state(
                NotificationDeliveryState::Delivered,
            ),
        ),
        (
            "settled_completion_deliveries_expired",
            settled_completion_deliveries_selection_sql_for_state(
                NotificationDeliveryState::Expired,
            ),
        ),
        (
            "settled_completion_notifications_delivered",
            settled_completion_notifications_selection_sql_for_state(
                NotificationDeliveryState::Delivered,
            ),
        ),
        (
            "settled_completion_notifications_expired",
            settled_completion_notifications_selection_sql_for_state(
                NotificationDeliveryState::Expired,
            ),
        ),
    ];
    for (label, sql) in selections {
        let plan = explain_cleanup_plan(&connection, &sql);
        assert!(!plan.is_empty(), "{label} 清理的执行计划不应为空");
        assert!(!plan.contains("SCAN "), "{label} 清理不得全表扫描：{plan}");
        assert!(
            !plan.contains("TEMP B-TREE"),
            "{label} 清理不得临时排序：{plan}"
        );
    }
}

fn explain_cleanup_plan(connection: &Connection, sql: &str) -> String {
    let batch = i64::try_from(RUNTIME_CLEANUP_BATCH_LIMIT).expect("批次上限应可转换");
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("执行计划应可编译");
    let details = statement
        .query_map([1_i64, batch], |row| row.get::<_, String>(3))
        .expect("执行计划应可查询")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("执行计划应可读取");
    details.join("\n")
}
