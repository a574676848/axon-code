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
const RETAINED_UPDATED_AT_MS: u64 = 4_102_444_800_000;

#[path = "session_family_cleanup_source_tests.rs"]
mod source_installed;

#[test]
fn stale_family_is_removed_from_all_associated_stores() {
    let root = axon_test_support::test_temp_dir();
    create_thread(root.path(), STALE_ROOT, None, 100);
    create_thread(root.path(), STALE_CHILD, Some(STALE_ROOT), 200);
    create_thread(root.path(), RETAINED_SESSION, None, RETAINED_UPDATED_AT_MS);
    create_runtime_family(root.path(), "idle", false);
    create_governance(root.path());
    create_context(root.path(), STALE_ROOT);
    let retained_context = create_retained_context(root.path());

    let stale = stale_session_family_ids(root.path(), CUTOFF_MS).expect("stale family 应可读取");
    assert_eq!(stale, vec![STALE_ROOT.to_owned(), STALE_CHILD.to_owned()]);
    let dry_run_snapshots = snapshot_files(&[
        runtime_path(root.path()),
        governance_database_path(root.path()),
        thread_shard(root.path(), STALE_ROOT),
        retained_context.clone(),
    ]);

    let mut dry_run = SessionCleanupReport::default();
    count_session_family_data(root.path(), &stale, CUTOFF_MS, &mut dry_run)
        .expect("dry-run 统计应成功");
    assert_eq!(dry_run.deleted_runtime_session_count, 2);
    // 事件已随会话目录删除，不再按表计数。
    assert_eq!(dry_run.deleted_session_event_count, 0);
    assert_eq!(dry_run.deleted_governance_count, 2);
    assert_eq!(dry_run.deleted_thread_file_count, 2);
    assert_eq!(dry_run.deleted_session_notes_count, 1);
    assert_eq!(dry_run.deleted_note_file_count, 2);
    assert_eq!(dry_run.deleted_archived_shard_count, 1);
    assert!(thread_exists(root.path(), STALE_ROOT));
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
    cleanup_session_family_data(root.path(), CUTOFF_MS, &stale, &mut report)
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
    assert!(!thread_exists(root.path(), STALE_ROOT));
    assert!(!runtime_exists(root.path(), STALE_ROOT));
    assert!(!root.path().join("sessions").join(STALE_ROOT).exists());
    assert!(
        thread_exists(root.path(), RETAINED_SESSION),
        "保留会话的 thread 事件不得被清理"
    );
    assert_eq!(
        std::fs::read(retained_context).expect("retained context 应可读"),
        b"retained-session-bytes\n"
    );
}

#[test]
fn active_or_execution_referenced_member_protects_whole_family() {
    for (status, with_execution) in [("running", false), ("idle", true)] {
        let root = axon_test_support::test_temp_dir();
        create_thread(root.path(), STALE_ROOT, None, 100);
        create_thread(root.path(), STALE_CHILD, Some(STALE_ROOT), 200);
        create_runtime_family(root.path(), status, with_execution);

        let stale = stale_session_family_ids(root.path(), CUTOFF_MS).expect("保护集合应可读取");

        assert!(stale.is_empty());
        assert!(thread_exists(root.path(), STALE_ROOT));
        assert!(thread_exists(root.path(), STALE_CHILD));
    }
}

#[test]
fn current_resume_session_family_is_protected() {
    let root = axon_test_support::test_temp_dir();
    create_thread(root.path(), STALE_ROOT, None, 100);
    create_thread(root.path(), STALE_CHILD, Some(STALE_ROOT), 200);
    let service = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore);
    service
        .start_empty_thread(
            root.path(),
            axon_session::StartEmptyThreadCommand {
                thread_id: STALE_CHILD.to_owned(),
                session_root_id: STALE_ROOT.to_owned(),
                parent_thread_id: Some(STALE_ROOT.to_owned()),
                forked_from_thread_id: None,
                source: axon_session::ThreadSource::CliRun,
                ephemeral: false,
                settings: axon_session::ThreadSettings {
                    cwd: ".".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 200,
            },
        )
        .expect("active thread 应可写入");
    // 把 --continue 指针指向子会话：整族受保护。
    let data = root.path().join(axon_session::AXON_RUNTIME_DATA_DIR);
    let store = axon_session::SqliteRuntimeStore::from_root(&data);
    store.initialize().expect("runtime schema 应可初始化");
    let session =
        axon_domain::DaemonSessionId::new(STALE_CHILD.to_owned()).expect("会话 ID 应有效");
    axon_session::ResumePointerStore::write_resume_pointer(&store, &session, 1)
        .expect("恢复指针应可写入");

    let stale = stale_session_family_ids(root.path(), CUTOFF_MS).expect("候选应可读取");

    assert!(
        stale.is_empty(),
        "当前 --continue 会话的家族不得被清理：{stale:?}"
    );
}

