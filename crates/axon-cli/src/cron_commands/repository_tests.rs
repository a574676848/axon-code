//! Cron 仓储契约测试：通用 cron 根、任务声明去重、schema 版本重建与历史清理。

use axon_domain::{CronJobInput, CronWakeTarget};

use super::*;

#[test]
fn repository_uses_generic_cron_root_and_fences_duplicate_claims() {
    let root = axon_test_support::test_temp_dir();
    let repository = CronRepository::open(root.path()).expect("repository 应可打开");
    let job = test_job(100);
    repository.save_job(&job).expect("job 应可保存");

    let claim = repository
        .claim_due(100)
        .expect("job 应可认领")
        .expect("应返回到期 job");

    assert_eq!(
        repository.path(),
        root.path().join(CRON_RUNTIME_DIR).join(CRON_DATABASE_FILE)
    );
    assert!(
        repository
            .claim_due(100)
            .expect("重复认领应正常返回")
            .is_none()
    );
    assert!(claim.claim_id.starts_with("cron-claim-"));
}

#[test]
fn manual_claim_excludes_scheduler_and_preserves_concurrent_updates() {
    let root = axon_test_support::test_temp_dir();
    let repository = CronRepository::open(root.path()).unwrap();
    let job = test_job(100);
    repository.save_job(&job).unwrap();
    let claim = repository.claim_manual(&job.job_id, 100).unwrap();
    assert!(repository.is_running(&job.job_id, 100).unwrap());
    assert!(repository.claim_manual(&job.job_id, 101).is_err());
    assert!(repository.claim_due(101).unwrap().is_none());
    let edited = job
        .with_update(
            axon_domain::CronJobUpdate {
                name: Some("执行中编辑".to_owned()),
                ..Default::default()
            },
            102,
            Some(900),
        )
        .unwrap()
        .with_status(CronJobStatus::Paused, 103);
    repository.save_job(&edited).unwrap();
    let run = CronRunRecord {
        schema_version: job.schema_version,
        run_id: "manual-completed".to_owned(),
        job_id: job.job_id.clone(),
        job_revision: job.revision,
        started_at_ms: 100,
        finished_at_ms: 110,
        success: true,
        agent_executed: true,
        delivery_enqueued: false,
        stdout_empty: false,
        dispatch_id: None,
        error_summary: None,
    };
    let next = job.after_run(110, Some(500), false);
    repository.complete_claim(&claim, &run, &next).unwrap();
    let persisted = repository.find_job(&job.job_id).unwrap().unwrap();
    assert_eq!(persisted.name, edited.name);
    assert_eq!(persisted.status, CronJobStatus::Paused);
    assert_eq!(persisted.next_run_at_ms, Some(900));
    assert_eq!(persisted.last_run_at_ms, Some(110));
    assert!(!repository.is_running(&job.job_id, 110).unwrap());
    assert!(repository.complete_claim(&claim, &run, &next).is_err());
    assert_eq!(repository.runs_page(&job.job_id, 20, 0).unwrap().1, 1);
}

#[test]
fn agent_configuration_remains_fixed_when_defaults_change() {
    let root = axon_test_support::test_temp_dir();
    let repository = CronRepository::open(root.path()).unwrap();
    let job = test_job(100);
    repository.save_job(&job).unwrap();
    assert!(repository.read_agent_config(&job.job_id).unwrap().is_none());
    let fixed = repository
        .agent_config(&job.job_id, "original-model", "high")
        .unwrap();
    assert_eq!(
        repository
            .agent_config(&job.job_id, "new-model", "low")
            .unwrap(),
        fixed
    );
    assert_eq!(
        repository.read_agent_config(&job.job_id).unwrap(),
        Some(fixed)
    );
}

#[test]
fn schema_version_mismatch_aligns_without_dropping_unrelated_objects() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join(CRON_RUNTIME_DIR).join(CRON_DATABASE_FILE);
    std::fs::create_dir_all(path.parent().expect("数据库应有父目录")).expect("父目录应可创建");
    let connection = Connection::open(&path).expect("旧数据库应可创建");
    connection
        .execute_batch("CREATE TABLE legacy(value TEXT); PRAGMA user_version = 99;")
        .expect("旧 schema 应可写入");
    drop(connection);

    let repository = CronRepository::open(root.path()).expect("新 schema 应可对齐");
    let connection = repository.connection().expect("数据库应可打开");
    let legacy_exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='legacy'",
            [],
            |row| row.get(0),
        )
        .expect("应可检查旧表");
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("应可读取 schema version");

    // 无关的遗留对象不属于 cron 的业务事实源，对齐过程不得顺手清理。
    assert_eq!(legacy_exists, 1);
    assert_eq!(version, CRON_DATABASE_SCHEMA_VERSION);
}

