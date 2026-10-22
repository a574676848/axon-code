use std::{collections::BTreeSet, path::Path, time::Duration};

use rusqlite::{
    Connection, OpenFlags, OptionalExtension as _, Transaction, params, params_from_iter,
    types::Value,
};

use super::model::{
    UsageFact, UsageProjectionState, UsageRequestView, UsageRollupRow, UsageSourceFilter,
};
pub(crate) mod audit;
pub(crate) mod diagnostics;
#[cfg(test)]
mod m4_m5_tests;
pub(crate) mod minute_rollup;
pub(crate) mod pricing;
pub(crate) mod recalculate;
pub(crate) mod retention;
mod rollup_rebuild;
pub(crate) mod schema;
#[cfg(test)]
mod tests;

pub(super) use rollup_rebuild::{rebuild_day_bucket, rebuild_hour_bucket};

const USAGE_RUNTIME_DIR: &str = "usage";
const USAGE_DATABASE_FILE: &str = "usage.sqlite3";
const USAGE_BUSY_TIMEOUT_MS: u64 = 250;
const SOURCE_BUSY_TIMEOUT_MS: u64 = 50;

pub(crate) struct UsageRepository {
    connection: Connection,
    #[cfg(test)]
    window: Option<std::sync::Arc<axon_domain::SlidingWindowRingBuffer>>,
}

pub(super) struct UsageSourceRow {
    pub(super) source_rowid: u64,
    pub(super) recorded_at_ms: u64,
    pub(super) record_json: String,
}

pub(super) struct UsageQuery<'a> {
    pub(super) period_start_ms: u64,
    pub(super) provider: Option<&'a str>,
    pub(super) model: Option<&'a str>,
    pub(super) source: UsageSourceFilter,
    pub(super) session_id: Option<&'a str>,
    pub(super) run_id: Option<&'a str>,
}

