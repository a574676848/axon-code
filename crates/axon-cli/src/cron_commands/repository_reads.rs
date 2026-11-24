//! Cron 管理读模型：筛选和分页在 SQLite 完成，最近结果按已有索引关联。

use axon_domain::{CronJob, CronJobStatus, CronRunRecord};
use rusqlite::{Row, params};

use super::{CronRepository, db_error, decode_job_row};

const JOB_FILTER: &str = "j.status <> ?1 AND (?2 = '' OR j.status = ?2)
    AND (?3 = '' OR instr(lower(json_extract(j.payload_json, '$.name') || ' ' ||
        json_extract(j.payload_json, '$.schedule') || ' ' ||
        json_extract(j.payload_json, '$.prompt')), lower(?3)) > 0)";

pub(crate) struct CronJobRead {
    pub(crate) job: CronJob,
    pub(crate) latest_run: Option<CronRunRecord>,
    pub(crate) running: bool,
}

fn decode_run(row: &Row<'_>) -> rusqlite::Result<CronRunRecord> {
    let payload: String = row.get(0)?;
    serde_json::from_str(&payload).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn sqlite_count(value: i64) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| "Cron 记录总数超出范围".to_owned())
}

fn sqlite_offset(value: usize) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "Cron 分页参数超出范围".to_owned())
}

impl CronRepository {
    pub(crate) fn jobs_page(
        &self,
        status: &str,
        search: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<CronJobRead>, usize), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(db_error("开始 Cron 列表读取"))?;
        let removed = CronJobStatus::Removed.as_str();
        let total = transaction
            .query_row(
                &format!("SELECT COUNT(*) FROM cron_jobs j WHERE {JOB_FILTER}"),
                params![removed, status, search],
                |row| row.get::<_, i64>(0),
            )
            .map_err(db_error("统计 Cron 任务"))?;
        let mut statement = transaction
            .prepare(&format!(
                "SELECT j.payload_json, (SELECT r.payload_json FROM cron_runs r
                WHERE r.job_id = j.job_id ORDER BY r.started_at_ms DESC, r.run_id DESC LIMIT 1),
                COALESCE(j.claim_until_ms > ?6, 0)
             FROM cron_jobs j WHERE {JOB_FILTER}
             ORDER BY j.created_at_ms, j.job_id LIMIT ?4 OFFSET ?5"
            ))
            .map_err(db_error("读取 Cron 任务页"))?;
        let jobs = statement
            .query_map(
                params![
                    removed,
                    status,
                    search,
                    sqlite_offset(limit)?,
                    sqlite_offset(offset)?,
                    super::to_i64(axon_time::unix_now_ms())?
                ],
                |row| {
                    let latest: Option<String> = row.get(1)?;
                    let latest_run = latest
                        .map(|payload| {
                            serde_json::from_str(&payload)
                                .map_err(|_| rusqlite::Error::InvalidQuery)
                        })
                        .transpose()?;
                    Ok(CronJobRead {
                        job: decode_job_row(row)?,
                        latest_run,
                        running: row.get(2)?,
                    })
                },
            )
            .map_err(db_error("查询 Cron 任务页"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 Cron 任务页"))?;
        Ok((jobs, sqlite_count(total)?))
    }

    pub(crate) fn runs_page(
        &self,
        job_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<CronRunRecord>, usize), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(db_error("开始 Cron 历史读取"))?;
        let total = transaction
            .query_row(
                "SELECT COUNT(*) FROM cron_runs WHERE job_id = ?1",
                [job_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(db_error("统计 Cron 执行历史"))?;
        let mut statement = transaction
            .prepare(
                "SELECT payload_json FROM cron_runs WHERE job_id = ?1
             ORDER BY started_at_ms DESC, run_id DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(db_error("读取 Cron 历史页"))?;
        let runs = statement
            .query_map(
                params![job_id, sqlite_offset(limit)?, sqlite_offset(offset)?],
                decode_run,
            )
            .map_err(db_error("查询 Cron 历史页"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 Cron 历史页"))?;
        Ok((runs, sqlite_count(total)?))
    }
}
