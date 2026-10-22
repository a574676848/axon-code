use super::*;

const V3_FACTS_SCHEMA: &str = "
    CREATE TABLE usage_facts (
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
        PRIMARY KEY (session_id, assistant_part_id)
    );
    CREATE INDEX idx_usage_facts_time ON usage_facts(recorded_at_ms DESC);
    CREATE INDEX idx_usage_facts_provider_time ON usage_facts(provider, recorded_at_ms DESC);
    CREATE INDEX idx_usage_facts_model_time ON usage_facts(model, recorded_at_ms DESC);
    CREATE INDEX idx_usage_facts_status_time ON usage_facts(status, recorded_at_ms ASC);
    CREATE TABLE usage_rollup_hour (
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
    CREATE TABLE usage_rollup_day (
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
    CREATE TABLE usage_projection_state (
        projector TEXT PRIMARY KEY,
        source_watermark_rowid INTEGER NOT NULL,
        last_success_at_ms INTEGER,
        projected_fact_count INTEGER NOT NULL
    );";

#[test]
fn schema_v3_to_v6_migration_preserves_data_without_drop() {
    let connection = Connection::open_in_memory().unwrap();
    // 1. 初始化既有 v3 库并写入一条历史事实数据
    connection.execute_batch(V3_FACTS_SCHEMA).unwrap();
    connection.pragma_update(None, "user_version", 3).unwrap();
    connection
        .execute(
            "INSERT INTO usage_facts (
                session_id, assistant_part_id, recorded_at_ms, hour_bucket_ms, day_bucket_ms,
                provider, model, source, status, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_calls, provider_duration_ms, usage_observed, cost_micros,
                source_fingerprint
             ) VALUES (
                'sess-v3', 'part-v3', 1000, 1000, 1000,
                'prov-v3', 'mod-v3', 'axon_cli', 'completed', 100, 0,
                0, 50, 0, 1, 300, 1, NULL, 'fp-v3'
             )",
            [],
        )
        .unwrap();

    let count_before: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_before, 1, "迁移前既有数据行数为 1");

    // 2. 执行平滑迁移升级
    initialize_schema(&connection).expect("平滑迁移应成功");

    // 3. 校验 user_version 升至 6
    let version_after: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version_after, 6, "PRAGMA user_version 必须为 6");

    // 4. 校验既有数据零丢失（未走 DROP 分支）
    let count_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_after, 1, "迁移后既有数据行数未变（零丢失）");

    // 5. 校验新增的 6 个列全部存在且旧行值为 NULL
    type TelemetryFactColumns = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<f64>,
    );
    let (upstream_model, reasoning_effort, request_path, speed_mode, ttft_ms, generation_speed): TelemetryFactColumns = connection
        .query_row(
            "SELECT upstream_model, reasoning_effort, request_path, speed_mode, ttft_ms, generation_speed_tps
             FROM usage_facts WHERE session_id = 'sess-v3'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert!(upstream_model.is_none());
    assert!(reasoning_effort.is_none());
    assert!(request_path.is_none());
    assert!(speed_mode.is_none());
    assert!(ttft_ms.is_none());
    assert!(generation_speed.is_none());

    // 6. 校验索引已替换
    let index_names: Vec<String> = connection
        .prepare("SELECT name FROM pragma_index_list('usage_facts')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert!(index_names.contains(&"idx_usage_facts_model_status_time".to_owned()));
    assert!(index_names.contains(&"idx_usage_facts_audit_query".to_owned()));
    assert!(!index_names.contains(&"idx_usage_facts_model_time".to_owned()));
    assert!(!index_names.contains(&"idx_usage_facts_status_time".to_owned()));

    // 7. 校验 usage_rollup_minute 表与索引存在
    let minute_indices: Vec<String> = connection
        .prepare("SELECT name FROM pragma_index_list('usage_rollup_minute')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(minute_indices.contains(&"idx_rollup_min_bucket".to_owned()));
}

#[test]
fn schema_v4_to_v6_migration_preserves_data_without_drop() {
    let connection = Connection::open_in_memory().unwrap();
    create_schema_v4(&connection).unwrap();
    connection.pragma_update(None, "user_version", 4).unwrap();
    connection
        .execute(
            "INSERT INTO usage_facts (
                session_id, assistant_part_id, recorded_at_ms, hour_bucket_ms, day_bucket_ms,
                provider, model, source, status, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_calls, provider_duration_ms, usage_observed, cost_micros,
                source_fingerprint, upstream_model, reasoning_effort, request_path,
                speed_mode, ttft_ms, generation_speed_tps
             ) VALUES (
                'sess-v4', 'part-v4', 2000, 2000, 2000,
                'prov-v4', 'mod-v4', 'axon_cli', 'completed', 200, 100,
                50, 100, 0, 1, 500, 1, NULL, 'fp-v4',
                'upstream-gpt', 'high', 'primary_path', 'fast', 180, 55.5
             )",
            [],
        )
        .unwrap();

    let count_before: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_before, 1);

    initialize_schema(&connection).expect("平滑迁移 v4 -> v6 应成功");

    let version_after: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version_after, 6);

    let count_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_after, 1);

    // 校验 v5 新增的两列
    let (pricing_rule_id, recalculated_at): (Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT pricing_rule_id, recalculated_at_ms FROM usage_facts WHERE session_id = 'sess-v4'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(pricing_rule_id.is_none());
    assert!(recalculated_at.is_none());

    // 校验 pricing_rules 表已存在
    connection
        .execute(
            "INSERT INTO pricing_rules (
                id, model_pattern, pricing_style, input_price_per_m_micros,
                output_price_per_m_micros, cache_read_price_per_m_micros,
                cache_write_price_per_m_micros, multiplier_basis_points,
                priority, created_at_ms, updated_at_ms
             ) VALUES ('r1', 'gpt-4o', 'openai', 2500000, 10000000, 1250000, 0, 10000, 1, 1000, 1000)",
            [],
        )
        .unwrap();

    // 校验 usage_rollup_minute 表存在且可写
    connection
        .execute(
            "INSERT INTO usage_rollup_minute (
                bucket_start_ms, provider, model, source, status,
                request_count, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_duration_ms, ttft_duration_ms, measured_ttft_count,
                priced_cost_micros, unpriced_request_count
             ) VALUES (60000, 'openai', 'gpt-4o', 'cli', 'completed', 1, 10, 0, 0, 5, 0, 100, 50, 1, 100, 0)",
            [],
        )
        .unwrap();
}

