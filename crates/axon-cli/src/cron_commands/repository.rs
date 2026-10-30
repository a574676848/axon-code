use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use axon_crypto::sha256_hex_prefix;
use axon_domain::{CronJob, CronJobStatus, CronRunRecord};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

const CRON_RUNTIME_DIR: &str = "cron";
const CRON_DATABASE_FILE: &str = "cron.sqlite3";
const CRON_DATABASE_SCHEMA_VERSION: i64 = 2;
const CRON_BUSY_TIMEOUT_MS: u64 = 5_000;
const CRON_CLAIM_LEASE_MS: u64 = 120_000;
const CRON_HEARTBEAT_MS: u64 = 30_000;
const CRON_HISTORY_VACUUM_PAGE_BATCH: u32 = 256;
/// 单批删除的 cron run 行数上限：直接决定 cron 库单次写锁持有时间。
const CRON_HISTORY_BATCH_LIMIT: usize = 256;
/// 单轮 cron 历史清理的墙上时钟预算：超出即停止，剩余记录由下一轮清理续跑。
const CRON_HISTORY_TIME_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub(crate) struct CronRepository {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct CronClaim {
    pub(crate) claim_id: String,
    pub(crate) job: CronJob,
}

impl CronRepository {
    pub(crate) fn open(axon_root: &Path) -> Result<Self, String> {
        let runtime_dir = axon_root.join(CRON_RUNTIME_DIR);
        std::fs::create_dir_all(&runtime_dir)
            .map_err(|error| format!("创建 cron 运行目录失败：{error}"))?;
        let repository = Self {
            path: runtime_dir.join(CRON_DATABASE_FILE),
        };
        repository.connection()?;
        Ok(repository)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn save_job(&self, job: &CronJob) -> Result<(), String> {
        let connection = self.connection()?;
        let payload = encode(job, "cron job")?;
        let changed = connection
            .execute(
                "INSERT INTO cron_jobs (
                    job_id, payload_json, status, next_run_at_ms, revision, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(job_id) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    status = excluded.status,
                    next_run_at_ms = excluded.next_run_at_ms,
                    revision = excluded.revision,
                    updated_at_ms = excluded.updated_at_ms
                 WHERE excluded.revision > cron_jobs.revision",
                params![
                    job.job_id,
                    payload,
                    job.status.as_str(),
                    to_i64_opt(job.next_run_at_ms)?,
                    to_i64(job.revision)?,
                    to_i64(job.updated_at_ms)?,
                ],
            )
            .map_err(db_error("保存 cron job"))?;
        if changed == 0 {
            return Err(format!(
                "cron job revision 冲突，拒绝覆盖较新状态：{}",
                job.job_id
            ));
        }
        Ok(())
    }

    pub(crate) fn jobs(&self, include_removed: bool) -> Result<Vec<CronJob>, String> {
        let connection = self.connection()?;
        let sql = if include_removed {
            "SELECT payload_json FROM cron_jobs ORDER BY created_at_ms, job_id"
        } else {
            "SELECT payload_json FROM cron_jobs WHERE status <> ?1 ORDER BY created_at_ms, job_id"
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(db_error("读取 cron jobs"))?;
        let rows = if include_removed {
            statement.query_map([], decode_job_row)
        } else {
            statement.query_map([CronJobStatus::Removed.as_str()], decode_job_row)
        }
        .map_err(db_error("查询 cron jobs"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 cron jobs"))
    }

    pub(crate) fn find_job(&self, job_id: &str) -> Result<Option<CronJob>, String> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT payload_json FROM cron_jobs WHERE job_id = ?1 AND status <> ?2",
                params![job_id, CronJobStatus::Removed.as_str()],
                decode_job_row,
            )
            .optional()
            .map_err(db_error("读取 cron job"))
    }

    pub(crate) fn find_job_any_status(&self, job_id: &str) -> Result<Option<CronJob>, String> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT payload_json FROM cron_jobs WHERE job_id = ?1",
                [job_id],
                decode_job_row,
            )
            .optional()
            .map_err(db_error("读取 cron job"))
    }

    pub(crate) fn claim_due(&self, now_ms: u64) -> Result<Option<CronClaim>, String> {
        self.claim_due_matching(now_ms)
    }

    fn claim_due_matching(&self, now_ms: u64) -> Result<Option<CronClaim>, String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("开始 cron claim 事务"))?;
        let payload = transaction
            .query_row(
                "SELECT payload_json FROM cron_jobs
                 WHERE status = ?1
                   AND next_run_at_ms IS NOT NULL
                   AND next_run_at_ms <= ?2
                   AND (claim_until_ms IS NULL OR claim_until_ms <= ?2)
                 ORDER BY next_run_at_ms, job_id
                 LIMIT 1",
                params![CronJobStatus::Active.as_str(), to_i64(now_ms)?],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("查询到期 cron job"))?;
        let claim = payload
            .map(|payload| {
                let job: CronJob = decode_job(&payload)?;
                let claim_id = claim_id(&job.job_id, now_ms);
                let changed = transaction
                    .execute(
                        "UPDATE cron_jobs SET claim_id = ?1, claim_until_ms = ?2
                         WHERE job_id = ?3
                           AND (claim_until_ms IS NULL OR claim_until_ms <= ?4)",
                        params![
                            claim_id,
                            to_i64(now_ms.saturating_add(CRON_CLAIM_LEASE_MS))?,
                            job.job_id,
                            to_i64(now_ms)?,
                        ],
                    )
                    .map_err(db_error("认领 cron job"))?;
                (changed == 1)
                    .then_some(CronClaim { claim_id, job })
                    .ok_or_else(|| "cron job 在认领事务中失去可用状态".to_owned())
            })
            .transpose()?;
        transaction
            .commit()
            .map_err(db_error("提交 cron claim 事务"))?;
        Ok(claim)
    }

    pub(crate) fn complete_claim(
        &self,
        claim: &CronClaim,
        run: &CronRunRecord,
        next_job: &CronJob,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("开始 cron 完成事务"))?;
        let current = transaction
            .query_row(
                "SELECT payload_json FROM cron_jobs WHERE job_id = ?1 AND claim_id = ?2",
                params![claim.job.job_id, claim.claim_id],
                decode_job_row,
            )
            .optional()
            .map_err(db_error("读取完成时的 Cron 配置"))?
            .ok_or_else(|| "cron job claim 已失效，拒绝重复完成".to_owned())?;
        // 执行期间允许暂停或编辑；完成回执仅合并执行事实，保留较新的配置。
        let next_job = if current.revision == claim.job.revision {
            next_job.clone()
        } else {
            let mut latest = current;
            latest.last_run_at_ms = Some(run.finished_at_ms);
            latest.revision = latest.revision.saturating_add(1);
            latest.updated_at_ms = latest.updated_at_ms.max(run.finished_at_ms);
            latest
        };
        insert_run(&transaction, run)?;
        let changed = transaction
            .execute(
                "UPDATE cron_jobs SET
                    payload_json = ?1, status = ?2, next_run_at_ms = ?3,
                    revision = ?4, updated_at_ms = ?5,
                    claim_id = NULL, claim_until_ms = NULL
                 WHERE job_id = ?6 AND claim_id = ?7",
                params![
                    encode(&next_job, "cron job")?,
                    next_job.status.as_str(),
                    to_i64_opt(next_job.next_run_at_ms)?,
                    to_i64(next_job.revision)?,
                    to_i64(next_job.updated_at_ms)?,
                    claim.job.job_id,
                    claim.claim_id,
                ],
            )
            .map_err(db_error("完成 cron job"))?;
        if changed != 1 {
            return Err("cron job claim 已失效，拒绝重复完成".to_owned());
        }
        transaction.commit().map_err(db_error("提交 cron 完成事务"))
    }

    #[cfg(test)]
    pub(crate) fn append_run(&self, run: &CronRunRecord) -> Result<(), String> {
        insert_run(&self.connection()?, run)
    }

    pub(crate) fn cleanup_history(&self, cutoff_ms: u64) -> Result<(usize, usize), String> {
        let mut connection = self.connection()?;
        let cutoff = to_i64(cutoff_ms)?;
        let deleted_runs = delete_expired_runs(&mut connection, cutoff)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("开始 cron 历史清理事务"))?;
        let deleted_jobs = transaction
            .execute(
                "DELETE FROM cron_jobs
                 WHERE status = ?1 AND updated_at_ms < ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM cron_runs WHERE cron_runs.job_id = cron_jobs.job_id
                   )",
                params![CronJobStatus::Removed.as_str(), cutoff],
            )
            .map_err(db_error("清理已移除 cron jobs"))?;
        transaction
            .commit()
            .map_err(db_error("提交 cron 历史清理事务"))?;
        if deleted_runs > 0 || deleted_jobs > 0 {
            connection
                .execute_batch(&format!(
                    "PRAGMA incremental_vacuum({CRON_HISTORY_VACUUM_PAGE_BATCH})"
                ))
                .map_err(db_error("回收 cron 历史存储空间"))?;
        }
        Ok((deleted_runs, deleted_jobs))
    }

    pub(crate) fn history_counts(&self, cutoff_ms: u64) -> Result<(usize, usize), String> {
        let connection = self.connection()?;
        let cutoff = to_i64(cutoff_ms)?;
        let runs: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM cron_runs WHERE finished_at_ms < ?1",
                [cutoff],
                |row| row.get(0),
            )
            .map_err(db_error("统计 cron 历史 runs"))?;
        let jobs: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM cron_jobs
                 WHERE status = ?1 AND updated_at_ms < ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM cron_runs WHERE cron_runs.job_id = cron_jobs.job_id
                   )",
                params![CronJobStatus::Removed.as_str(), cutoff],
                |row| row.get(0),
            )
            .map_err(db_error("统计已移除 cron jobs"))?;
        Ok((
            usize::try_from(runs).unwrap_or(usize::MAX),
            usize::try_from(jobs).unwrap_or(usize::MAX),
        ))
    }

    pub(crate) fn heartbeat(&self, claim_id: &str, now_ms: u64) -> Result<(), String> {
        if self.renew_claim(claim_id, now_ms)? {
            return Ok(());
        }
        Err(format!("cron job claim 已失效：{claim_id}"))
    }

    pub(super) fn start_heartbeat(&self, claim_id: String) -> CronClaimHeartbeat {
        CronClaimHeartbeat::start(self.clone(), claim_id)
    }

    fn renew_claim(&self, claim_id: &str, now_ms: u64) -> Result<bool, String> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE cron_jobs SET claim_until_ms = ?1 WHERE claim_id = ?2",
                params![
                    to_i64(now_ms.saturating_add(CRON_CLAIM_LEASE_MS))?,
                    claim_id,
                ],
            )
            .map_err(db_error("续租 cron claim"))?;
        Ok(changed == 1)
    }

    pub(super) fn connection(&self) -> Result<Connection, String> {
        let mut connection = Connection::open(&self.path).map_err(db_error("打开 cron SQLite"))?;
        connection
            .busy_timeout(Duration::from_millis(CRON_BUSY_TIMEOUT_MS))
            .map_err(db_error("配置 cron SQLite busy timeout"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
            )
            .map_err(db_error("配置 cron SQLite"))?;
        let schema_version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(db_error("读取 cron schema version"))?;
        // 版本不匹配时只做「补齐 + 搬迁重建」，绝不清理业务表：这里曾经是整表 DROP
        // 重建，一次版本号变更就会清空全部定时任务与运行历史。
        if schema_version != CRON_DATABASE_SCHEMA_VERSION {
            schema::align_schema(&mut connection, CRON_DATABASE_SCHEMA_VERSION)?;
        }
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS cron_agent_config (
            job_id TEXT PRIMARY KEY, model TEXT NOT NULL, reasoning_effort TEXT NOT NULL)",
            )
            .map_err(db_error("创建 Cron 模型配置表"))?;
        Ok(connection)
    }

    pub(crate) fn latest_run_for_job(&self, job_id: &str) -> Result<Option<CronRunRecord>, String> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT payload_json FROM cron_runs WHERE job_id = ?1 ORDER BY started_at_ms DESC LIMIT 1",
                [job_id],
                |row| {
                    let payload = row.get::<_, String>(0)?;
                    serde_json::from_str(&payload).map_err(|_| rusqlite::Error::InvalidQuery)
                },
            )
            .optional()
            .map_err(db_error("读取 cron 最近执行记录"))
    }
}

