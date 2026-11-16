use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use axon_session::RuntimeStore;
use rusqlite::{Connection, params};

use super::*;

const CUTOFF_MS: u64 = 500;
const STALE_ROOT: &str = "stale-root";
const STALE_CHILD: &str = "stale-root::child";
const RETAINED_SESSION: &str = "retained-session";
const RETAINED_BYTES: &[u8] = b"retained-session-bytes\n";
const RETAINED_UPDATED_AT_MS: i64 = 4_102_444_800_000;

#[path = "session_family_cleanup_source_tests.rs"]
mod source_installed;

#[test]
fn stale_family_is_removed_from_all_associated_stores() {
    let root = axon_test_support::test_temp_dir();
    let rollout_path = rollout_path(root.path());
    create_rollout(&rollout_path, true);
    create_runtime_family(root.path(), "idle", false);
    create_governance(root.path());
    create_thread(root.path(), STALE_ROOT, b"stale-thread\n");
    create_thread(root.path(), RETAINED_SESSION, RETAINED_BYTES);
    create_context(root.path(), STALE_ROOT);
    let retained_context = create_retained_context(root.path());

    let stale = stale_session_family_ids(root.path(), &rollout_path, CUTOFF_MS)
        .expect("stale family 应可读取");
    assert_eq!(stale, vec![STALE_ROOT.to_owned(), STALE_CHILD.to_owned()]);
    let dry_run_snapshots = snapshot_files(&[
        rollout_path.clone(),
        runtime_path(root.path()),
        governance_database_path(root.path()),
        thread_path(root.path(), STALE_ROOT),
        retained_context.clone(),
    ]);

    let mut dry_run = SessionCleanupReport::default();
    count_session_family_data(root.path(), &stale, CUTOFF_MS, &mut dry_run)
        .expect("dry-run 统计应成功");
    assert_eq!(dry_run.deleted_runtime_session_count, 2);
    // 事件已随会话目录删除，不再按表计数。
    assert_eq!(dry_run.deleted_session_event_count, 0);
    assert_eq!(dry_run.deleted_governance_count, 2);
    assert_eq!(dry_run.deleted_thread_file_count, 1);
    assert_eq!(dry_run.deleted_session_notes_count, 1);
    assert_eq!(dry_run.deleted_note_file_count, 2);
    assert_eq!(dry_run.deleted_archived_shard_count, 1);
    assert!(rollout_exists(&rollout_path, STALE_ROOT));
    assert_eq!(
        snapshot_files(
            &dry_run_snapshots
                .iter()
                .map(|item| item.0.clone())
                .collect::<Vec<_>>()
        ),
        dry_run_snapshots,
        "dry-run 不得改写任何被统计文件"
    );

    let mut report = SessionCleanupReport::default();
    cleanup_session_family_data(root.path(), &rollout_path, CUTOFF_MS, &stale, &mut report)
        .expect("整族清理应成功");

    assert_eq!(report.deleted_session_count, 2);
    assert_eq!(
        report.deleted_runtime_session_count,
        dry_run.deleted_runtime_session_count
    );
    assert_eq!(
        report.deleted_session_event_count,
        dry_run.deleted_session_event_count
    );
    assert_eq!(
        report.deleted_governance_count,
        dry_run.deleted_governance_count
    );
    assert_eq!(
        report.deleted_thread_file_count,
        dry_run.deleted_thread_file_count
    );
    assert!(!rollout_exists(&rollout_path, STALE_ROOT));
    assert!(!runtime_exists(root.path(), STALE_ROOT));
    assert!(!thread_path(root.path(), STALE_ROOT).exists());
    assert!(!root.path().join("sessions").join(STALE_ROOT).exists());
    assert_eq!(
        std::fs::read(thread_path(root.path(), RETAINED_SESSION)).expect("retained JSONL 应可读"),
        RETAINED_BYTES
    );
    assert_eq!(
        std::fs::read(retained_context).expect("retained context 应可读"),
        RETAINED_BYTES
    );
    assert!(rollout_exists(&rollout_path, RETAINED_SESSION));
}

#[test]
fn active_or_execution_referenced_member_protects_whole_rollout_family() {
    for (status, with_execution) in [("running", false), ("idle", true)] {
        let root = axon_test_support::test_temp_dir();
        let rollout_path = rollout_path(root.path());
        create_rollout(&rollout_path, false);
        create_runtime_family(root.path(), status, with_execution);

        let stale = stale_session_family_ids(root.path(), &rollout_path, CUTOFF_MS)
            .expect("保护集合应可读取");

        assert!(stale.is_empty());
        assert!(rollout_exists(&rollout_path, STALE_ROOT));
        assert!(rollout_exists(&rollout_path, STALE_CHILD));
    }
}

