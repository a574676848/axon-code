//! 会话级累计用量：按会话把账本里每轮那一行的用量加总。
//!
//! 事实源是 `rollout_turns`：每轮收口时都写下该轮 provider 实测的用量，因此把同一会话
//! 各轮相加就等于整条会话截至当前的消耗。界面据此不必回放正文，也不必先知道这条会话
//! 一共多少轮、当前翻到第几页。
//!
//! **性能**：汇总只读用量那一组整数列，不触碰 `record_json`。账本里每行的 JSON 都含着
//! prompt、response、工具回执与注入上下文，逐行反序列化会把一次指标刷新变成几 MB 的读放大；
//! 整数列配合覆盖索引则是每行几十个字节，翻十几次页也还是同一条 SQL。

use rusqlite::{Connection, params};

use super::RolloutSqliteStore;

/// 每轮用量列的列名与取值：会话累计读数按这一组加总。
///
/// 顺序即建列顺序；`usage_tokens` 是总量，其余是它的分桶。
const USAGE_COLUMNS: &[(&str, &str)] = &[
    ("usage_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("usage_input_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("usage_cache_read_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("usage_cache_write_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("usage_output_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ("usage_reasoning_tokens", "INTEGER NOT NULL DEFAULT 0"),
];

/// 覆盖索引：按会话汇总只读索引页，顺序扫一批整数。
const USAGE_INDEX: &str = "idx_rollout_turn_session_usage";
/// 上一轮实现留下的窄索引；不删掉就白占空间，而且与本条口径不同。
const LEGACY_USAGE_INDEX: &str = "idx_rollout_turn_usage";

/// 一条会话的累计用量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SessionUsageTotals {
    /// 总量。
    pub(crate) total_tokens: u64,
    /// 未缓存输入。
    pub(crate) input_tokens: u64,
    /// 缓存读取。
    pub(crate) cache_read_tokens: u64,
    /// 缓存写入。
    pub(crate) cache_write_tokens: u64,
    /// 输出。
    pub(crate) output_tokens: u64,
    /// 推理输出。
    pub(crate) reasoning_tokens: u64,
}

impl RolloutSqliteStore {
    /// 汇总一条会话截至当前的累计用量。
    ///
    /// 同一轮的起始记录与收口记录共用一行（按 `assistant_part_id` 唯一），因此按行相加
    /// 不会把一轮算两次；还没收到用量的轮次贡献 0。
    pub(crate) fn session_usage_totals(
        &self,
        session_id: &str,
    ) -> Result<SessionUsageTotals, String> {
        self.connection
            .query_row(
                "SELECT COALESCE(SUM(usage_tokens), 0), COALESCE(SUM(usage_input_tokens), 0),
                        COALESCE(SUM(usage_cache_read_tokens), 0),
                        COALESCE(SUM(usage_cache_write_tokens), 0),
                        COALESCE(SUM(usage_output_tokens), 0),
                        COALESCE(SUM(usage_reasoning_tokens), 0)
                 FROM rollout_turns WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(|_| "rollout 会话用量汇总失败".to_owned())
            .and_then(|values| {
                Ok(SessionUsageTotals {
                    total_tokens: non_negative(values.0)?,
                    input_tokens: non_negative(values.1)?,
                    cache_read_tokens: non_negative(values.2)?,
                    cache_write_tokens: non_negative(values.3)?,
                    output_tokens: non_negative(values.4)?,
                    reasoning_tokens: non_negative(values.5)?,
                })
            })
    }
}

fn non_negative(value: i64) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| "rollout 会话用量不能为负数".to_owned())
}

/// 建好用量列与覆盖索引；旧库缺列时补列并把已有记录回填一次。
///
/// 不推 schema 版本：版本号一变，账本会按既有约定整表重建，历史会话会一起丢掉。
/// 这里用 `ALTER TABLE ADD COLUMN` 加列，回填只发生在刚补列的那一次。
pub(super) fn ensure_usage_columns(connection: &Connection) -> Result<(), String> {
    let mut added = false;
    for (column, definition) in USAGE_COLUMNS {
        if column_exists(connection, "rollout_turns", column)? {
            continue;
        }
        connection
            .execute(
                &format!("ALTER TABLE rollout_turns ADD COLUMN {column} {definition}"),
                [],
            )
            .map_err(|_| "rollout 会话用量列添加失败".to_owned())?;
        added = true;
    }
    if added {
        backfill_usage_columns(connection)?;
    }
    connection
        .execute_batch(&format!("DROP INDEX IF EXISTS {LEGACY_USAGE_INDEX};"))
        .map_err(|_| "rollout 旧会话用量索引清理失败".to_owned())?;
    let columns = USAGE_COLUMNS
        .iter()
        .map(|(column, _)| *column)
        .collect::<Vec<_>>()
        .join(", ");
    connection
        .execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS {USAGE_INDEX} ON rollout_turns(session_id, {columns})"
        ))
        .map_err(|_| "rollout 会话用量索引创建失败".to_owned())
}