#[test]
fn legacy_cron_tables_keep_rows_when_columns_change() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join(CRON_RUNTIME_DIR).join(CRON_DATABASE_FILE);
    std::fs::create_dir_all(path.parent().expect("数据库应有父目录")).expect("父目录应可创建");
    let connection = Connection::open(&path).expect("旧数据库应可创建");
    // 旧形态：缺 claim_id/claim_until_ms 与 created_at_ms 生成列。
    connection
        .execute_batch(
            "CREATE TABLE cron_jobs (
                 job_id TEXT PRIMARY KEY, payload_json TEXT NOT NULL, status TEXT NOT NULL,
                 next_run_at_ms INTEGER, revision INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL);
             INSERT INTO cron_jobs VALUES ('legacy-job', '{}', 'active', 10, 3, 5);
             CREATE TABLE cron_runs (
                 run_id TEXT PRIMARY KEY, job_id TEXT NOT NULL, payload_json TEXT NOT NULL,
                 started_at_ms INTEGER NOT NULL, finished_at_ms INTEGER NOT NULL);
             INSERT INTO cron_runs VALUES ('legacy-run', 'legacy-job', '{}', 1, 2);
             PRAGMA user_version = 99;",
        )
        .expect("旧 schema 应可写入");
    drop(connection);

    let repository = CronRepository::open(root.path()).expect("旧库应可对齐");
    let connection = repository.connection().expect("数据库应可打开");
    let jobs: i64 = connection
        .query_row("SELECT COUNT(*) FROM cron_jobs", [], |row| row.get(0))
        .expect("应可统计 jobs");
    let runs: i64 = connection
        .query_row("SELECT COUNT(*) FROM cron_runs", [], |row| row.get(0))
        .expect("应可统计 runs");
    let (status, revision, claim_id): (String, i64, Option<String>) = connection
        .query_row(
            "SELECT status, revision, claim_id FROM cron_jobs WHERE job_id = 'legacy-job'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("历史行应可读取");

    // 列形态变化只能搬迁，不能清空：任务定义与运行历史都是唯一事实源。
    assert_eq!(jobs, 1);
    assert_eq!(runs, 1);
    assert_eq!(status, "active");
    assert_eq!(revision, 3);
    assert_eq!(claim_id, None);
}

#[test]
fn cleanup_history_removes_expired_runs_and_removed_jobs_only() {
    let root = axon_test_support::test_temp_dir();
    let repository = CronRepository::open(root.path()).expect("repository 应可打开");
    let old_job = test_job_with_wake("old-removed", 1, None).with_status(CronJobStatus::Removed, 1);
    repository.save_job(&old_job).expect("旧 job 应可保存");
    let active = test_job_with_wake("active", 1, None);
    repository.save_job(&active).expect("活动 job 应可保存");
    repository
        .append_run(&CronRunRecord {
            schema_version: 1,
            run_id: "old-run".to_owned(),
            job_id: active.job_id.clone(),
            job_revision: active.revision,
            success: true,
            started_at_ms: 1,
            finished_at_ms: 1,
            agent_executed: false,
            delivery_enqueued: false,
            stdout_empty: true,
            dispatch_id: None,
            error_summary: None,
        })
        .expect("旧 run 应可保存");

    assert_eq!(repository.cleanup_history(100).unwrap(), (1, 1));
    assert!(
        repository
            .find_job_any_status("old-removed")
            .unwrap()
            .is_none()
    );
    assert!(repository.find_job_any_status("active").unwrap().is_some());
}

/// cron 历史清理必须走 `finished_at_ms` 索引，并按有界批次推进。
///
/// 断言口径：单批 256 行的删除走索引范围扫描而非全表扫描，
/// 且超过三批的历史记录能被逐批幂等清空。
#[test]
fn cleanup_history_is_index_backed_and_batched() {
    let root = axon_test_support::test_temp_dir();
    let repository = CronRepository::open(root.path()).expect("repository 应可打开");
    let total = CRON_HISTORY_BATCH_LIMIT * 3 + 1;
    let connection = repository.connection().expect("cron 数据库应可打开");
    connection
        .execute_batch(&format!(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < {total})
                 INSERT INTO cron_runs(run_id, job_id, payload_json, started_at_ms, finished_at_ms)
                 SELECT 'run-'||n, 'batch-job', '{{}}', 1, 1 FROM seq;"
        ))
        .expect("历史记录应可写入");

    let plan = explain_cron_plan(
        &connection,
        "EXPLAIN QUERY PLAN DELETE FROM cron_runs WHERE run_id IN (
                 SELECT run_id FROM cron_runs
                 WHERE finished_at_ms < ?1
                 ORDER BY finished_at_ms ASC
                 LIMIT ?2
             )",
    );
    assert!(
        plan.contains("idx_cron_runs_finished_at"),
        "cron 历史清理应走 finished_at_ms 索引：{plan}"
    );
    assert!(
        !plan.contains("SCAN cron_runs"),
        "cron 历史清理不得全表扫描：{plan}"
    );

    let deleted_runs = repository.cleanup_history(100).expect("清理应成功").0;
    assert_eq!(deleted_runs, total);

    let remaining: i64 = connection
        .query_row("SELECT COUNT(*) FROM cron_runs", [], |row| row.get(0))
        .expect("统计应成功");
    assert_eq!(remaining, 0);
}

