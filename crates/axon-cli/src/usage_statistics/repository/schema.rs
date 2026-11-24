use rusqlite::Connection;

use super::db_error;

pub(crate) const USAGE_DATABASE_SCHEMA_VERSION: i64 = 6;

pub(super) fn initialize_schema(connection: &Connection) -> Result<(), String> {
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(db_error("读取 usage schema version"))?;

    if version == USAGE_DATABASE_SCHEMA_VERSION {
        return Ok(());
    }

    if version == 3 {
        // M1 平滑 ALTER TABLE 迁移：6 列新增 + 复合索引替换，严禁走清库 DROP 分支
        migrate_v3_to_v4(connection)?;
        migrate_v4_to_v5(connection)?;
        migrate_v5_to_v6(connection)?;
        connection
            .pragma_update(None, "user_version", USAGE_DATABASE_SCHEMA_VERSION)
            .map_err(db_error("写入 usage schema version"))?;
        return Ok(());
    }

    if version == 4 {
        // M2 平滑 ALTER TABLE 迁移：pricing_rules 表与两列新增，严禁走清库 DROP 分支
        migrate_v4_to_v5(connection)?;
        migrate_v5_to_v6(connection)?;
        connection
            .pragma_update(None, "user_version", USAGE_DATABASE_SCHEMA_VERSION)
            .map_err(db_error("写入 usage schema version"))?;
        return Ok(());
    }

    if version == 5 {
        // M3 平滑迁移：usage_rollup_minute 表与分钟索引，严禁走清库 DROP 分支
        migrate_v5_to_v6(connection)?;
        connection
            .pragma_update(None, "user_version", USAGE_DATABASE_SCHEMA_VERSION)
            .map_err(db_error("写入 usage schema version"))?;
        return Ok(());
    }

    if version != 0 {
        // 未知历史旧版本，执行兼容清理
        connection
            .execute_batch(
                "DROP TABLE IF EXISTS usage_rollup_minute;
                 DROP TABLE IF EXISTS usage_rollup_hour;
                 DROP TABLE IF EXISTS usage_rollup_day;
                 DROP TABLE IF EXISTS usage_facts;
                 DROP TABLE IF EXISTS usage_projection_state;
                 DROP TABLE IF EXISTS pricing_rules;",
            )
            .map_err(db_error("清理旧 usage schema"))?;
    }

    create_schema_v6(connection)?;
    connection
        .pragma_update(None, "user_version", USAGE_DATABASE_SCHEMA_VERSION)
        .map_err(db_error("写入 usage schema version"))
}

fn migrate_v3_to_v4(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "ALTER TABLE usage_facts ADD COLUMN upstream_model TEXT DEFAULT NULL;
             ALTER TABLE usage_facts ADD COLUMN reasoning_effort TEXT DEFAULT NULL;
             ALTER TABLE usage_facts ADD COLUMN request_path TEXT DEFAULT NULL;
             ALTER TABLE usage_facts ADD COLUMN speed_mode TEXT DEFAULT NULL;
             ALTER TABLE usage_facts ADD COLUMN ttft_ms INTEGER DEFAULT NULL;
             ALTER TABLE usage_facts ADD COLUMN generation_speed_tps REAL DEFAULT NULL;
             DROP INDEX IF EXISTS idx_usage_facts_model_time;
             DROP INDEX IF EXISTS idx_usage_facts_status_time;
             CREATE INDEX IF NOT EXISTS idx_usage_facts_model_status_time
                ON usage_facts(model, status, recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_audit_query
                ON usage_facts(model, status, source, recorded_at_ms DESC);",
        )
        .map_err(db_error("平滑迁移 usage schema 3 -> 4"))
}