/// 表里是否已有这一列。
fn column_exists(connection: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|_| "rollout 表结构查询准备失败".to_owned())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| "rollout 表结构查询失败".to_owned())?;
    for name in columns {
        if name.map_err(|_| "rollout 表结构列读取失败".to_owned())? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 补列之后把历史记录逐行换算一次（只做一次，之后由写入路径维护）。
fn backfill_usage_columns(connection: &Connection) -> Result<(), String> {
    let pending = {
        let mut statement = connection
            .prepare("SELECT rowid, record_json FROM rollout_turns")
            .map_err(|_| "rollout 用量回填查询准备失败".to_owned())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| "rollout 用量回填读取失败".to_owned())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| "rollout 用量回填行解析失败".to_owned())?
    };
    let mut update = connection
        .prepare(
            "UPDATE rollout_turns SET usage_tokens = ?1, usage_input_tokens = ?2,
                    usage_cache_read_tokens = ?3, usage_cache_write_tokens = ?4,
                    usage_output_tokens = ?5, usage_reasoning_tokens = ?6
             WHERE rowid = ?7",
        )
        .map_err(|_| "rollout 用量回填更新准备失败".to_owned())?;
    for (rowid, record_json) in pending {
        let usage = rollout_turn_usage(&record_json);
        update
            .execute(params![
                to_i64(usage.total_tokens()),
                to_i64(usage.input_tokens),
                to_i64(usage.cache_read_input_tokens),
                to_i64(usage.cache_write_input_tokens),
                to_i64(usage.output_tokens),
                to_i64(usage.reasoning_output_tokens),
                rowid,
            ])
            .map_err(|_| "rollout 用量回填更新失败".to_owned())?;
    }
    Ok(())
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// 一轮落进用量列的整数集合。
///
/// 只声明用得到的最小反序列化合同：账本里另有几十个字段，读数不该因为某个不相关字段
/// 改型就整段失效。用量缺失（起始记录、失败的轮次）读作全零。
pub(super) fn rollout_turn_usage(record_json: &str) -> axon_agent_runtime::TokenUsageTelemetry {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct UsageOnly {
        #[serde(default)]
        usage: Option<axon_agent_runtime::TokenUsageTelemetry>,
    }

    serde_json::from_str::<UsageOnly>(record_json)
        .ok()
        .and_then(|parsed| parsed.usage)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use axon_agent_runtime::TokenUsageTelemetry;

    use super::{RolloutSqliteStore, rollout_turn_usage};
    use crate::run_commands::run_rollout_store::tests::{cleanup, record, temp_db};

    #[test]
    fn turn_record_usage_carries_every_bucket() {
        let record = serde_json::json!({
            "sessionId": "session-lane-1",
            "provider": "openai",
            "model": "gpt-5",
            "assistantPartId": "assistant-1",
            "usage": {
                "inputTokens": 800,
                "cacheReadInputTokens": 200,
                "cacheWriteInputTokens": 50,
                "outputTokens": 100,
                "reasoningOutputTokens": 25,
                "totalInputTokens": 1050
            }
        });

        let usage = rollout_turn_usage(&record.to_string());
        assert_eq!(usage.total_tokens(), 1_175);
        assert_eq!(usage.input_tokens, 800);
        assert_eq!(usage.cache_read_input_tokens, 200);
        assert_eq!(usage.cache_write_input_tokens, 50);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.reasoning_output_tokens, 25);
    }

    #[test]
    fn turn_record_without_usage_counts_as_zero() {
        let record = serde_json::json!({
            "sessionId": "session-lane-1",
            "provider": "openai",
            "model": "gpt-5",
            "assistantPartId": "assistant-1",
            "usage": null
        });

        assert_eq!(rollout_turn_usage(&record.to_string()).total_tokens(), 0);
        assert_eq!(rollout_turn_usage("{}").total_tokens(), 0);
    }

    #[test]
    fn session_usage_totals_sum_every_turn_of_that_session_only() {
        let path = temp_db("session-usage-total");
        let store = RolloutSqliteStore::open(&path).unwrap();

        let mut first = record("lane-1", None, 100);
        first.usage = Some(TokenUsageTelemetry {
            total_input_tokens: 1_000,
            input_tokens: 400,
            cache_read_input_tokens: 600,
            output_tokens: 50,
            ..TokenUsageTelemetry::default()
        });
        let mut second = record("lane-1", None, 200);
        second.assistant_part_id = "assistant-followup".to_owned();
        second.usage = Some(TokenUsageTelemetry {
            total_input_tokens: 2_000,
            input_tokens: 2_000,
            reasoning_output_tokens: 70,
            ..TokenUsageTelemetry::default()
        });
        let mut other = record("lane-2", None, 300);
        other.usage = Some(TokenUsageTelemetry {
            output_tokens: 999,
            ..TokenUsageTelemetry::default()
        });
        for turn in [&first, &second, &other] {
            store.record_session(turn).unwrap();
        }

        let totals = store.session_usage_totals("lane-1").unwrap();
        assert_eq!(totals.total_tokens, 3_120);
        assert_eq!(totals.input_tokens, 2_400);
        assert_eq!(totals.cache_read_tokens, 600);
        assert_eq!(totals.cache_write_tokens, 0);
        assert_eq!(totals.output_tokens, 50);
        assert_eq!(totals.reasoning_tokens, 70);

        assert_eq!(
            store.session_usage_totals("lane-2").unwrap().total_tokens,
            999
        );
        assert_eq!(
            store.session_usage_totals("lane-without-usage").unwrap(),
            super::SessionUsageTotals::default()
        );
        cleanup(&path);
    }
}