fn explain_cron_plan(connection: &Connection, sql: &str) -> String {
    let batch = i64::try_from(CRON_HISTORY_BATCH_LIMIT).expect("批次上限应可转换");
    let mut statement = connection.prepare(sql).expect("执行计划应可编译");
    let details = statement
        .query_map(params![1_i64, batch], |row| row.get::<_, String>(3))
        .expect("执行计划应可查询")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("执行计划应可读取");
    details.join("\n")
}

/// 容量验收：大规模 cron 历史必须按批次线性推进并在有界时间内清空。
#[test]
#[ignore = "容量验收：构造大规模 cron 历史验证批次清理耗时"]
fn cron_history_cleanup_drains_large_backlog() {
    let root = axon_test_support::test_temp_dir();
    let repository = CronRepository::open(root.path()).expect("repository 应可打开");
    let total = 20_000usize;
    {
        let connection = repository.connection().expect("cron 数据库应可打开");
        connection
                .execute_batch(&format!(
                    "BEGIN;
                     WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n < {total})
                     INSERT INTO cron_runs(run_id, job_id, payload_json, started_at_ms, finished_at_ms)
                     SELECT 'run-'||n, 'batch-job', '{{}}', 1, 1 FROM seq;
                     COMMIT;"
                ))
                .expect("历史记录应可写入");
    }

    let started = Instant::now();
    let deleted_runs = repository.cleanup_history(100).expect("清理应成功").0;
    let elapsed = started.elapsed();
    println!("cron 历史清理：{deleted_runs} 行 elapsed={elapsed:?}");

    assert_eq!(deleted_runs, total);
    assert!(
        elapsed.as_secs() < 60,
        "大规模 cron 历史清理必须在有界时间内完成，实测 {elapsed:?}"
    );
}

fn test_job(next_run_at_ms: u64) -> CronJob {
    test_job_with_wake(
        "cron-test",
        next_run_at_ms,
        Some(CronWakeTarget::Feishu("oc_test".to_owned())),
    )
}

fn test_job_with_wake(job_id: &str, next_run_at_ms: u64, wake: Option<CronWakeTarget>) -> CronJob {
    CronJob::new(
        CronJobInput {
            job_id: job_id.to_owned(),
            name: "测试".to_owned(),
            schedule: "interval:1h".to_owned(),
            prompt: "执行".to_owned(),
            skills: Vec::new(),
            wake,
            workdir: None,
            no_agent: false,
            script: None,
        },
        1,
        Some(next_run_at_ms),
    )
    .expect("测试 job 应有效")
}

#[test]
fn legacy_payload_version_is_rejected_before_migration() {
    let root = axon_test_support::test_temp_dir();
    let repository = CronRepository::open(root.path()).expect("repository 应可打开");
    // v1 载荷：带 delivery、没有 wake。serde 会把缺失的 Option 字段当成 None，
    // 所以必须靠版本门禁拦住它，否则任务会被静默降级成「不投递」。
    let payload = serde_json::json!({
        "schemaVersion": 1,
        "jobId": "legacy-v1",
        "name": "旧任务",
        "schedule": "interval:1h",
        "prompt": "执行",
        "skills": [],
        "delivery": {"adapter": "feishu", "sessionId": 0, "recipientId": "oc_legacy", "threadId": null},
        "workdir": null,
        "noAgent": false,
        "script": null,
        "status": "active",
        "revision": 1,
        "nextRunAtMs": 10,
        "lastRunAtMs": null,
        "createdAtMs": 1,
        "updatedAtMs": 1,
    })
    .to_string();
    repository
        .connection()
        .expect("连接应可打开")
        .execute(
            "INSERT INTO cron_jobs (job_id, payload_json, status, next_run_at_ms, revision, updated_at_ms)
             VALUES (?1, ?2, 'active', 10, 1, 1)",
            rusqlite::params!["legacy-v1", payload],
        )
        .expect("写入旧载荷");

    let error = repository.jobs(false).expect_err("v1 载荷必须被拒绝");
    assert!(error.contains("版本为 1"), "{error}");
    assert!(error.contains("迁移"), "错误里必须给出迁移路径：{error}");
}