#[test]
fn context_failure_keeps_rollout_retry_anchor() {
    let root = axon_test_support::test_temp_dir();
    let rollout_path = rollout_path(root.path());
    create_rollout(&rollout_path, false);
    let invalid_context = root.path().join("sessions").join(STALE_ROOT);
    std::fs::create_dir_all(invalid_context.parent().expect("context 必须有父目录"))
        .expect("context 父目录应可创建");
    std::fs::write(&invalid_context, b"not-a-directory").expect("异常 context 应可创建");
    let stale = vec![STALE_ROOT.to_owned(), STALE_CHILD.to_owned()];

    let error = cleanup_session_family_data(
        root.path(),
        &rollout_path,
        CUTOFF_MS,
        &stale,
        &mut SessionCleanupReport::default(),
    )
    .expect_err("context 删除失败必须向上传递");

    assert!(error.contains("删除会话上下文目录"));
    assert!(rollout_exists(&rollout_path, STALE_ROOT));
    assert!(rollout_exists(&rollout_path, STALE_CHILD));
}

#[test]
fn governance_or_runtime_read_failure_keeps_rollout_retry_anchor() {
    for broken_store in ["governance", "runtime"] {
        let root = axon_test_support::test_temp_dir();
        let rollout_path = rollout_path(root.path());
        create_rollout(&rollout_path, false);
        let broken_path = if broken_store == "governance" {
            governance_database_path(root.path())
        } else {
            runtime_path(root.path())
        };
        std::fs::create_dir_all(broken_path.parent().expect("损坏存储必须有父目录"))
            .expect("损坏存储目录应可创建");
        std::fs::write(&broken_path, b"not-sqlite").expect("损坏存储应可创建");

        stale_session_family_ids(root.path(), &rollout_path, CUTOFF_MS)
            .expect_err("关联 SQLite 读取失败必须终止候选生成");

        assert!(rollout_exists(&rollout_path, STALE_ROOT));
        assert!(rollout_exists(&rollout_path, STALE_CHILD));
    }
}

#[test]
fn rollout_cleanup_crosses_bounded_batch_boundary() {
    let root = axon_test_support::test_temp_dir();
    let rollout_path = rollout_path(root.path());
    let store = RolloutSqliteStore::open(&rollout_path).expect("rollout 应可初始化");
    drop(store);
    let mut connection = Connection::open(&rollout_path).expect("rollout 应可打开");
    let transaction = connection.transaction().expect("rollout 事务应可创建");
    for index in 0..=SESSION_FAMILY_BATCH_LIMIT {
        insert_rollout(&transaction, &format!("stale-{index:04}"), None, 1);
    }
    transaction.commit().expect("rollout fixture 应可提交");
    let stale =
        stale_session_family_ids(root.path(), &rollout_path, CUTOFF_MS).expect("批量候选应可读取");
    assert_eq!(stale.len(), SESSION_FAMILY_BATCH_LIMIT + 1);

    let mut report = SessionCleanupReport::default();
    cleanup_session_family_data(root.path(), &rollout_path, CUTOFF_MS, &stale, &mut report)
        .expect("跨批次清理应成功");

    assert_eq!(report.deleted_session_count, SESSION_FAMILY_BATCH_LIMIT + 1);
    assert_eq!(rollout_count(&rollout_path), 0);
}

#[test]
fn associated_cleanup_queries_use_indexes() {
    let root = axon_test_support::test_temp_dir();
    create_runtime_family(root.path(), "idle", false);
    create_governance(root.path());
    let ids = [STALE_ROOT, STALE_CHILD];
    let placeholders = "?,?";

    let runtime = Connection::open(runtime_path(root.path())).expect("runtime 应可打开");
    assert_indexed_plan(
        &runtime,
        &format!(
            "SELECT session_id FROM sessions
             WHERE session_id IN ({placeholders}) AND status IN ('running','compacting')"
        ),
        &ids,
        "runtime active Session",
    );

    let governance =
        Connection::open(governance_database_path(root.path())).expect("governance 应可打开");
    let governance_plan = explain_plan(
        &governance,
        "SELECT COUNT(*) FROM governance_jobs
         WHERE status IN ('completed','failed') AND updated_at_ms < ?
           AND session_id IN (?,?)",
        &["500", STALE_ROOT, STALE_CHILD],
    );
    assert!(
        governance_plan.contains("idx_governance_retention"),
        "governance 清理必须命中 retention 索引：{governance_plan}"
    );
}

