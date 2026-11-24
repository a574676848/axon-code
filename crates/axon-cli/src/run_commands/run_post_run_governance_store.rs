//! Post-run governance SQLite 工作队列。

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

const GOVERNANCE_DB_FILE: &str = "post_run_governance.sqlite3";
const GOVERNANCE_SCHEMA_VERSION: i64 = 2;
const GOVERNANCE_STATUS_QUEUED: &str = "queued";
const GOVERNANCE_STATUS_RUNNING: &str = "running";
const GOVERNANCE_STATUS_COMPLETED: &str = "completed";
const GOVERNANCE_STATUS_FAILED: &str = "failed";
const GOVERNANCE_LEASE_MS: u64 = 5 * 60 * 1_000;
const GOVERNANCE_RETRY_DELAY_MS: u64 = 5_000;
const GOVERNANCE_SQLITE_BUSY_TIMEOUT_MS: u64 = 2_000;
const GOVERNANCE_AUDIT_RETENTION_MS: u64 = 60 * 60 * 1_000;
const GOVERNANCE_VACUUM_PAGE_BATCH: u32 = 256;
const GOVERNANCE_AUDIT_VERSION: u32 = 1;
pub(super) const GOVERNANCE_MAX_ATTEMPTS: u32 = 3;

pub(super) struct GovernanceQueue {
    connection: Connection,
    path: PathBuf,
}

pub(super) struct ClaimedGovernanceJob {
    pub(super) job_id: String,
    pub(super) payload_json: String,
    pub(super) attempts: u32,
}

