use axon_session::{NotificationDeliveryState, RuntimeStore};

use crate::cron_commands::runtime_cleanup_families::{
    expired_daemon_instances_selection_sql, expired_decisions_selection_sql,
    expired_events_selection_sql, expired_upgrade_admissions_selection_sql,
    retention::{
        expired_agent_messages_selection_sql, expired_continuations_selection_sql,
        expired_user_questions_selection_sql,
    },
    settled_completion_deliveries_selection_sql_for_state,
    settled_completion_notifications_selection_sql_for_state,
    terminal_dispatch_interactions_selection_sql, terminal_goals_selection_sql,
    terminal_interactions_selection_sql,
};

use super::*;

pub(super) fn open_fixture(root: &Path) -> Connection {
    let directory = root.join(axon_config::AXON_RUNTIME_DATA_DIR);
    let path = directory.join(CONTROL_DATABASE_FILE);
    std::fs::create_dir_all(path.parent().expect("路径应含父目录")).expect("测试目录应可创建");
    axon_session::SqliteRuntimeStore::from_root(&directory)
        .initialize()
        .expect("测试 schema 应可初始化");
    let connection = Connection::open(&path).expect("测试数据库应可打开");
    // 夹具按「一份批次写两个库」的方式播种，因此队列库也要具备控制面表的形状：
    // 只镜像表结构，不复制索引与数据，空表对断言无影响。
    let outbox = Connection::open(outbox_sibling(&connection)).expect("队列库应可打开");
    mirror_schema(&connection, &outbox);
    connection
}

fn mirror_schema(control: &Connection, outbox: &Connection) {
    let mut statement = control
        .prepare(
            "SELECT name, sql FROM sqlite_master
             WHERE type='table' AND sql IS NOT NULL AND name NOT LIKE 'sqlite_%'",
        )
        .expect("读取控制面表定义");
    let tables = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("枚举控制面表定义")
        .collect::<Result<Vec<_>, _>>()
        .expect("收集控制面表定义");
    for (name, sql) in tables {
        let exists: bool = outbox
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [&name],
                |row| row.get(0),
            )
            .expect("查询队列库表");
        if !exists {
            outbox.execute_batch(&sql).expect("镜像控制面表结构");
        }
    }
}

/// 队列库与给定连接指向的控制面库同目录，文件名固定为 `outbox.sqlite3`。
fn outbox_sibling(connection: &Connection) -> std::path::PathBuf {
    let path: String = connection
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |row| row.get(0),
        )
        .expect("读取库文件路径");
    std::path::Path::new(&path).with_file_name(OUTBOX_DATABASE_FILE)
}

/// 清理横跨两个库，测试夹具也按同一个批次写两份：每个库各自持有本库内存在的表，
/// 这样既保留「一份数据、两侧可读」的既有断言，又不需要为每张表判断归属。
fn seed(connection: &Connection, batch: &str, what: &str) {
    connection.execute_batch(batch).expect(what);
    let outbox = Connection::open(outbox_sibling(connection)).expect("队列库应可打开");
    outbox.execute_batch(batch).expect(what);
}

#[test]
fn cleanup_protects_active_and_unread_records() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    seed(&connection, "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('s-run','s-run',NULL,'private','primary','/w','running',NULL,NULL,0,0,0,0,1,1,1);
             INSERT INTO goals(goal_id,scope_json,parent_goal_id,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms)
                 VALUES('t-done','{}',NULL,'完成任务','completed',1,'{}','{}',NULL,NULL,NULL,0,NULL,NULL,NULL,NULL,1,1);
             INSERT INTO work_item_history(work_id,goal_id,session_id,kind,payload_json,attempt,priority,status,terminal_kind,started_at_ms,committed_at_ms,created_at_ms,updated_at_ms,finished_at_ms)
                 VALUES('e-done','t-done',NULL,'turn','{}',1,0,'completed','completed',1,NULL,1,1,1);
             INSERT INTO goal_results VALUES('r-unread','t-done','e-done',1,'done','[]','[]',NULL,1);
             INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,received_at_ms,updated_at_ms,attachments_json,session_id)
                 VALUES('i-live','tui','m-2','{}','{}','{}','hi',1,1,'[]','s-run');", "测试数据应可写入");
    drop(connection);

    let dry_run = count_runtime_cleanup(root.path(), 100).expect("dry-run 应成功");
    assert_eq!(dry_run, RuntimeCleanupCounts::default());

    let counts = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(counts, RuntimeCleanupCounts::default());
}