impl UsageRepository {
    pub(crate) fn open(axon_root: &Path) -> Result<Self, String> {
        let runtime_dir = axon_root.join(USAGE_RUNTIME_DIR);
        std::fs::create_dir_all(&runtime_dir)
            .map_err(|error| format!("创建 usage 运行目录失败：{error}"))?;
        let path = runtime_dir.join(USAGE_DATABASE_FILE);
        let connection = Connection::open(&path).map_err(db_error("打开 usage SQLite"))?;
        connection
            .busy_timeout(Duration::from_millis(USAGE_BUSY_TIMEOUT_MS))
            .map_err(db_error("配置 usage SQLite busy timeout"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
            )
            .map_err(db_error("配置 usage SQLite"))?;
        schema::initialize_schema(&connection)?;
        Ok(Self {
            connection,
            #[cfg(test)]
            window: None,
        })
    }

    #[cfg(test)]
    pub(super) fn set_window(
        &mut self,
        window: std::sync::Arc<axon_domain::SlidingWindowRingBuffer>,
    ) {
        self.window = Some(window);
    }

    pub(super) fn projection_state(&self) -> Result<UsageProjectionState, String> {
        self.connection
            .query_row(
                "SELECT source_watermark_rowid, last_success_at_ms, projected_fact_count
                 FROM usage_projection_state WHERE projector = ?1",
                [super::USAGE_PROJECTOR_NAME],
                |row| {
                    Ok(UsageProjectionState {
                        source_watermark_rowid: from_i64(row.get(0)?)?,
                        last_success_at_ms: from_i64_opt(row.get(1)?)?,
                        projected_fact_count: from_i64(row.get(2)?)?,
                    })
                },
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(db_error("读取 usage projector 状态"))
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    pub(crate) fn save_pricing_rule(
        &self,
        rule: &axon_domain::ModelPriceRule,
    ) -> Result<(), String> {
        pricing::save_pricing_rule(&self.connection, rule)
    }

    pub(crate) fn save_pricing_rules(
        &mut self,
        rules: &[axon_domain::ModelPriceRule],
    ) -> Result<(), String> {
        pricing::save_pricing_rules(&mut self.connection, rules)
    }

    #[cfg(test)]
    pub(crate) fn get_pricing_rule(
        &self,
        id: &str,
    ) -> Result<Option<axon_domain::ModelPriceRule>, String> {
        pricing::get_pricing_rule(&self.connection, id)
    }

    pub(crate) fn list_pricing_rules(&self) -> Result<Vec<axon_domain::ModelPriceRule>, String> {
        pricing::list_pricing_rules(&self.connection)
    }

    pub(crate) fn delete_pricing_rule(&self, id: &str) -> Result<bool, String> {
        pricing::delete_pricing_rule(&self.connection, id)
    }

    #[cfg(test)]
    pub(crate) fn delete_all_pricing_rules(&self) -> Result<usize, String> {
        pricing::delete_all_pricing_rules(&self.connection)
    }

    pub(crate) fn rebuild_minute_bucket(&mut self, bucket: u64) -> Result<(), String> {
        let transaction = self
            .connection
            .transaction()
            .map_err(db_error("启动 rebuild_minute_bucket 事务"))?;
        rollup_rebuild::rebuild_minute_bucket(&transaction, bucket)?;
        transaction
            .commit()
            .map_err(db_error("提交 rebuild_minute_bucket 事务"))
    }

    pub(crate) fn recalculate_batch(
        &mut self,
        rules: &[axon_domain::ModelPriceRule],
        cursor: Option<&recalculate::RecalculateCursor>,
        now_ms: u64,
    ) -> Result<recalculate::BatchRecalculateOutcome, String> {
        recalculate::recalculate_batch(&mut self.connection, rules, cursor, now_ms)
    }

    pub(crate) fn query_minute_rollups(
        &self,
        since_ms: u64,
        limit: usize,
    ) -> Result<Vec<minute_rollup::MinuteRollupRow>, String> {
        minute_rollup::query_minute_rollups(&self.connection, since_ms, limit)
    }

    pub(super) fn project_facts(
        &mut self,
        facts: &[UsageFact],
        source_watermark_rowid: u64,
        completed_at_ms: u64,
    ) -> Result<u64, String> {
        let transaction = self
            .connection
            .transaction()
            .map_err(db_error("启动 usage projection 事务"))?;
        let mut touched_hours = BTreeSet::new();
        let mut touched_days = BTreeSet::new();
        let mut changed_facts = 0_u64;
        for fact in facts {
            let changed = upsert_fact(&transaction, fact)?;
            if changed > 0 {
                changed_facts = changed_facts.saturating_add(changed);
                if let Some((hour, day)) = existing_buckets(&transaction, fact)? {
                    touched_hours.insert(hour);
                    touched_days.insert(day);
                }
                touched_hours.insert(fact.hour_bucket_ms);
                touched_days.insert(fact.day_bucket_ms);
                minute_rollup::upsert_minute_rollup(&transaction, fact)?;
                #[cfg(test)]
                if let Some(ref win) = self.window {
                    let delta = super::realtime_aggregator::fact_to_metric_delta(fact);
                    win.record(fact.recorded_at_ms, &delta);
                } else {
                    super::realtime_aggregator::record_fact_to_sliding_window(fact);
                }
                #[cfg(not(test))]
                super::realtime_aggregator::record_fact_to_sliding_window(fact);
            }
        }
        if changed_facts > 0 {
            for bucket in touched_hours {
                rollup_rebuild::rebuild_hour_bucket(&transaction, bucket)?;
            }
            for bucket in touched_days {
                rollup_rebuild::rebuild_day_bucket(&transaction, bucket)?;
            }
        }
        let projected_fact_count = transaction
            .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(db_error("统计 usage facts"))?;
        transaction
            .execute(
                "INSERT INTO usage_projection_state (
                    projector, source_watermark_rowid, last_success_at_ms, projected_fact_count
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(projector) DO UPDATE SET
                    source_watermark_rowid = excluded.source_watermark_rowid,
                    last_success_at_ms = excluded.last_success_at_ms,
                    projected_fact_count = excluded.projected_fact_count",
                params![
                    super::USAGE_PROJECTOR_NAME,
                    to_i64(source_watermark_rowid)?,
                    to_i64(completed_at_ms)?,
                    projected_fact_count,
                ],
            )
            .map_err(db_error("更新 usage projector 状态"))?;
        transaction
            .commit()
            .map_err(db_error("提交 usage projection 事务"))?;
        Ok(changed_facts)
    }

    pub(super) fn rollups(&self, query: &UsageQuery<'_>) -> Result<Vec<UsageRollupRow>, String> {
        if query.session_id.is_some() || query.run_id.is_some() {
            return self
                .fact_rollups(query, false)
                .map(|rows| rows.into_iter().map(|(_, row)| row).collect());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT provider, model, source, status,
                        SUM(request_count), SUM(provider_call_count),
                        SUM(total_input_tokens), SUM(cache_read_input_tokens),
                        SUM(cache_write_input_tokens), SUM(output_tokens),
                        SUM(reasoning_output_tokens), SUM(provider_duration_ms),
                        SUM(measured_latency_count), SUM(priced_cost_micros),
                        SUM(unpriced_request_count)
                 FROM usage_rollup_day
                 WHERE bucket_start_ms >= ?1
                   AND (?2 IS NULL OR provider = ?2)
                   AND (?3 IS NULL OR model = ?3)
                   AND (?4 IS NULL OR source = ?4)
                 GROUP BY provider, model, source, status",
            )
            .map_err(db_error("准备 usage rollup 查询"))?;
        let rows = statement
            .query_map(
                params![
                    to_i64(query.period_start_ms)?,
                    query.provider,
                    query.model,
                    query.source.as_query(),
                ],
                |row| {
                    Ok(UsageRollupRow {
                        provider: row.get(0)?,
                        model: row.get(1)?,
                        source: row.get(2)?,
                        status: row.get(3)?,
                        request_count: from_i64(row.get(4)?)?,
                        provider_call_count: from_i64(row.get(5)?)?,
                        total_input_tokens: from_i64(row.get(6)?)?,
                        cache_read_input_tokens: from_i64(row.get(7)?)?,
                        cache_write_input_tokens: from_i64(row.get(8)?)?,
                        output_tokens: from_i64(row.get(9)?)?,
                        reasoning_output_tokens: from_i64(row.get(10)?)?,
                        provider_duration_ms: from_i64(row.get(11)?)?,
                        measured_latency_count: from_i64(row.get(12)?)?,
                        priced_cost_micros: from_i64(row.get(13)?)?,
                        unpriced_request_count: from_i64(row.get(14)?)?,
                    })
                },
            )
            .map_err(db_error("查询 usage rollup"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 usage rollup"))
    }

    pub(super) fn hourly_rollups(
        &self,
        query: &UsageQuery<'_>,
    ) -> Result<Vec<(u64, UsageRollupRow)>, String> {
        if query.session_id.is_some() || query.run_id.is_some() {
            return self.fact_rollups(query, true);
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT bucket_start_ms, provider, model, source, status,
                        SUM(request_count), SUM(provider_call_count),
                        SUM(total_input_tokens), SUM(cache_read_input_tokens),
                        SUM(cache_write_input_tokens), SUM(output_tokens),
                        SUM(reasoning_output_tokens), SUM(provider_duration_ms),
                        SUM(measured_latency_count), SUM(priced_cost_micros),
                        SUM(unpriced_request_count)
                 FROM usage_rollup_hour
                 WHERE bucket_start_ms >= ?1
                   AND (?2 IS NULL OR provider = ?2)
                   AND (?3 IS NULL OR model = ?3)
                   AND (?4 IS NULL OR source = ?4)
                 GROUP BY bucket_start_ms, provider, model, source, status
                 ORDER BY bucket_start_ms",
            )
            .map_err(db_error("准备 usage 小时趋势查询"))?;
        let rows = statement
            .query_map(
                params![
                    to_i64(query.period_start_ms)?,
                    query.provider,
                    query.model,
                    query.source.as_query(),
                ],
                |row| {
                    Ok((
                        from_i64(row.get(0)?)?,
                        UsageRollupRow {
                            provider: row.get(1)?,
                            model: row.get(2)?,
                            source: row.get(3)?,
                            status: row.get(4)?,
                            request_count: from_i64(row.get(5)?)?,
                            provider_call_count: from_i64(row.get(6)?)?,
                            total_input_tokens: from_i64(row.get(7)?)?,
                            cache_read_input_tokens: from_i64(row.get(8)?)?,
                            cache_write_input_tokens: from_i64(row.get(9)?)?,
                            output_tokens: from_i64(row.get(10)?)?,
                            reasoning_output_tokens: from_i64(row.get(11)?)?,
                            provider_duration_ms: from_i64(row.get(12)?)?,
                            measured_latency_count: from_i64(row.get(13)?)?,
                            priced_cost_micros: from_i64(row.get(14)?)?,
                            unpriced_request_count: from_i64(row.get(15)?)?,
                        },
                    ))
                },
            )
            .map_err(db_error("查询 usage 小时趋势"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 usage 小时趋势"))
    }

    pub(super) fn recent_requests(
        &self,
        query: &UsageQuery<'_>,
        limit: usize,
    ) -> Result<Vec<UsageRequestView>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT recorded_at_ms, session_id, assistant_part_id, provider, model,
                        source, status, total_input_tokens, cache_read_input_tokens,
                        cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                        provider_calls, provider_duration_ms, cost_micros
                 FROM usage_facts
                 WHERE recorded_at_ms >= ?1
                   AND (?2 IS NULL OR provider = ?2)
                   AND (?3 IS NULL OR model = ?3)
                   AND (?4 IS NULL OR source = ?4)
                   AND (?5 IS NULL OR session_id = ?5)
                   AND (?6 IS NULL OR assistant_part_id = ?6)
                 ORDER BY recorded_at_ms DESC, session_id, assistant_part_id
                 LIMIT ?7",
            )
            .map_err(db_error("准备 usage 最近请求查询"))?;
        let rows = statement
            .query_map(
                params![
                    to_i64(query.period_start_ms)?,
                    query.provider,
                    query.model,
                    query.source.as_query(),
                    query.session_id,
                    query.run_id,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ],
                |row| {
                    let total_input_tokens = from_i64(row.get(7)?)?;
                    let cache_read_input_tokens = from_i64(row.get(8)?)?;
                    let cache_write_input_tokens = from_i64(row.get(9)?)?;
                    let output_tokens = from_i64(row.get(10)?)?;
                    let reasoning_output_tokens = from_i64(row.get(11)?)?;
                    Ok(UsageRequestView {
                        recorded_at_ms: from_i64(row.get(0)?)?,
                        session_id: row.get(1)?,
                        assistant_part_id: row.get(2)?,
                        provider: row.get(3)?,
                        model: row.get(4)?,
                        source: row.get(5)?,
                        status: row.get(6)?,
                        total_input_tokens,
                        cache_read_input_tokens,
                        cache_write_input_tokens,
                        output_tokens,
                        reasoning_output_tokens,
                        total_tokens: total_input_tokens
                            .saturating_add(output_tokens)
                            .saturating_add(reasoning_output_tokens),
                        provider_calls: from_i64(row.get(12)?)?,
                        provider_duration_ms: from_i64(row.get(13)?)?,
                        cost_micros: from_i64_opt(row.get(14)?)?,
                    })
                },
            )
            .map_err(db_error("查询 usage 最近请求"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 usage 最近请求"))
    }

    fn fact_rollups(
        &self,
        query: &UsageQuery<'_>,
        hourly: bool,
    ) -> Result<Vec<(u64, UsageRollupRow)>, String> {
        let bucket = if hourly { "hour_bucket_ms" } else { "0" };
        let group_bucket = if hourly { ", hour_bucket_ms" } else { "" };
        let order_by = if hourly {
            "hour_bucket_ms, provider, model, source, status"
        } else {
            "provider, model, source, status"
        };
        let sql = format!(
            "SELECT {bucket}, provider, model, source, status,
                    COUNT(*), COALESCE(SUM(provider_calls), 0),
                    COALESCE(SUM(total_input_tokens), 0), COALESCE(SUM(cache_read_input_tokens), 0),
                    COALESCE(SUM(cache_write_input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(reasoning_output_tokens), 0), COALESCE(SUM(provider_duration_ms), 0),
                    COALESCE(SUM(CASE WHEN provider_duration_ms > 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(COALESCE(cost_micros, 0)), 0),
                    COALESCE(SUM(CASE WHEN cost_micros IS NULL THEN 1 ELSE 0 END), 0)
             FROM usage_facts
             WHERE recorded_at_ms >= ?1
               AND (?2 IS NULL OR provider = ?2)
               AND (?3 IS NULL OR model = ?3)
               AND (?4 IS NULL OR source = ?4)
               AND (?5 IS NULL OR session_id = ?5)
               AND (?6 IS NULL OR assistant_part_id = ?6)
             GROUP BY provider, model, source, status{group_bucket}
             ORDER BY {order_by}"
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(db_error("准备 usage session/run 聚合查询"))?;
        let rows = statement
            .query_map(
                params![
                    to_i64(query.period_start_ms)?,
                    query.provider,
                    query.model,
                    query.source.as_query(),
                    query.session_id,
                    query.run_id,
                ],
                parse_rollup_row,
            )
            .map_err(db_error("查询 usage session/run 聚合"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error("解析 usage session/run 聚合"))
    }
}

fn parse_rollup_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u64, UsageRollupRow)> {
    Ok((
        from_i64(row.get(0)?)?,
        UsageRollupRow {
            provider: row.get(1)?,
            model: row.get(2)?,
            source: row.get(3)?,
            status: row.get(4)?,
            request_count: from_i64(row.get(5)?)?,
            provider_call_count: from_i64(row.get(6)?)?,
            total_input_tokens: from_i64(row.get(7)?)?,
            cache_read_input_tokens: from_i64(row.get(8)?)?,
            cache_write_input_tokens: from_i64(row.get(9)?)?,
            output_tokens: from_i64(row.get(10)?)?,
            reasoning_output_tokens: from_i64(row.get(11)?)?,
            provider_duration_ms: from_i64(row.get(12)?)?,
            measured_latency_count: from_i64(row.get(13)?)?,
            priced_cost_micros: from_i64(row.get(14)?)?,
            unpriced_request_count: from_i64(row.get(15)?)?,
        },
    ))
}

pub(super) fn read_rollout_usage_rows(
    rollout_path: &Path,
    watermark_rowid: u64,
    reconcile_keys: &[(String, String)],
) -> Result<Vec<UsageSourceRow>, String> {
    if !rollout_path.exists() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(
        rollout_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error("只读打开 rollout SQLite"))?;
    connection
        .busy_timeout(Duration::from_millis(SOURCE_BUSY_TIMEOUT_MS))
        .map_err(db_error("配置 rollout 只读 busy timeout"))?;
    let mut where_clauses = vec!["rowid > ?1".to_owned()];
    let mut values = vec![Value::Integer(to_i64(watermark_rowid)?)];
    for (index, (session_id, assistant_part_id)) in reconcile_keys.iter().enumerate() {
        let session_placeholder = index.saturating_mul(2).saturating_add(2);
        let part_placeholder = session_placeholder.saturating_add(1);
        where_clauses.push(format!(
            "(session_id = ?{session_placeholder} AND assistant_part_id = ?{part_placeholder})"
        ));
        values.push(Value::Text(session_id.clone()));
        values.push(Value::Text(assistant_part_id.clone()));
    }
    let sql = format!(
        "SELECT rowid, recorded_at_ms, record_json
         FROM rollout_turns
         WHERE {}
         ORDER BY rowid",
        where_clauses.join(" OR ")
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(db_error("准备 rollout usage 增量查询"))?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok(UsageSourceRow {
                source_rowid: from_i64(row.get(0)?)?,
                recorded_at_ms: from_i64(row.get(1)?)?,
                record_json: row.get(2)?,
            })
        })
        .map_err(db_error("查询 rollout usage 增量"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error("解析 rollout usage 增量"))
}

fn existing_buckets(
    transaction: &Transaction<'_>,
    fact: &UsageFact,
) -> Result<Option<(u64, u64)>, String> {
    transaction
        .query_row(
            "SELECT hour_bucket_ms, day_bucket_ms FROM usage_facts
             WHERE session_id = ?1 AND assistant_part_id = ?2",
            params![fact.session_id, fact.assistant_part_id],
            |row| Ok((from_i64(row.get(0)?)?, from_i64(row.get(1)?)?)),
        )
        .optional()
        .map_err(db_error("读取 usage fact 旧 bucket"))
}

fn upsert_fact(transaction: &Transaction<'_>, fact: &UsageFact) -> Result<u64, String> {
    let changed = transaction
        .execute(
            "INSERT INTO usage_facts (
                session_id, assistant_part_id, recorded_at_ms, hour_bucket_ms, day_bucket_ms,
                provider, model, source, status, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_calls, provider_duration_ms, usage_observed, cost_micros,
                source_fingerprint, upstream_model, reasoning_effort, request_path,
                speed_mode, ttft_ms, generation_speed_tps, pricing_rule_id, recalculated_at_ms
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27
             )
             ON CONFLICT(session_id, assistant_part_id) DO UPDATE SET
                recorded_at_ms = excluded.recorded_at_ms,
                hour_bucket_ms = excluded.hour_bucket_ms,
                day_bucket_ms = excluded.day_bucket_ms,
                provider = excluded.provider,
                model = excluded.model,
                source = excluded.source,
                status = excluded.status,
                total_input_tokens = excluded.total_input_tokens,
                cache_read_input_tokens = excluded.cache_read_input_tokens,
                cache_write_input_tokens = excluded.cache_write_input_tokens,
                output_tokens = excluded.output_tokens,
                reasoning_output_tokens = excluded.reasoning_output_tokens,
                provider_calls = excluded.provider_calls,
                provider_duration_ms = excluded.provider_duration_ms,
                usage_observed = excluded.usage_observed,
                cost_micros = excluded.cost_micros,
                source_fingerprint = excluded.source_fingerprint,
                upstream_model = excluded.upstream_model,
                reasoning_effort = excluded.reasoning_effort,
                request_path = excluded.request_path,
                speed_mode = excluded.speed_mode,
                ttft_ms = excluded.ttft_ms,
                generation_speed_tps = excluded.generation_speed_tps,
                pricing_rule_id = excluded.pricing_rule_id,
                recalculated_at_ms = excluded.recalculated_at_ms
             WHERE usage_facts.source_fingerprint <> excluded.source_fingerprint",
            params![
                fact.session_id,
                fact.assistant_part_id,
                to_i64(fact.recorded_at_ms)?,
                to_i64(fact.hour_bucket_ms)?,
                to_i64(fact.day_bucket_ms)?,
                fact.provider,
                fact.model,
                fact.source.as_str(),
                fact.status.as_str(),
                to_i64(fact.total_input_tokens)?,
                to_i64(fact.cache_read_input_tokens)?,
                to_i64(fact.cache_write_input_tokens)?,
                to_i64(fact.output_tokens)?,
                to_i64(fact.reasoning_output_tokens)?,
                to_i64(fact.provider_calls)?,
                to_i64(fact.provider_duration_ms)?,
                i64::from(fact.usage_observed),
                fact.cost_micros.map(to_i64).transpose()?,
                fact.source_fingerprint,
                fact.upstream_model,
                fact.reasoning_effort,
                fact.request_path,
                fact.speed_mode,
                fact.ttft_ms.map(to_i64).transpose()?,
                fact.generation_speed_tps,
                fact.pricing_rule_id,
                fact.recalculated_at_ms.map(to_i64).transpose()?,
            ],
        )
        .map_err(db_error("写入 usage fact"))?;
    u64::try_from(changed).map_err(|_| "usage fact 变更数超出范围".to_owned())
}

fn to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "usage 数值超出 SQLite INTEGER 范围".to_owned())
}

fn from_i64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

fn from_i64_opt(value: Option<i64>) -> rusqlite::Result<Option<u64>> {
    value.map(from_i64).transpose()
}

pub(super) fn db_error(context: &'static str) -> impl FnOnce(rusqlite::Error) -> String {
    move |error| format!("{context}失败：{error}")
}