impl GovernanceQueue {
    pub(super) fn open(config_path: &Path) -> Result<Self, String> {
        let path = axon_session::session_store_path(&super::run_session_store::resolved_axon_root(
            config_path,
        )?)
        .join(GOVERNANCE_DB_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| "governance SQLite 目录创建失败".to_owned())?;
        }
        let connection =
            Connection::open(&path).map_err(|_| "governance SQLite 数据库打开失败".to_owned())?;
        connection
            .pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .map_err(|_| "governance SQLite 自动空间回收配置失败".to_owned())?;
        connection
            .busy_timeout(Duration::from_millis(GOVERNANCE_SQLITE_BUSY_TIMEOUT_MS))
            .map_err(|_| "governance SQLite busy timeout 配置失败".to_owned())?;
        let queue = Self { connection, path };
        queue.ensure_schema()?;
        Ok(queue)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn enqueue(
        &self,
        job_id: &str,
        session_id: &str,
        payload_json: &str,
        now_ms: u64,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO governance_jobs (
                    job_id, session_id, payload_json, status, attempts,
                    available_at_ms, lease_expires_at_ms, created_at_ms, updated_at_ms, last_error
                 ) VALUES (?1, ?2, ?3, ?4, 0, ?5, NULL, ?5, ?5, NULL)
                 ON CONFLICT(job_id) DO NOTHING",
                params![
                    job_id,
                    session_id,
                    payload_json,
                    GOVERNANCE_STATUS_QUEUED,
                    timestamp(now_ms)
                ],
            )
            .map_err(|_| "governance job 入队失败".to_owned())?;
        Ok(())
    }

    pub(super) fn claim_next(
        &mut self,
        now_ms: u64,
    ) -> Result<Option<ClaimedGovernanceJob>, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| "governance claim 事务启动失败".to_owned())?;
        let selected = transaction
            .query_row(
                "SELECT job_id, payload_json, attempts
                 FROM governance_jobs
                 WHERE (status = ?1 AND available_at_ms <= ?3)
                    OR (status = ?2 AND lease_expires_at_ms <= ?3)
                 ORDER BY created_at_ms, job_id
                 LIMIT 1",
                params![
                    GOVERNANCE_STATUS_QUEUED,
                    GOVERNANCE_STATUS_RUNNING,
                    timestamp(now_ms)
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| "governance job 查询失败".to_owned())?;
        let Some((job_id, payload_json, attempts)) = selected else {
            transaction
                .commit()
                .map_err(|_| "governance 空 claim 提交失败".to_owned())?;
            return Ok(None);
        };
        let next_attempt = attempts.saturating_add(1);
        let lease_expires_at_ms = now_ms.saturating_add(GOVERNANCE_LEASE_MS);
        transaction
            .execute(
                "UPDATE governance_jobs
                 SET status = ?2, attempts = ?3, lease_expires_at_ms = ?4, updated_at_ms = ?5
                 WHERE job_id = ?1",
                params![
                    job_id,
                    GOVERNANCE_STATUS_RUNNING,
                    next_attempt,
                    timestamp(lease_expires_at_ms),
                    timestamp(now_ms),
                ],
            )
            .map_err(|_| "governance job claim 更新失败".to_owned())?;
        transaction
            .commit()
            .map_err(|_| "governance claim 事务提交失败".to_owned())?;
        Ok(Some(ClaimedGovernanceJob {
            job_id,
            payload_json,
            attempts: next_attempt,
        }))
    }

    pub(super) fn next_queued_delay_ms(&self, now_ms: u64) -> Result<Option<u64>, String> {
        let available_at_ms = self
            .connection
            .query_row(
                "SELECT MIN(available_at_ms) FROM governance_jobs WHERE status = ?1",
                params![GOVERNANCE_STATUS_QUEUED],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(|_| "governance 下一重试时间查询失败".to_owned())?;
        Ok(available_at_ms.map(|available_at_ms| {
            u64::try_from(available_at_ms.max(0))
                .unwrap_or(u64::MAX)
                .saturating_sub(now_ms)
        }))
    }

    pub(super) fn complete(&self, job_id: &str, now_ms: u64) -> Result<(), String> {
        self.update_terminal(job_id, GOVERNANCE_STATUS_COMPLETED, None, now_ms)
    }

    pub(super) fn compact_expired_payloads(&mut self, now_ms: u64) -> Result<usize, String> {
        let cutoff_ms = now_ms.saturating_sub(GOVERNANCE_AUDIT_RETENTION_MS);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| "governance 审计降级事务启动失败".to_owned())?;
        let updated = transaction
            .execute(
                r"
                UPDATE governance_jobs
                SET payload_json = json_object(
                        'audit', 'summarized',
                        'version', ?1,
                        'sessionRecordId', json_extract(payload_json, '$.sessionRecord.sessionId'),
                        'assistantPartId', json_extract(payload_json, '$.sessionRecord.assistantPartId'),
                        'originalPayloadBytes', length(cast(payload_json as blob))
                )
                WHERE status = ?2
                  AND updated_at_ms <= ?3
                  AND json_extract(payload_json, '$.audit') IS NULL
                ",
                params![
                    GOVERNANCE_AUDIT_VERSION,
                    GOVERNANCE_STATUS_COMPLETED,
                    timestamp(cutoff_ms),
                ],
            )
            .map_err(|_| "governance completed payload 审计降级失败".to_owned())?;
        transaction
            .commit()
            .map_err(|_| "governance 审计降级事务提交失败".to_owned())?;
        if updated > 0 {
            self.connection
                .execute_batch(&format!(
                    "PRAGMA incremental_vacuum({GOVERNANCE_VACUUM_PAGE_BATCH});"
                ))
                .unwrap_or_else(|error| {
                    axon_logger::warning(&format!(
                        "跳过本轮 governance SQLite 增量空间回收：{error}"
                    ));
                });
        }
        Ok(updated)
    }

    pub(super) fn fail(
        &self,
        job_id: &str,
        attempts: u32,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), String> {
        if attempts >= GOVERNANCE_MAX_ATTEMPTS {
            return self.update_terminal(job_id, GOVERNANCE_STATUS_FAILED, Some(reason), now_ms);
        }
        self.connection
            .execute(
                "UPDATE governance_jobs
                 SET status = ?2, available_at_ms = ?3, lease_expires_at_ms = NULL,
                     updated_at_ms = ?4, last_error = ?5
                 WHERE job_id = ?1",
                params![
                    job_id,
                    GOVERNANCE_STATUS_QUEUED,
                    timestamp(now_ms.saturating_add(GOVERNANCE_RETRY_DELAY_MS)),
                    timestamp(now_ms),
                    reason,
                ],
            )
            .map_err(|_| "governance job 重试状态写入失败".to_owned())?;
        Ok(())
    }

    fn ensure_schema(&self) -> Result<(), String> {
        let current_version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| "governance SQLite schema 版本读取失败".to_owned())?;
        if current_version != 0 && current_version != GOVERNANCE_SCHEMA_VERSION {
            self.connection
                .execute_batch("DROP TABLE IF EXISTS governance_jobs;")
                .map_err(|_| "governance SQLite 旧 schema 清理失败".to_owned())?;
        }
        self.connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS governance_jobs (
                    job_id TEXT PRIMARY KEY NOT NULL,
                    session_id TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    status TEXT NOT NULL,
                    attempts INTEGER NOT NULL,
                    available_at_ms INTEGER NOT NULL,
                    lease_expires_at_ms INTEGER,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    last_error TEXT
                 );
                 CREATE INDEX IF NOT EXISTS idx_governance_claim
                    ON governance_jobs (status, available_at_ms, lease_expires_at_ms, created_at_ms);
                 CREATE INDEX IF NOT EXISTS idx_governance_retention
                    ON governance_jobs (status, updated_at_ms, session_id);",
            )
            .map_err(|_| "governance SQLite schema 初始化失败".to_owned())?;
        self.connection
            .pragma_update(None, "user_version", GOVERNANCE_SCHEMA_VERSION)
            .map_err(|_| "governance SQLite schema 版本写入失败".to_owned())?;
        Ok(())
    }

    fn update_terminal(
        &self,
        job_id: &str,
        status: &str,
        reason: Option<&str>,
        now_ms: u64,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE governance_jobs
                 SET status = ?2, lease_expires_at_ms = NULL, updated_at_ms = ?3, last_error = ?4
                 WHERE job_id = ?1",
                params![job_id, status, timestamp(now_ms), reason],
            )
            .map_err(|_| "governance job 终态写入失败".to_owned())?;
        Ok(())
    }
}