#[test]
#[ignore = "性能验收：20 轮隔离 stale Session family 清理"]
fn stale_family_cleanup_reports_percentiles() {
    const SAMPLE_COUNT: usize = 20;
    const FAMILY_COUNT: usize = 1_000;
    const P99_BUDGET_MS: u128 = 2_000;

    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for sample in 0..SAMPLE_COUNT {
        let root = axon_test_support::test_temp_dir();
        let rollout_path = rollout_path(root.path());
        let store = RolloutSqliteStore::open(&rollout_path).expect("rollout 应可初始化");
        drop(store);
        let mut connection = Connection::open(&rollout_path).expect("rollout 应可打开");
        let transaction = connection.transaction().expect("rollout 事务应可创建");
        for index in 0..FAMILY_COUNT {
            insert_rollout(
                &transaction,
                &format!("sample-{sample:02}-stale-{index:04}"),
                None,
                1,
            );
        }
        transaction.commit().expect("rollout fixture 应可提交");
        let stale =
            stale_session_family_ids(root.path(), &rollout_path, CUTOFF_MS).expect("候选应可读取");
        let started = Instant::now();
        cleanup_session_family_data(
            root.path(),
            &rollout_path,
            CUTOFF_MS,
            &stale,
            &mut SessionCleanupReport::default(),
        )
        .expect("性能样本清理应成功");
        samples.push(started.elapsed().as_millis());
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    let p99 = percentile(&samples, 99);
    eprintln!("stale family cleanup: p50={p50}ms p95={p95}ms p99={p99}ms n={SAMPLE_COUNT}");
    assert!(p99 < P99_BUDGET_MS, "p99={p99}ms 超出 {P99_BUDGET_MS}ms");
}

fn create_rollout(path: &Path, with_retained: bool) {
    let store = RolloutSqliteStore::open(path).expect("rollout 应可初始化");
    drop(store);
    let connection = Connection::open(path).expect("rollout 应可打开");
    insert_rollout(&connection, STALE_ROOT, None, 100);
    insert_rollout(&connection, STALE_CHILD, Some(STALE_ROOT), 200);
    if with_retained {
        insert_rollout(&connection, RETAINED_SESSION, None, RETAINED_UPDATED_AT_MS);
    }
}

fn insert_rollout(
    connection: &Connection,
    session_id: &str,
    parent_session_id: Option<&str>,
    updated_at_ms: i64,
) {
    connection
        .execute(
            "INSERT INTO rollout_sessions(
                session_id,parent_session_id,created_at_ms,updated_at_ms,provider,model,title
             ) VALUES(?1,?2,?3,?3,'gateway','test','fixture')",
            params![session_id, parent_session_id, updated_at_ms],
        )
        .expect("rollout fixture 应可写入");
}

fn create_runtime_family(root: &Path, child_status: &str, with_execution: bool) {
    let data = root.join(axon_session::AXON_RUNTIME_DATA_DIR);
    axon_session::SqliteRuntimeStore::from_root(&data)
        .initialize()
        .expect("runtime schema 应可初始化");
    let connection = Connection::open(data.join("control.sqlite3")).expect("runtime 应可打开");
    for (id, status) in [(STALE_ROOT, "idle"), (STALE_CHILD, child_status)] {
        connection
            .execute(
                "INSERT INTO sessions(session_id,run_session_id,root_session_id,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?1,?1,'/workspace',?2,NULL,NULL,0,0,1,1,1,1,1)",
                params![id, status],
            )
            .expect("runtime Session 应可写入");
    }
    if with_execution {
        connection
            .execute(
                "INSERT INTO work_items(
                    execution_id,goal_id,session_id,payload_json,attempt,priority,status,created_at_ms,updated_at_ms
                 ) VALUES('execution','task',?1,'{}',1,0,'completed',1,1)",
                [STALE_CHILD],
            )
            .expect("Execution 引用应可写入");
    }
}

fn create_governance(root: &Path) {
    let path = governance_database_path(root);
    std::fs::create_dir_all(path.parent().expect("governance 必须有父目录"))
        .expect("governance 目录应可创建");
    let connection = Connection::open(path).expect("governance 应可打开");
    connection
        .execute_batch(
            "CREATE TABLE governance_jobs(
                job_id TEXT PRIMARY KEY,session_id TEXT NOT NULL,payload_json TEXT NOT NULL,
                status TEXT NOT NULL,attempts INTEGER NOT NULL,available_at_ms INTEGER NOT NULL,
                lease_expires_at_ms INTEGER,created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,last_error TEXT
             );
             CREATE INDEX idx_governance_claim
                ON governance_jobs(status,available_at_ms,lease_expires_at_ms,created_at_ms);
             CREATE INDEX idx_governance_retention
                ON governance_jobs(status,updated_at_ms,session_id);
             INSERT INTO governance_jobs VALUES('job-root','stale-root','{}','completed',1,1,NULL,1,1,NULL);
             INSERT INTO governance_jobs VALUES('job-child','stale-root::child','{}','failed',1,1,NULL,1,1,NULL);",
        )
        .expect("governance fixture 应可写入");
}