#[test]
fn explicit_session_family_cleanup_rechecks_active_state_and_execution_references() {
    for (status, work_item_status) in [
        ("running", None),
        ("idle", Some("running")),
        ("idle", Some("queued")),
        ("idle", Some("waiting")),
    ] {
        let root = axon_test_support::test_temp_dir();
        let connection = open_fixture(root.path());
        connection
            .execute(
                "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('protected','protected',NULL,'private','primary','/w',?1,NULL,NULL,0,0,0,0,1,1,1)",
                [status],
            )
            .expect("受保护 Session 应可写入");
        if let Some(work_item_status) = work_item_status {
            connection
                .execute(
                    "INSERT INTO work_items(
                        execution_id,goal_id,session_id,payload_json,attempt,priority,status,created_at_ms,updated_at_ms
                     ) VALUES('execution','task','protected','{}',1,0,?1,1,1)",
                    [work_item_status],
                )
                .expect("Execution 引用应可写入");
        }
        drop(connection);

        cleanup_runtime_session_families(root.path(), &["protected".to_owned()])
            .expect_err("事务内复核必须拒绝受保护 Session");

        let connection = Connection::open(
            root.path()
                .join(axon_config::AXON_RUNTIME_DATA_DIR)
                .join(CONTROL_DATABASE_FILE),
        )
        .expect("runtime 应可重开");
        let retained: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE session_id='protected'",
                [],
                |row| row.get(0),
            )
            .expect("受保护 Session 应可查询");
        assert_eq!(retained, 1);
    }
}

/// 终态工作项不得把会话钉在运行态库里：它已不再推进任何执行，
/// 却会让「活跃判定」永远命中，使会话族一套都删不掉。
#[test]
fn explicit_session_family_cleanup_reclaims_terminal_work_items() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    connection
        .execute_batch(
            "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('stale','stale',NULL,'private','primary','/w','idle',NULL,NULL,0,0,0,0,1,1,1);
             INSERT INTO work_items(
                 execution_id,goal_id,session_id,payload_json,attempt,priority,status,created_at_ms,updated_at_ms
             ) VALUES('done','task','stale','{}',1,0,'completed',1,1);
             INSERT INTO work_items(
                 execution_id,goal_id,session_id,payload_json,attempt,priority,status,created_at_ms,updated_at_ms
             ) VALUES('cancelled','task','stale','{}',1,0,'cancelled',1,1);
             INSERT INTO work_item_history(work_id,goal_id,session_id,kind,payload_json,attempt,priority,status,terminal_kind,created_at_ms,updated_at_ms,finished_at_ms)
                 VALUES('archived','task','stale','turn','{}',1,0,'completed','completed',1,1,1);
             INSERT INTO session_metrics(session_id,turn_count,updated_at_ms)
                 VALUES('stale',1,1);
             INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,received_at_ms,updated_at_ms,attachments_json,session_id)
                 VALUES('i-stale','tui','m-stale','{}','{}','{}','hi',1,1,'[]','stale');
             INSERT INTO resume_pointer(singleton_key,session_id,updated_at_ms)
                 VALUES('default','stale',1);",
        )
        .expect("过期会话数据应可写入");
    drop(connection);

    let counts = cleanup_runtime_session_families(root.path(), &["stale".to_owned()])
        .expect("终态工作项不应阻止会话族清理");
    assert_eq!(counts.sessions, 1);
    assert_eq!(counts.executions, 3);

    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(CONTROL_DATABASE_FILE),
    )
    .expect("runtime 应可重开");
    for (table, column) in [
        ("sessions", "session_id"),
        ("work_items", "session_id"),
        ("work_item_history", "session_id"),
        ("session_metrics", "session_id"),
        ("inbox", "session_id"),
        ("resume_pointer", "session_id"),
    ] {
        let retained: i64 = connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column}='stale'"),
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| panic!("{table} 应可查询"));
        assert_eq!(retained, 0, "{table} 必须随会话族一并回收");
    }
}