pub(super) struct CronClaimHeartbeat {
    repository: CronRepository,
    claim_id: String,
    claim_lost: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    wake: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl CronClaimHeartbeat {
    fn start(repository: CronRepository, claim_id: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let claim_lost = Arc::new(AtomicBool::new(false));
        let worker_claim_lost = Arc::clone(&claim_lost);
        let worker_repository = repository.clone();
        let worker_claim_id = claim_id.clone();
        let (wake, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                if receiver
                    .recv_timeout(Duration::from_millis(CRON_HEARTBEAT_MS))
                    .is_ok()
                {
                    break;
                }
                match worker_repository.renew_claim(&worker_claim_id, axon_time::unix_now_ms()) {
                    Ok(true) => {}
                    Ok(false) => {
                        worker_claim_lost.store(true, Ordering::SeqCst);
                        axon_logger::warning(&format!(
                            "cron heartbeat 停止：claim 已失效：{worker_claim_id}"
                        ));
                        break;
                    }
                    Err(error) => axon_logger::warning(&format!(
                        "cron heartbeat 续租失败，将在下一周期重试：{error}"
                    )),
                }
            }
        });
        Self {
            repository,
            claim_id,
            claim_lost,
            stop,
            wake,
            thread: Some(thread),
        }
    }

    pub(super) fn finish(mut self) -> Result<(), String> {
        self.stop_worker()?;
        if self.claim_lost.load(Ordering::SeqCst) {
            return Err(format!("cron job claim 已失效：{}", self.claim_id));
        }
        self.repository
            .heartbeat(&self.claim_id, axon_time::unix_now_ms())
    }

    fn stop_worker(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.wake.send(());
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| "cron heartbeat worker panic，无法确认 claim 状态".to_owned())
    }
}

