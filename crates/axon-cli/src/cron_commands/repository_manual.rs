//! 手动执行与调度共享认领租约，避免同一任务并发执行。
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};

use super::{
    CRON_CLAIM_LEASE_MS, CronClaim, CronRepository, claim_id, db_error, decode_job_row, to_i64,
};
use axon_domain::CronJobStatus;

impl CronRepository {
    pub(crate) fn claim_manual(&self, job_id: &str, now_ms: u64) -> Result<CronClaim, String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("开始手动 Cron 认领事务"))?;
        let job = transaction
            .query_row(
                "SELECT payload_json FROM cron_jobs WHERE job_id = ?1 AND status <> ?2",
                params![job_id, CronJobStatus::Removed.as_str()],
                decode_job_row,
            )
            .optional()
            .map_err(db_error("读取手动 Cron 任务"))?
            .ok_or_else(|| "未找到 Cron 任务".to_owned())?;
        let claim_id = claim_id(job_id, now_ms);
        let changed = transaction
            .execute(
                "UPDATE cron_jobs SET claim_id = ?1, claim_until_ms = ?2 WHERE job_id = ?3
             AND (claim_until_ms IS NULL OR claim_until_ms <= ?4)",
                params![
                    claim_id,
                    to_i64(now_ms.saturating_add(CRON_CLAIM_LEASE_MS))?,
                    job_id,
                    to_i64(now_ms)?
                ],
            )
            .map_err(db_error("认领手动 Cron 任务"))?;
        if changed != 1 {
            return Err("任务正在执行，请等待当前执行结束".to_owned());
        }
        transaction
            .commit()
            .map_err(db_error("提交手动 Cron 认领事务"))?;
        Ok(CronClaim { claim_id, job })
    }

    pub(crate) fn is_running(&self, job_id: &str, now_ms: u64) -> Result<bool, String> {
        self.connection()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cron_jobs WHERE job_id = ?1 AND claim_until_ms > ?2)",
                params![job_id, to_i64(now_ms)?],
                |row| row.get(0),
            )
            .map_err(db_error("读取 Cron 执行状态"))
    }
}