#[test]
fn context_failure_keeps_retry_anchor() {
    let root = axon_test_support::test_temp_dir();
    create_thread(root.path(), STALE_ROOT, None, 100);
    create_thread(root.path(), STALE_CHILD, Some(STALE_ROOT), 200);
    let invalid_context = root.path().join("sessions").join(STALE_ROOT);
    std::fs::create_dir_all(invalid_context.parent().expect("context 必须有父目录"))
        .expect("context 父目录应可创建");
    std::fs::write(&invalid_context, b"not-a-directory").expect("异常 context 应可创建");
    let stale = vec![STALE_ROOT.to_owned(), STALE_CHILD.to_owned()];

    let error = cleanup_session_family_data(
        root.path(),
        CUTOFF_MS,
        &stale,
        &mut SessionCleanupReport::default(),
    )
    .expect_err("context 删除失败必须向上传递");

    assert!(error.contains("删除会话上下文目录"));
    // 上下文删除失败时，thread 文件仍是下一次清理的重试入口。
    assert!(thread_exists(root.path(), STALE_ROOT));
    assert!(thread_exists(root.path(), STALE_CHILD));
}

#[test]
fn governance_or_runtime_read_failure_stops_candidate_generation() {
    for broken_store in ["governance", "runtime"] {
        let root = axon_test_support::test_temp_dir();
        create_thread(root.path(), STALE_ROOT, None, 100);
        create_thread(root.path(), STALE_CHILD, Some(STALE_ROOT), 200);
        let broken_path = if broken_store == "governance" {
            governance_database_path(root.path())
        } else {
            runtime_path(root.path())
        };
        std::fs::create_dir_all(broken_path.parent().expect("损坏存储必须有父目录"))
            .expect("损坏存储目录应可创建");
        std::fs::write(&broken_path, b"not-sqlite").expect("损坏存储应可创建");

        stale_session_family_ids(root.path(), CUTOFF_MS)
            .expect_err("关联 SQLite 读取失败必须终止候选生成");

        assert!(thread_exists(root.path(), STALE_ROOT));
        assert!(thread_exists(root.path(), STALE_CHILD));
    }
}

#[test]
fn thread_cleanup_crosses_bounded_batch_boundary() {
    let root = axon_test_support::test_temp_dir();
    for index in 0..=SESSION_FAMILY_BATCH_LIMIT {
        create_thread(root.path(), &format!("stale-{index:04}"), None, 1);
    }
    let stale = stale_session_family_ids(root.path(), CUTOFF_MS).expect("批量候选应可读取");
    assert_eq!(stale.len(), SESSION_FAMILY_BATCH_LIMIT + 1);

    let mut report = SessionCleanupReport::default();
    cleanup_session_family_data(root.path(), CUTOFF_MS, &stale, &mut report)
        .expect("跨批次清理应成功");

    assert_eq!(report.deleted_session_count, SESSION_FAMILY_BATCH_LIMIT + 1);
    assert_eq!(thread_file_count(root.path()), 0);
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
        for index in 0..FAMILY_COUNT {
            create_thread(
                root.path(),
                &format!("sample-{sample:02}-stale-{index:04}"),
                None,
                1,
            );
        }
        let stale = stale_session_family_ids(root.path(), CUTOFF_MS).expect("候选应可读取");
        let started = Instant::now();
        cleanup_session_family_data(
            root.path(),
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

/// 写一条最小可投影的 thread：`threadStarted` 的 recordedAtMs 就是活动时间。
fn create_thread(root: &Path, session_id: &str, parent: Option<&str>, updated_at_ms: u64) {
    let service = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore);
    service
        .start_empty_thread(
            root,
            axon_session::StartEmptyThreadCommand {
                thread_id: session_id.to_owned(),
                session_root_id: parent.unwrap_or(session_id).to_owned(),
                parent_thread_id: parent.map(ToOwned::to_owned),
                forked_from_thread_id: None,
                source: axon_session::ThreadSource::CliRun,
                ephemeral: false,
                settings: axon_session::ThreadSettings {
                    cwd: ".".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: updated_at_ms,
            },
        )
        .expect("thread fixture 应可写入");
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
                "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?1,NULL,'private','primary','/workspace',?2,NULL,NULL,0,0,1,1,1,1,1)",
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
    std::fs::write(&path, b"retained-session-bytes\n").expect("retained context 应可写入");
    path
}

fn thread_exists(root: &Path, session_id: &str) -> bool {
    axon_session::JsonlThreadEventStore::thread_shard_paths(root, session_id)
        .map(|paths| !paths.is_empty())
        .unwrap_or(false)
}

fn thread_shard(root: &Path, session_id: &str) -> PathBuf {
    axon_session::JsonlThreadEventStore::thread_shard_paths(root, session_id)
        .expect("thread shard 应可读取")
        .into_iter()
        .next()
        .expect("thread shard 必须存在")
}

fn thread_file_count(root: &Path) -> usize {
    let directory = axon_session::JsonlThreadEventStore::path(root);
    match std::fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .count(),
        Err(_) => 0,
    }
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