impl Drop for CronClaimHeartbeat {
    fn drop(&mut self) {
        if let Err(error) = self.stop_worker() {
            axon_logger::error(&error);
        }
    }
}

/// 按有界批次回收过期 cron run。
///
/// 每批独立 `IMMEDIATE` 事务并按 `finished_at_ms` 索引定位，单次写锁只覆盖
/// 至多 `CRON_HISTORY_BATCH_LIMIT` 行；预算耗尽立即返回，剩余记录由下一轮
/// 清理幂等续跑，避免历史清理长时间阻塞 Cron 调度写入。
fn delete_expired_runs(connection: &mut Connection, cutoff: i64) -> Result<usize, String> {
    let deadline = Instant::now() + CRON_HISTORY_TIME_BUDGET;
    let batch_limit = i64::try_from(CRON_HISTORY_BATCH_LIMIT).unwrap_or(i64::MAX);
    let mut deleted = 0usize;
    loop {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("开始 cron 历史清理事务"))?;
        let batch = transaction
            .execute(
                "DELETE FROM cron_runs
                 WHERE run_id IN (
                     SELECT run_id FROM cron_runs
                     WHERE finished_at_ms < ?1
                     ORDER BY finished_at_ms ASC
                     LIMIT ?2
                 )",
                params![cutoff, batch_limit],
            )
            .map_err(db_error("清理 cron 历史 runs"))?;
        transaction
            .commit()
            .map_err(db_error("提交 cron 历史清理事务"))?;
        deleted = deleted.saturating_add(batch);
        if batch < CRON_HISTORY_BATCH_LIMIT || Instant::now() >= deadline {
            break;
        }
    }
    Ok(deleted)
}