#[test]
fn dry_run_counts_match_real_deletion() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    seed(&connection, "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('s-cleanup','s-cleanup',NULL,'private','primary','/w','idle',0,0,0,0,1,1,1);
             INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,received_at_ms,updated_at_ms,attachments_json,session_id)
                 VALUES('i-1','tui','m-1','{}','{}','{}','hi',1,1,'[]','s-cleanup');
             INSERT INTO work_item_history(work_id,kind,attempt,priority,status,inbox_id,terminal_kind,created_at_ms,updated_at_ms,finished_at_ms)
                 VALUES('w-i-1','turn',1,0,'cancelled','i-1','rejected',1,1,1);
             INSERT INTO decisions VALUES('d-1','{}','user','allow',1,1,1);", "测试数据应可写入");
    drop(connection);

    let dry_run = count_runtime_cleanup(root.path(), 100).expect("dry-run 应成功");
    assert_eq!(dry_run.interactions, 1);
    assert_eq!(dry_run.decisions, 1);

    let real = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(real, dry_run);
}

#[test]
fn dispatch_turn_inbox_rows_are_reclaimed_without_work_items() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    // 派发轮（source_channel = 'agent'）的终态直接落在入站行上，没有工作项历史行：
    // 只按 work_item_history 选批会让这行永远删不掉，inbox 因此单调增长。
    seed(&connection, "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('s-dispatch','s-dispatch',NULL,'private','primary','/w','idle',0,0,0,0,1,1,1);
             INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,received_at_ms,updated_at_ms,attachments_json,session_id,terminal_kind)
                 VALUES('i-dispatch','agent','m-dispatch','{}','{}','{}','hi',1,1,'[]','s-dispatch','understood');", "派发轮入站事实应可写入");
    drop(connection);

    let dry_run = count_runtime_cleanup(root.path(), 100).expect("dry-run 应成功");
    assert_eq!(
        dry_run.interactions, 1,
        "dry-run 必须与删除路径用同一份口径，否则容量评估会低估"
    );

    let real = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(real.interactions, 1);

    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(CONTROL_DATABASE_FILE),
    )
    .expect("runtime 应可重开");
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM inbox WHERE interaction_id='i-dispatch'",
            [],
            |row| row.get(0),
        )
        .expect("统计应成功");
    assert_eq!(remaining, 0, "派发轮的入站行必须被回收");
}