fn timestamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_recovers_expired_lease_and_stops_after_max_attempts() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-governance-queue-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        let mut queue = GovernanceQueue::open(&config_path).unwrap();
        queue.enqueue("job-1", "session-1", "{}", 10).unwrap();
        let first = queue.claim_next(10).unwrap().unwrap();
        assert_eq!(first.attempts, 1);
        assert!(queue.claim_next(11).unwrap().is_none());
        let recovered = queue.claim_next(10 + GOVERNANCE_LEASE_MS).unwrap().unwrap();
        assert_eq!(recovered.attempts, 2);
        queue
            .fail("job-1", GOVERNANCE_MAX_ATTEMPTS, "failed", 20)
            .unwrap();
        assert!(queue.claim_next(u64::MAX / 2).unwrap().is_none());
        queue.compact_expired_payloads(20).unwrap();
        let (status, payload): (String, String) = queue
            .connection
            .query_row(
                "SELECT status, payload_json FROM governance_jobs WHERE job_id = 'job-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, GOVERNANCE_STATUS_FAILED);
        assert_eq!(payload, "{}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn completed_payloads_expire_to_audit_summary() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-governance-audit-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        let mut queue = GovernanceQueue::open(&config_path).unwrap();
        let original_payload = r#"{"version":1,"sessionRecord":{"sessionId":"session-1","assistantPartId":"assistant-1","response":"大负载"}}"#;
        queue
            .enqueue("job-1", "session-1", original_payload, 10)
            .unwrap();
        let job = queue.claim_next(10).unwrap().unwrap();
        queue.complete(&job.job_id, 20).unwrap();

        assert_eq!(queue.compact_expired_payloads(20).unwrap(), 0);
        assert_eq!(
            queue
                .compact_expired_payloads(20 + GOVERNANCE_AUDIT_RETENTION_MS)
                .unwrap(),
            1
        );

        let payload: String = queue
            .connection
            .query_row(
                "SELECT payload_json FROM governance_jobs WHERE job_id = 'job-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let payload_value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert!(payload.contains(r#""audit":"summarized""#));
        assert!(payload.contains(r#""sessionRecordId":"session-1""#));
        assert_eq!(
            payload_value
                .get("originalPayloadBytes")
                .and_then(serde_json::Value::as_u64),
            Some(original_payload.len() as u64)
        );
        assert!(!payload.contains("大负载"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn queue_marks_non_retryable_failure_terminal_on_first_attempt() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-governance-terminal-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        let mut queue = GovernanceQueue::open(&config_path).unwrap();
        queue.enqueue("job-1", "session-1", "{}", 10).unwrap();
        let job = queue.claim_next(10).unwrap().unwrap();

        queue
            .fail(
                "job-1",
                GOVERNANCE_MAX_ATTEMPTS,
                "API key 环境变量未设置",
                11,
            )
            .unwrap();

        assert_eq!(job.attempts, 1);
        assert!(queue.claim_next(u64::MAX / 2).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn queued_retry_exposes_remaining_delay() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-governance-delay-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        let mut queue = GovernanceQueue::open(&config_path).unwrap();
        queue.enqueue("job-1", "session-1", "{}", 10).unwrap();
        let job = queue.claim_next(10).unwrap().unwrap();
        queue
            .fail(&job.job_id, job.attempts, "transient", 20)
            .unwrap();

        assert_eq!(
            queue.next_queued_delay_ms(20).unwrap(),
            Some(GOVERNANCE_RETRY_DELAY_MS)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn separate_workers_do_not_claim_the_same_job() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-governance-claim-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        let mut first_worker = GovernanceQueue::open(&config_path).unwrap();
        first_worker
            .enqueue("job-1", "session-1", "{}", 10)
            .unwrap();
        let mut second_worker = GovernanceQueue::open(&config_path).unwrap();

        assert!(first_worker.claim_next(10).unwrap().is_some());
        assert!(second_worker.claim_next(10).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