fn insert_run(connection: &Connection, run: &CronRunRecord) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO cron_runs
                (run_id, job_id, payload_json, started_at_ms, finished_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(run_id) DO NOTHING",
            params![
                run.run_id,
                run.job_id,
                encode(run, "cron run")?,
                to_i64(run.started_at_ms)?,
                to_i64(run.finished_at_ms)?,
            ],
        )
        .map_err(db_error("保存 cron run"))?;
    Ok(())
}

/// 解析 Cron 任务载荷，并强制校验持久化模型版本。
///
/// `wake` 在领域模型里是 `Option`，serde 会把「字段缺失」也算成 `None`：历史版本的
/// 载荷因此会被静默降级成「不投递」，把用户声明过的目标丢掉。这里按版本号显式拒绝，
/// 让调用方直接看到可执行的迁移提示。
fn decode_job(payload: &str) -> Result<CronJob, String> {
    let value: serde_json::Value =
        serde_json::from_str(payload).map_err(|error| format!("解析 cron job 失败：{error}"))?;
    let version = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if version != u64::from(axon_domain::CRON_SCHEMA_VERSION) {
        return Err(format!(
            "cron 任务载荷版本为 {version}，当前支持 {}；请先完成 wake 目标迁移",
            axon_domain::CRON_SCHEMA_VERSION
        ));
    }
    serde_json::from_value(value).map_err(|error| format!("解析 cron job 失败：{error}"))
}

fn decode_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronJob> {
    let payload = row.get::<_, String>(0)?;
    decode_job(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            std::io::Error::new(std::io::ErrorKind::InvalidData, error).into(),
        )
    })
}

fn encode<T: serde::Serialize>(value: &T, label: &str) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("序列化 {label} 失败：{error}"))
}

fn to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "cron 时间或版本超出 SQLite INTEGER 范围".to_owned())
}

fn to_i64_opt(value: Option<u64>) -> Result<Option<i64>, String> {
    value.map(to_i64).transpose()
}

fn claim_id(job_id: &str, now_ms: u64) -> String {
    let source = format!("{job_id}:{now_ms}:{}", std::process::id());
    format!("cron-claim-{}", sha256_hex_prefix(source.as_bytes(), 20))
}

fn db_error(context: &'static str) -> impl FnOnce(rusqlite::Error) -> String {
    move |error| format!("{context}失败：{error}")
}

#[path = "repository_manual.rs"]
mod manual;
#[path = "repository_reads.rs"]
mod reads;
#[cfg(test)]
#[path = "repository_tests.rs"]
mod repository_tests;
#[path = "repository_schema.rs"]
mod schema;