fn migrate_v4_to_v5(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "ALTER TABLE usage_facts ADD COLUMN pricing_rule_id TEXT DEFAULT NULL;
             ALTER TABLE usage_facts ADD COLUMN recalculated_at_ms INTEGER DEFAULT NULL;
             CREATE TABLE IF NOT EXISTS pricing_rules (
                id TEXT PRIMARY KEY NOT NULL,
                model_pattern TEXT NOT NULL,
                pricing_style TEXT NOT NULL,
                input_price_per_m_micros INTEGER NOT NULL,
                output_price_per_m_micros INTEGER NOT NULL,
                cache_read_price_per_m_micros INTEGER NOT NULL,
                cache_write_price_per_m_micros INTEGER NOT NULL,
                multiplier_basis_points INTEGER NOT NULL DEFAULT 10000,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_pricing_pattern
                ON pricing_rules(model_pattern, priority DESC);",
        )
        .map_err(db_error("平滑迁移 usage schema 4 -> 5"))
}

fn migrate_v5_to_v6(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_rollup_minute (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                ttft_duration_ms INTEGER NOT NULL,
                measured_ttft_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE INDEX IF NOT EXISTS idx_rollup_min_bucket
                ON usage_rollup_minute(bucket_start_ms);",
        )
        .map_err(db_error("平滑迁移 usage schema 5 -> 6"))
}