#[test]
fn expired_events_beyond_floor_are_removed() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    seed(&connection, "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 1001)
             INSERT INTO ledger_events(event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms)
             SELECT 'ev-'||n, n, 'goal.completed', 'k-'||n, NULL, '{}', 1 FROM seq;", "测试数据应可写入");
    drop(connection);

    let counts = cleanup_runtime_data(root.path(), 100).expect("清理应成功");
    assert_eq!(counts.events, 1);

    let connection = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(CONTROL_DATABASE_FILE),
    )
    .expect("测试数据库应可重开");
    let remaining: i64 = connection
        .query_row("SELECT COUNT(*) FROM ledger_events", [], |row| row.get(0))
        .expect("统计应成功");
    assert_eq!(remaining, 1_000);
    let lowest: i64 = connection
        .query_row("SELECT MIN(sequence) FROM ledger_events", [], |row| {
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
    seed(&connection, "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('s-cleanup','s-cleanup',NULL,'private','primary','/w','idle',0,0,0,0,1,1,1);
             WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 1000)
             INSERT INTO inbox
                 (interaction_id, source_channel, source_message_id, scope_json, audience_json,
                  reply_route_json, content, received_at_ms, updated_at_ms, session_id)
             SELECT 'i-'||n, 'tui', 'm-'||n, '{}', '{}', '{}', 'hi', 1, 1, 's-cleanup'
             FROM seq;
             INSERT INTO work_item_history
                 (work_id, kind, attempt, priority, status, inbox_id, terminal_kind, created_at_ms, updated_at_ms, finished_at_ms)
             SELECT 'w-'||interaction_id, 'turn', 1, 0, 'cancelled', interaction_id, 'rejected', 1, 1, 1 FROM inbox;", "测试数据应可写入");
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
    seed(&connection, "BEGIN;
             INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('s-cleanup','s-cleanup',NULL,'private','primary','/w','idle',0,0,0,0,1,1,1);
             WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 50000)
                          INSERT INTO inbox
                 (interaction_id, source_channel, source_message_id, scope_json, audience_json,
                  reply_route_json, content, received_at_ms, updated_at_ms, session_id)
             SELECT 'i-'||n, 'tui', 'm-'||n, '{}', '{}', '{}', 'hi', 1, 1, 's-cleanup'
             FROM seq;
             INSERT INTO work_item_history
                 (work_id, kind, attempt, priority, status, inbox_id, terminal_kind, created_at_ms, updated_at_ms, finished_at_ms)
             SELECT 'w-'||interaction_id, 'turn', 1, 0, 'completed', interaction_id, 'understood', 1, 1 FROM inbox;
             COMMIT;", "交互数据应可写入");
    seed(&connection, "BEGIN;
             WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 200000)
             INSERT INTO ledger_events
                 (event_id, sequence, event_type, idempotency_key, cause_event_id, payload_json, occurred_at_ms)
             SELECT 'ev-'||n, n, 'goal.completed', 'k-'||n, NULL, '{}', 1 FROM seq;
             COMMIT;", "事件数据应可写入");
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
    seed(&connection, "BEGIN;
             INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES('s-cleanup','s-cleanup',NULL,'private','primary','/w','idle',0,0,0,0,1,1,1);
             WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < 20000)
                          INSERT INTO inbox
                 (interaction_id, source_channel, source_message_id, scope_json, audience_json,
                  reply_route_json, content, received_at_ms, updated_at_ms, session_id)
             SELECT 'i-'||n, 'tui', 'm-'||n, '{}', '{}', '{}', 'hi', 1, 1, 's-cleanup'
             FROM seq;
             INSERT INTO work_item_history
                 (work_id, kind, attempt, priority, status, inbox_id, terminal_kind, created_at_ms, updated_at_ms, finished_at_ms)
             SELECT 'w-'||interaction_id, 'turn', 1, 0, 'cancelled', interaction_id, 'rejected', 1, 1, 1 FROM inbox;
             COMMIT;", "交互数据应可写入");
    drop(connection);

    let writer_path = root
        .path()
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(CONTROL_DATABASE_FILE);
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
                        "INSERT INTO ledger_events
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

/// 执行计划审计：七个清理族的批次选择必须全部走索引范围扫描。
/// 每批 256 行意味着「全表扫描 + 临时排序」会把单轮成本放大到 O(N²/批大小)，
/// 因此索引退化必须在单元测试阶段被拦住，而不是等真机出现写锁抖动。
#[test]
fn cleanup_selection_plans_use_indexes_without_full_scan_or_temp_sort() {
    let root = axon_test_support::test_temp_dir();
    let connection = open_fixture(root.path());
    // 队列库的清理语句要在队列库上取执行计划：两个库各自持有自己的表。
    let outbox = Connection::open(
        root.path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join(OUTBOX_DATABASE_FILE),
    )
    .expect("队列库应可打开");
    let selections = [
        (
            "upgrade_admissions",
            expired_upgrade_admissions_selection_sql(),
        ),
        ("terminal_goals", terminal_goals_selection_sql()),
        (
            "terminal_interactions",
            terminal_interactions_selection_sql(),
        ),
        (
            "terminal_dispatch_interactions",
            terminal_dispatch_interactions_selection_sql(),
        ),
        ("expired_decisions", expired_decisions_selection_sql()),
        ("expired_events", expired_events_selection_sql()),
        (
            "expired_daemon_instances",
            expired_daemon_instances_selection_sql(),
        ),
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
        (
            "expired_agent_messages",
            expired_agent_messages_selection_sql(),
        ),
        (
            "expired_continuations",
            expired_continuations_selection_sql(),
        ),
        (
            "expired_user_questions",
            expired_user_questions_selection_sql(),
        ),
    ];
    for (label, sql) in selections {
        // 投递记录、通知本体与三张保留期表都在队列库，其余在控制面库。
        let target = if label.starts_with("settled_completion")
            || label == "expired_agent_messages"
            || label == "expired_continuations"
            || label == "expired_user_questions"
        {
            &outbox
        } else {
            &connection
        };
        let plan = explain_cleanup_plan(target, &sql);
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
