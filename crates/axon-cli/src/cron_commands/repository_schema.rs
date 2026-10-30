//! cron SQLite schema 对齐：任何情况下都不删除既有数据。
//!
//! 版本号不匹配时只允许做三件事：建立缺失的表与索引、把形态不符的业务表按列映射
//! 搬迁重建、对齐 `PRAGMA user_version`。直接 `DROP` 业务表是被禁止的：一次版本号
//! 变更就会清空全部定时任务与运行历史。
//!
//! 无关的遗留对象一律不动。历史实现会连带清理它们，但那属于破坏性副作用，收益不足以
//! 抵偿「误删可能是唯一事实源的表」的风险。

use rusqlite::{Connection, Transaction, TransactionBehavior};

/// 一张业务表的对齐规格。
struct TableSpec {
    /// 表名。
    name: &'static str,
    /// 目标建表语句。
    create_sql: &'static str,
    /// 目标索引语句；可重复执行。
    index_sql: &'static str,
    /// 必须存在的全部列。
    required_columns: &'static [&'static str],
    /// 必须是生成列（`GENERATED ALWAYS AS`）的列。
    ///
    /// 只核对列名不够：历史库可能把生成列建成普通列，排序会静默读到空值，因此这里
    /// 逐个核对 `PRAGMA table_xinfo` 的 hidden 标记。
    generated_columns: &'static [&'static str],
    /// 搬迁重建时写入的列，以及该列在旧表缺失时的填充表达式。
    writable_columns: &'static [(&'static str, &'static str)],
}

const CRON_JOBS_CREATE_SQL: &str = "CREATE TABLE cron_jobs (
    job_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    next_run_at_ms INTEGER,
    revision INTEGER NOT NULL,
    created_at_ms INTEGER GENERATED ALWAYS AS
        (json_extract(payload_json, '$.createdAtMs')) VIRTUAL,
    updated_at_ms INTEGER NOT NULL,
    claim_id TEXT,
    claim_until_ms INTEGER
);";

const CRON_JOBS_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_cron_jobs_due ON cron_jobs(status, next_run_at_ms, claim_until_ms);";

const CRON_RUNS_CREATE_SQL: &str = "CREATE TABLE cron_runs (
    run_id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    started_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER NOT NULL
);";

const CRON_RUNS_INDEX_SQL: &str =
    "CREATE INDEX IF NOT EXISTS idx_cron_runs_job ON cron_runs(job_id, started_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_cron_runs_finished_at ON cron_runs(finished_at_ms);";

const CRON_JOBS_SPEC: TableSpec = TableSpec {
    name: "cron_jobs",
    create_sql: CRON_JOBS_CREATE_SQL,
    index_sql: CRON_JOBS_INDEX_SQL,
    required_columns: &[
        "job_id",
        "payload_json",
        "status",
        "next_run_at_ms",
        "revision",
        "created_at_ms",
        "updated_at_ms",
        "claim_id",
        "claim_until_ms",
    ],
    generated_columns: &["created_at_ms"],
    writable_columns: &[
        ("job_id", "''"),
        ("payload_json", "''"),
        ("status", "'active'"),
        ("next_run_at_ms", "NULL"),
        ("revision", "1"),
        ("updated_at_ms", "0"),
        ("claim_id", "NULL"),
        ("claim_until_ms", "NULL"),
    ],
};

const CRON_RUNS_SPEC: TableSpec = TableSpec {
    name: "cron_runs",
    create_sql: CRON_RUNS_CREATE_SQL,
    index_sql: CRON_RUNS_INDEX_SQL,
    required_columns: &[
        "run_id",
        "job_id",
        "payload_json",
        "started_at_ms",
        "finished_at_ms",
    ],
    generated_columns: &[],
    writable_columns: &[
        ("run_id", "''"),
        ("job_id", "''"),
        ("payload_json", "''"),
        ("started_at_ms", "0"),
        ("finished_at_ms", "0"),
    ],
};

const TABLE_SPECS: &[TableSpec] = &[CRON_JOBS_SPEC, CRON_RUNS_SPEC];