fn create_thread(root: &Path, session_id: &str, content: &[u8]) {
    let path = thread_path(root, session_id);
    std::fs::create_dir_all(path.parent().expect("thread 必须有父目录"))
        .expect("thread 目录应可创建");
    std::fs::write(path, content).expect("thread fixture 应可写入");
}

fn create_context(root: &Path, session_id: &str) {
    let session = root.join("sessions").join(session_id);
    let notes = session.join(axon_agent_runtime::NOTES_DIR_NAME);
    let archive = session.join(axon_agent_runtime::ARCHIVED_HISTORY_DIR_NAME);
    std::fs::create_dir_all(notes.join("archive")).expect("notes 目录应可创建");
    std::fs::create_dir_all(&archive).expect("archive 目录应可创建");
    std::fs::write(
        session.join(axon_agent_runtime::SESSION_NOTES_FILE_NAME),
        b"{}",
    )
    .expect("notes 状态应可写入");
    std::fs::write(notes.join("checkpoint.md"), b"checkpoint").expect("notes 应可写入");
    std::fs::write(
        notes.join("archive").join("old-notes.md"),
        b"archived notes",
    )
    .expect("归档便签应可写入");
    std::fs::write(
        axon_jsonl::ShardedJsonlStore::physical_shard_path(archive.join("window.jsonl"), 0),
        b"{}\n",
    )
    .expect("archive 应可写入");
}

fn create_retained_context(root: &Path) -> PathBuf {
    let path = root
        .join("sessions")
        .join(RETAINED_SESSION)
        .join("retained.txt");
    std::fs::create_dir_all(path.parent().expect("retained context 必须有父目录"))
        .expect("retained context 目录应可创建");
    std::fs::write(&path, RETAINED_BYTES).expect("retained context 应可写入");
    path
}

fn rollout_path(root: &Path) -> PathBuf {
    axon_session::session_store_path(root).join("rollout.sqlite3")
}

fn thread_path(root: &Path, session_id: &str) -> PathBuf {
    let encoded = session_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    axon_session::JsonlThreadEventStore::path(root).join(format!("{encoded}.000000.jsonl"))
}

fn rollout_exists(path: &Path, session_id: &str) -> bool {
    Connection::open(path)
        .expect("rollout 应可打开")
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rollout_sessions WHERE session_id=?1)",
            [session_id],
            |row| row.get(0),
        )
        .expect("rollout 应可查询")
}

fn rollout_count(path: &Path) -> usize {
    let count: i64 = Connection::open(path)
        .expect("rollout 应可打开")
        .query_row("SELECT COUNT(*) FROM rollout_sessions", [], |row| {
            row.get(0)
        })
        .expect("rollout 应可统计");
    usize::try_from(count).expect("rollout 数量应可转换")
}

fn runtime_exists(root: &Path, session_id: &str) -> bool {
    Connection::open(runtime_path(root))
        .expect("runtime 应可打开")
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
            [session_id],
            |row| row.get(0),
        )
        .expect("runtime 应可查询")
}

fn runtime_path(root: &Path) -> PathBuf {
    root.join(axon_session::AXON_RUNTIME_DATA_DIR)
        .join("control.sqlite3")
}

fn snapshot_files(paths: &[PathBuf]) -> Vec<(PathBuf, Vec<u8>)> {
    paths
        .iter()
        .map(|path| {
            (
                path.clone(),
                std::fs::read(path).unwrap_or_else(|error| {
                    panic!("快照文件 {} 应可读取：{error}", path.display())
                }),
            )
        })
        .collect()
}

fn assert_indexed_plan(connection: &Connection, sql: &str, values: &[&str], label: &str) {
    let plan = explain_plan(connection, sql, values);
    assert!(!plan.contains("SCAN "), "{label} 不得全表扫描：{plan}");
    assert!(plan.contains("SEARCH "), "{label} 必须使用索引：{plan}");
}

fn explain_plan(connection: &Connection, sql: &str, values: &[&str]) -> String {
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("执行计划应可编译");
    statement
        .query_map(rusqlite::params_from_iter(values.iter()), |row| {
            row.get::<_, String>(3)
        })
        .expect("执行计划应可查询")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("执行计划应可读取")
        .join("\n")
}

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    let index = samples
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1);
    samples[index.min(samples.len().saturating_sub(1))]
}