#[cfg(test)]
fn create_schema_v4(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_facts (
                session_id TEXT NOT NULL,
                assistant_part_id TEXT NOT NULL,
                recorded_at_ms INTEGER NOT NULL,
                hour_bucket_ms INTEGER NOT NULL,
                day_bucket_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_calls INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                usage_observed INTEGER NOT NULL,
                cost_micros INTEGER,
                source_fingerprint TEXT NOT NULL,
                upstream_model TEXT DEFAULT NULL,
                reasoning_effort TEXT DEFAULT NULL,
                request_path TEXT DEFAULT NULL,
                speed_mode TEXT DEFAULT NULL,
                ttft_ms INTEGER DEFAULT NULL,
                generation_speed_tps REAL DEFAULT NULL,
                PRIMARY KEY (session_id, assistant_part_id)
             );
             CREATE INDEX IF NOT EXISTS idx_usage_facts_time
                ON usage_facts(recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_provider_time
                ON usage_facts(provider, recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_model_status_time
                ON usage_facts(model, status, recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_audit_query
                ON usage_facts(model, status, source, recorded_at_ms DESC);
             CREATE TABLE IF NOT EXISTS usage_rollup_hour (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                provider_call_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                measured_latency_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE TABLE IF NOT EXISTS usage_rollup_day (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                provider_call_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                measured_latency_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE TABLE IF NOT EXISTS usage_projection_state (
                projector TEXT PRIMARY KEY,
                source_watermark_rowid INTEGER NOT NULL,
                last_success_at_ms INTEGER,
                projected_fact_count INTEGER NOT NULL
             );",
        )
        .map_err(db_error("初始化 usage schema v4"))
}

#[cfg(test)]
fn create_schema_v5(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_facts (
                session_id TEXT NOT NULL,
                assistant_part_id TEXT NOT NULL,
                recorded_at_ms INTEGER NOT NULL,
                hour_bucket_ms INTEGER NOT NULL,
                day_bucket_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_calls INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                usage_observed INTEGER NOT NULL,
                cost_micros INTEGER,
                source_fingerprint TEXT NOT NULL,
                upstream_model TEXT DEFAULT NULL,
                reasoning_effort TEXT DEFAULT NULL,
                request_path TEXT DEFAULT NULL,
                speed_mode TEXT DEFAULT NULL,
                ttft_ms INTEGER DEFAULT NULL,
                generation_speed_tps REAL DEFAULT NULL,
                pricing_rule_id TEXT DEFAULT NULL,
                recalculated_at_ms INTEGER DEFAULT NULL,
                PRIMARY KEY (session_id, assistant_part_id)
             );
             CREATE INDEX IF NOT EXISTS idx_usage_facts_time
                ON usage_facts(recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_provider_time
                ON usage_facts(provider, recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_model_status_time
                ON usage_facts(model, status, recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_audit_query
                ON usage_facts(model, status, source, recorded_at_ms DESC);
             CREATE TABLE IF NOT EXISTS usage_rollup_hour (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                provider_call_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                measured_latency_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE TABLE IF NOT EXISTS usage_rollup_day (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                provider_call_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                measured_latency_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE TABLE IF NOT EXISTS usage_projection_state (
                projector TEXT PRIMARY KEY,
                source_watermark_rowid INTEGER NOT NULL,
                last_success_at_ms INTEGER,
                projected_fact_count INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pricing_rules (
                id TEXT PRIMARY KEY NOT NULL,
                model_pattern TEXT NOT NULL,
                pricing_style TEXT NOT NULL,
                input_price_per_m_micros INTEGER NOT NULL,
                output_price_per_m_micros INTEGER NOT NULL,
                cache_read_price_per_m_micros INTEGER NOT NULL,
                cache_write_price_per_m_micros INTEGER NOT NULL,
                multiplier_basis_points INTEGER NOT NULL DEFAULT 10000,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_pricing_pattern
                ON pricing_rules(model_pattern, priority DESC);",
        )
        .map_err(db_error("初始化 usage schema v5"))
}

fn create_schema_v6(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_facts (
                session_id TEXT NOT NULL,
                assistant_part_id TEXT NOT NULL,
                recorded_at_ms INTEGER NOT NULL,
                hour_bucket_ms INTEGER NOT NULL,
                day_bucket_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_calls INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                usage_observed INTEGER NOT NULL,
                cost_micros INTEGER,
                source_fingerprint TEXT NOT NULL,
                upstream_model TEXT DEFAULT NULL,
                reasoning_effort TEXT DEFAULT NULL,
                request_path TEXT DEFAULT NULL,
                speed_mode TEXT DEFAULT NULL,
                ttft_ms INTEGER DEFAULT NULL,
                generation_speed_tps REAL DEFAULT NULL,
                pricing_rule_id TEXT DEFAULT NULL,
                recalculated_at_ms INTEGER DEFAULT NULL,
                PRIMARY KEY (session_id, assistant_part_id)
             );
             CREATE INDEX IF NOT EXISTS idx_usage_facts_time
                ON usage_facts(recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_provider_time
                ON usage_facts(provider, recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_model_status_time
                ON usage_facts(model, status, recorded_at_ms DESC);
             CREATE INDEX IF NOT EXISTS idx_usage_facts_audit_query
                ON usage_facts(model, status, source, recorded_at_ms DESC);
             CREATE TABLE IF NOT EXISTS usage_rollup_minute (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                ttft_duration_ms INTEGER NOT NULL,
                measured_ttft_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE INDEX IF NOT EXISTS idx_rollup_min_bucket
                ON usage_rollup_minute(bucket_start_ms);
             CREATE TABLE IF NOT EXISTS usage_rollup_hour (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                provider_call_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                measured_latency_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE TABLE IF NOT EXISTS usage_rollup_day (
                bucket_start_ms INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                request_count INTEGER NOT NULL,
                provider_call_count INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL,
                cache_read_input_tokens INTEGER NOT NULL,
                cache_write_input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL,
                provider_duration_ms INTEGER NOT NULL,
                measured_latency_count INTEGER NOT NULL,
                priced_cost_micros INTEGER NOT NULL,
                unpriced_request_count INTEGER NOT NULL,
                PRIMARY KEY (bucket_start_ms, provider, model, source, status)
             );
             CREATE TABLE IF NOT EXISTS usage_projection_state (
                projector TEXT PRIMARY KEY,
                source_watermark_rowid INTEGER NOT NULL,
                last_success_at_ms INTEGER,
                projected_fact_count INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pricing_rules (
                id TEXT PRIMARY KEY NOT NULL,
                model_pattern TEXT NOT NULL,
                pricing_style TEXT NOT NULL,
                input_price_per_m_micros INTEGER NOT NULL,
                output_price_per_m_micros INTEGER NOT NULL,
                cache_read_price_per_m_micros INTEGER NOT NULL,
                cache_write_price_per_m_micros INTEGER NOT NULL,
                multiplier_basis_points INTEGER NOT NULL DEFAULT 10000,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_pricing_pattern
                ON pricing_rules(model_pattern, priority DESC);",
        )
        .map_err(db_error("初始化 usage schema v6"))
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod schema_tests;