/// 把库对齐到当前 schema 版本，保留全部既有行。
pub(super) fn align_schema(connection: &mut Connection, schema_version: i64) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("开始 cron schema 对齐事务失败：{error}"))?;
    for spec in TABLE_SPECS {
        ensure_table(&transaction, spec)?;
    }
    transaction
        .execute_batch(&format!("PRAGMA user_version = {schema_version};"))
        .map_err(|error| format!("对齐 cron schema 版本失败：{error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("提交 cron schema 对齐事务失败：{error}"))
}

/// 建立或修正单张业务表；缺失数据表时直接建成目标形态。
fn ensure_table(transaction: &Transaction<'_>, spec: &TableSpec) -> Result<(), String> {
    if !table_exists(transaction, spec.name)? {
        transaction
            .execute_batch(spec.create_sql)
            .map_err(|error| format!("创建 {} 表失败：{error}", spec.name))?;
        transaction
            .execute_batch(spec.index_sql)
            .map_err(|error| format!("创建 {} 索引失败：{error}", spec.name))?;
        return Ok(());
    }
    let existing = table_columns(transaction, spec.name)?;
    let columns_ready = spec
        .required_columns
        .iter()
        .all(|required| existing.iter().any(|column| column.name == *required));
    let generated_ready = spec.generated_columns.iter().all(|required| {
        existing
            .iter()
            .any(|column| column.name == *required && column.is_generated)
    });
    if columns_ready && generated_ready {
        // 形态已对齐，只需幂等补齐索引：历史库可能缺索引但仍然可用。
        transaction
            .execute_batch(spec.index_sql)
            .map_err(|error| format!("补齐 {} 索引失败：{error}", spec.name))?;
        return Ok(());
    }
    move_rebuild_table(transaction, spec, &existing)
}

/// 按列映射搬迁重建：先改名保留旧行，再建目标表并回填，最后才删除旧表。
///
/// 临时表名带进程号与时间戳：固定名要么需要先 `DROP IF EXISTS`（可能误删用户自建
/// 的同名表），要么在并发对齐时互相踩踏。
fn move_rebuild_table(
    transaction: &Transaction<'_>,
    spec: &TableSpec,
    existing: &[ColumnInfo],
) -> Result<(), String> {
    let legacy = format!(
        "{}_schema_legacy_{}_{}",
        spec.name,
        std::process::id(),
        axon_time::unix_now_ms()
    );
    transaction
        .execute_batch(&format!(
            "ALTER TABLE \"{}\" RENAME TO \"{legacy}\";",
            spec.name
        ))
        .map_err(|error| format!("重命名旧 {} 表失败：{error}", spec.name))?;
    transaction
        .execute_batch(spec.create_sql)
        .map_err(|error| format!("重建 {} 表失败：{error}", spec.name))?;
    let target_columns = spec
        .writable_columns
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let source_values = spec
        .writable_columns
        .iter()
        .map(|(name, fallback)| {
            if existing.iter().any(|column| column.name == *name) {
                format!("\"{legacy}\".\"{name}\"")
            } else {
                (*fallback).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let moved = transaction
        .execute(
            &format!(
                "INSERT INTO \"{}\" ({target_columns}) SELECT {source_values} FROM \"{legacy}\";",
                spec.name
            ),
            [],
        )
        .map_err(|error| format!("回填 {} 历史行失败：{error}", spec.name))?;
    transaction
        .execute_batch(&format!("DROP TABLE \"{legacy}\";"))
        .map_err(|error| format!("清理 {} 旧表失败：{error}", spec.name))?;
    transaction
        .execute_batch(spec.index_sql)
        .map_err(|error| format!("重建 {} 索引失败：{error}", spec.name))?;
    axon_logger::info(&format!(
        "cron schema 对齐：{} 已按列映射重建，回填 {moved} 行",
        spec.name
    ));
    Ok(())
}

fn table_exists(transaction: &Transaction<'_>, name: &str) -> Result<bool, String> {
    let count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 {name} 表是否存在失败：{error}"))?;
    Ok(count > 0)
}

/// 一列的名称与生成列标记。
struct ColumnInfo {
    name: String,
    is_generated: bool,
}

fn table_columns(transaction: &Transaction<'_>, name: &str) -> Result<Vec<ColumnInfo>, String> {
    // `table_xinfo` 比 `table_info` 多一列 hidden：2 与 3 表示生成列。
    let mut statement = transaction
        .prepare(&format!("PRAGMA table_xinfo(\"{name}\")"))
        .map_err(|error| format!("读取 {name} 列定义失败：{error}"))?;
    let columns = statement
        .query_map([], |row| {
            let hidden = row.get::<_, i64>(6)?;
            Ok(ColumnInfo {
                name: row.get::<_, String>(1)?,
                is_generated: hidden == 2 || hidden == 3,
            })
        })
        .map_err(|error| format!("查询 {name} 列定义失败：{error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("解析 {name} 列定义失败：{error}"))?;
    Ok(columns)
}