#[test]
fn schema_v5_to_v6_migration_preserves_data_without_drop() {
    let connection = Connection::open_in_memory().unwrap();
    create_schema_v5(&connection).unwrap();
    connection.pragma_update(None, "user_version", 5).unwrap();

    // 插入 v5 历史事实数据
    connection
        .execute(
            "INSERT INTO usage_facts (
                session_id, assistant_part_id, recorded_at_ms, hour_bucket_ms, day_bucket_ms,
                provider, model, source, status, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_calls, provider_duration_ms, usage_observed, cost_micros,
                source_fingerprint, upstream_model, reasoning_effort, request_path,
                speed_mode, ttft_ms, generation_speed_tps, pricing_rule_id, recalculated_at_ms
             ) VALUES (
                'sess-v5', 'part-v5', 3000, 3000, 3000,
                'prov-v5', 'mod-v5', 'axon_cli', 'completed', 300, 150,
                0, 80, 20, 1, 600, 1, 1200, 'fp-v5',
                'upstream-v5', 'low', 'path_v5', 'standard', 120, 45.0,
                'rule-v5', 3001
             )",
            [],
        )
        .unwrap();

    let count_before: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_before, 1, "迁移前行数为 1");

    // 执行平滑迁移升级 v5 -> v6
    initialize_schema(&connection).expect("平滑迁移 v5 -> v6 应成功");

    let version_after: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version_after, 6, "PRAGMA user_version 必须升级为 6");

    let count_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_after, 1, "迁移后既有数据行数未变（零丢失）");

    // 验证 usage_rollup_minute 表与索引已建立且支持写入
    connection
        .execute(
            "INSERT INTO usage_rollup_minute (
                bucket_start_ms, provider, model, source, status,
                request_count, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_duration_ms, ttft_duration_ms, measured_ttft_count,
                priced_cost_micros, unpriced_request_count
             ) VALUES (60000, 'prov-v5', 'mod-v5', 'axon_cli', 'completed', 1, 300, 150, 0, 80, 20, 600, 120, 1, 1200, 0)",
            [],
        )
        .unwrap();

    let inserted_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_rollup_minute", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(inserted_count, 1);
}

#[test]
fn clean_v6_database_creates_all_columns_and_indices() {
    let connection = Connection::open_in_memory().unwrap();
    initialize_schema(&connection).expect("初始全新建库应成功");

    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 6, "全新建库 user_version 必须为 6");

    // 校验 usage_facts, pricing_rules, usage_rollup_minute 均存在
    connection
        .execute(
            "INSERT INTO usage_facts (
                session_id, assistant_part_id, recorded_at_ms, hour_bucket_ms, day_bucket_ms,
                provider, model, source, status, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_calls, provider_duration_ms, usage_observed, cost_micros,
                source_fingerprint, upstream_model, reasoning_effort, request_path,
                speed_mode, ttft_ms, generation_speed_tps, pricing_rule_id, recalculated_at_ms
             ) VALUES (
                'sess-v6', 'part-v6', 2000, 2000, 2000,
                'prov-v6', 'mod-v6', 'axon_cli', 'completed', 200, 100,
                50, 100, 0, 1, 500, 1, NULL, 'fp-v6',
                'upstream-gpt', 'high', 'primary_path', 'fast', 180, 55.5,
                'rule-1', 2001
             )",
            [],
        )
        .unwrap();

    connection
        .execute(
            "INSERT INTO pricing_rules (
                id, model_pattern, pricing_style, input_price_per_m_micros,
                output_price_per_m_micros, cache_read_price_per_m_micros,
                cache_write_price_per_m_micros, multiplier_basis_points,
                priority, created_at_ms, updated_at_ms
             ) VALUES ('r-v6', 'model-v6', 'openai', 100, 200, 50, 0, 10000, 1, 100, 100)",
            [],
        )
        .unwrap();

    connection
        .execute(
            "INSERT INTO usage_rollup_minute (
                bucket_start_ms, provider, model, source, status,
                request_count, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_duration_ms, ttft_duration_ms, measured_ttft_count,
                priced_cost_micros, unpriced_request_count
             ) VALUES (60000, 'prov-v6', 'mod-v6', 'axon_cli', 'completed', 1, 200, 100, 50, 100, 0, 500, 180, 1, 0, 1)",
            [],
        )
        .unwrap();

    let minute_indices: Vec<String> = connection
        .prepare("SELECT name FROM pragma_index_list('usage_rollup_minute')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(minute_indices.contains(&"idx_rollup_min_bucket".to_owned()));
}
