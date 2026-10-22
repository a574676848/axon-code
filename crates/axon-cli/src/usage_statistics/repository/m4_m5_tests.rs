//! M4 (诊断与审计) 与 M5 (数据生命周期与防膨胀) 专项测试套件。
//!
//! 覆盖：
//! 1. M4: 分位数 nearest-rank、样本抽稀披露、散点上限、模型效益公式；
//! 2. M4: Base64 游标翻页无跳漏无重复单测；
//! 3. M4: pricingStyle 由 JOIN 派生验证；
//! 4. M4: CSV RFC4180 转义与 JSON 流式导出；
//! 5. M5: dry-run 与真实删除谓词 100% 镜像；
//! 6. M5: 五档 TTL 淘汰验证；
//! 7. M5: auto_vacuum 一次性迁移前后值与幂等性；
//! 8. M5: 索引规格核验。

use std::path::Path;

use tempfile::tempdir;

use super::UsageRepository;
use super::audit::{
    AuditLogQuery, export_audit_logs_csv, export_audit_logs_json, query_audit_logs,
};
use super::diagnostics::{
    RawDiagnosticFact, compute_diagnostics_report, nearest_rank_percentile, thin_equidistant,
};
use super::retention::{
    TTL_DAY_ROLLUP_MS, TTL_FACTS_MS, TTL_HOUR_ROLLUP_MS, TTL_MINUTE_ROLLUP_MS, TTL_ORPHAN_ACTIVE_MS,
};
use crate::usage_statistics::model::{UsageFact, UsageSource, UsageStatus};
use axon_domain::{ModelPriceRule, PricingStyle};

fn create_temp_repository(dir: &Path) -> UsageRepository {
    UsageRepository::open(dir).expect("打开测试数据库")
}

fn make_fact(
    session_id: &str,
    part_id: &str,
    recorded_at_ms: u64,
    status: UsageStatus,
    pricing_rule_id: Option<&str>,
) -> UsageFact {
    UsageFact {
        session_id: session_id.to_owned(),
        assistant_part_id: part_id.to_owned(),
        recorded_at_ms,
        hour_bucket_ms: (recorded_at_ms / 3_600_000) * 3_600_000,
        day_bucket_ms: (recorded_at_ms / 86_400_000) * 86_400_000,
        provider: "openai".to_owned(),
        model: "gpt-5-turbo".to_owned(),
        source: UsageSource::AxonCli,
        status,
        total_input_tokens: 1000,
        cache_read_input_tokens: 300,
        cache_write_input_tokens: 0,
        output_tokens: 200,
        reasoning_output_tokens: 50,
        provider_calls: 1,
        provider_duration_ms: 1500,
        usage_observed: true,
        cost_micros: Some(15000),
        pricing_rule_id: pricing_rule_id.map(ToOwned::to_owned),
        recalculated_at_ms: None,
        source_fingerprint: format!("{session_id}:{part_id}"),
        upstream_model: Some("gpt-5".to_owned()),
        reasoning_effort: Some("medium".to_owned()),
        request_path: Some("/chat/completions".to_owned()),
        speed_mode: Some("standard".to_owned()),
        ttft_ms: Some(450),
        generation_speed_tps: Some(35.5),
    }
}

// ============================================================================
// M4 专项测试
// ============================================================================

#[test]
fn test_m4_nearest_rank_percentiles() {
    let samples: Vec<u64> = (1..=100).collect();
    assert_eq!(nearest_rank_percentile(&samples, 50), Some(50));
    assert_eq!(nearest_rank_percentile(&samples, 90), Some(90));
    assert_eq!(nearest_rank_percentile(&samples, 95), Some(95));
    assert_eq!(nearest_rank_percentile(&samples, 99), Some(99));

    let ten_samples: Vec<u64> = (1..=10).collect();
    assert_eq!(nearest_rank_percentile(&ten_samples, 50), Some(5));
    assert_eq!(nearest_rank_percentile(&ten_samples, 90), Some(9));
    assert_eq!(nearest_rank_percentile(&ten_samples, 95), Some(10));
    assert_eq!(nearest_rank_percentile(&ten_samples, 99), Some(10));
}

#[test]
fn test_m4_equidistant_thinning_and_disclosure() {
    let large_samples: Vec<u64> = (1..=15_000).collect();
    let (thinned, disclosure) = thin_equidistant(large_samples, 10_000);
    assert_eq!(thinned.len(), 10_000);
    assert_eq!(thinned[0], 1);
    assert_eq!(thinned[9_999], 15_000);

    let info = disclosure.expect("应披露抽稀率");
    assert_eq!(info.sampled_of, 15_000);
    assert_eq!(info.sample_count, 10_000);
    assert!((info.sampling_ratio - (10_000.0 / 15_000.0)).abs() < 1e-4);
}

#[test]
fn test_m4_model_efficiency_and_scatter_limit() {
    let mut facts = Vec::new();
    for i in 1..=3_000 {
        facts.push(RawDiagnosticFact {
            recorded_at_ms: i * 1000,
            model: "gemini-3.8-flash-high".to_owned(),
            ttft_ms: Some(500),
            provider_duration_ms: 1200,
            total_tokens: 10_000,
            cost_micros: 852,
        });
    }
    // 添加一个 0 token 模型
    facts.push(RawDiagnosticFact {
        recorded_at_ms: 4_000_000,
        model: "zero-token-model".to_owned(),
        ttft_ms: None,
        provider_duration_ms: 50,
        total_tokens: 0,
        cost_micros: 0,
    });

    let report = compute_diagnostics_report(facts, None);
    assert_eq!(report.scatter_points.len(), 2_000); // 严格截断至散点上限 2,000
    assert_eq!(report.model_efficiency.len(), 1); // 0 token 模型不进榜
    assert_eq!(report.model_efficiency[0].model, "gemini-3.8-flash-high");
    assert!((report.model_efficiency[0].cost_per1m_tokens - 0.0852).abs() < 1e-4);
}

#[test]
fn test_m4_audit_cursor_pagination_no_skip_no_dup() {
    let temp_dir = tempdir().expect("创建临时目录");
    let mut repo = create_temp_repository(temp_dir.path());

    // 写入 120 条审计记录，时间戳递增，request_path 携带唯一 sess_i
    let mut facts = Vec::new();
    for i in 1..=120 {
        let mut f = make_fact(
            &format!("sess_{i}"),
            "part_0",
            1_700_000_000_000 + i * 1_000,
            UsageStatus::Completed,
            None,
        );
        f.request_path = Some(format!("/sess_{i}"));
        facts.push(f);
    }
    repo.project_facts(&facts, 120, 1_700_000_200_000)
        .expect("写入事实");

    let mut collected_ids = Vec::new();
    let mut current_cursor: Option<String> = None;
    let limit = 25;

    loop {
        let query = AuditLogQuery {
            cursor: current_cursor.as_deref(),
            limit,
            ..Default::default()
        };
        let page = query_audit_logs(repo.connection(), &query).expect("查询分页");

        for event in &page.events {
            collected_ids.push(event.request_path.clone().unwrap());
        }

        if !page.has_more {
            break;
        }
        assert!(page.next_cursor.is_some());
        current_cursor = page.next_cursor;
    }

    assert_eq!(collected_ids.len(), 120, "总条数严格无漏");
    // 检查是否有重复
    let mut deduped = collected_ids.clone();
    deduped.dedup();
    assert_eq!(deduped.len(), 120, "分页翻页严格无重复");
}

#[test]
fn test_m4_pricing_style_derived_via_join() {
    let temp_dir = tempdir().expect("创建临时目录");
    let mut repo = create_temp_repository(temp_dir.path());

    // 1. 插入一条价格规则
    let rule = ModelPriceRule {
        id: "pr_claude_1".to_owned(),
        model_pattern: "claude*".to_owned(),
        pricing_style: PricingStyle::Claude,
        input_price_per_m_micros: 3_000_000,
        output_price_per_m_micros: 15_000_000,
        cache_read_price_per_m_micros: 375_000,
        cache_write_price_per_m_micros: 3_750_000,
        multiplier_basis_points: 10_000,
        priority: 10,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };
    repo.save_pricing_rule(&rule).expect("保存规则");

    // 2. 插入两条事实：一条关联该规则，一条不关联
    let fact_with_rule = make_fact(
        "s1",
        "p1",
        1000,
        UsageStatus::Completed,
        Some("pr_claude_1"),
    );
    let fact_without_rule = make_fact("s2", "p2", 2000, UsageStatus::Completed, None);
    repo.project_facts(&[fact_with_rule, fact_without_rule], 2, 2000)
        .expect("写入事实");

    let logs = query_audit_logs(
        repo.connection(),
        &AuditLogQuery {
            limit: 10,
            ..Default::default()
        },
    )
    .expect("查询日志");

    assert_eq!(logs.events.len(), 2);
    // 按照时间降序：s2 是 2000，s1 是 1000
    assert_eq!(logs.events[0].pricing_style, "unpriced");
    assert_eq!(logs.events[1].pricing_style, "claude");
}

#[test]
fn test_m4_csv_rfc4180_and_json_streaming_export() {
    let temp_dir = tempdir().expect("创建临时目录");
    let mut repo = create_temp_repository(temp_dir.path());

    let mut fact = make_fact("s1", "p1", 1000, UsageStatus::Completed, None);
    fact.model = "test,model\"quote\"\nnewline".to_owned();
    repo.project_facts(&[fact], 1, 1000).expect("写入事实");

    // CSV 导出
    let mut csv_buf = Vec::new();
    let query = AuditLogQuery {
        limit: 10,
        ..Default::default()
    };
    let count = export_audit_logs_csv(repo.connection(), &query, &mut csv_buf).expect("导出 CSV");
    assert_eq!(count, 1);
    let csv_str = String::from_utf8(csv_buf).expect("UTF-8 无 BOM");
    assert!(csv_str.contains("\"test,model\"\"quote\"\"\nnewline\""));

    // JSON 导出
    let mut json_buf = Vec::new();
    let count_json =
        export_audit_logs_json(repo.connection(), &query, &mut json_buf).expect("导出 JSON");
    assert_eq!(count_json, 1);
    let json_val: serde_json::Value = serde_json::from_slice(&json_buf).expect("JSON 合法");
    assert!(json_val.is_array());
    assert_eq!(json_val.as_array().unwrap().len(), 1);
}

// ============================================================================
// M5 专项测试
// ============================================================================

#[test]
fn test_m5_five_tier_retention_and_dry_run_mirror() {
    let temp_dir = tempdir().expect("创建临时目录");
    let mut repo = create_temp_repository(temp_dir.path());

    let now_ms = 1_700_000_000_000_u64;

    // 1. 孤儿 active 事实（超 7 天）：now - 8 天
    let stale_active_time = now_ms - TTL_ORPHAN_ACTIVE_MS - 86_400_000;
    let stale_active = make_fact(
        "s_orphan",
        "p0",
        stale_active_time,
        UsageStatus::Active,
        None,
    );

    // 2. 超 90 天普通事实：now - 95 天
    let old_fact_time = now_ms - TTL_FACTS_MS - 5 * 86_400_000;
    let old_fact = make_fact("s_old", "p0", old_fact_time, UsageStatus::Completed, None);

    // 3. 正常新鲜事实：now - 1 天
    let fresh_time = now_ms - 86_400_000;
    let fresh_fact = make_fact("s_fresh", "p0", fresh_time, UsageStatus::Completed, None);

    repo.project_facts(&[stale_active, old_fact, fresh_fact], 3, now_ms)
        .expect("写入事实");

    // 插入测试 minute_rollup (超 48 小时 vs 新鲜)
    let old_minute_bucket = (now_ms - TTL_MINUTE_ROLLUP_MS - 3_600_000) as i64;
    let fresh_minute_bucket = (now_ms - 1_800_000) as i64;
    repo.connection()
        .execute(
            "INSERT INTO usage_rollup_minute (
                bucket_start_ms, provider, model, source, status,
                request_count, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_duration_ms, ttft_duration_ms, measured_ttft_count,
                priced_cost_micros, unpriced_request_count
             ) VALUES (?1, 'openai', 'gpt-5', 'cli', 'completed', 1, 100, 0, 0, 50, 0, 500, 200, 1, 100, 0),
                      (?2, 'openai', 'gpt-5', 'cli', 'completed', 1, 100, 0, 0, 50, 0, 500, 200, 1, 100, 0)",
            rusqlite::params![old_minute_bucket, fresh_minute_bucket],
        )
        .expect("插入测试分钟聚合");

    // 插入测试 hour_rollup (超 90 天 vs 新鲜)
    let old_hour_bucket = (now_ms - TTL_HOUR_ROLLUP_MS - 86_400_000) as i64;
    repo.connection()
        .execute(
            "INSERT INTO usage_rollup_hour (
                bucket_start_ms, provider, model, source, status,
                request_count, provider_call_count, total_input_tokens,
                cache_read_input_tokens, cache_write_input_tokens, output_tokens,
                reasoning_output_tokens, provider_duration_ms, measured_latency_count,
                priced_cost_micros, unpriced_request_count
             ) VALUES (?1, 'openai', 'gpt-5', 'cli', 'completed', 1, 1, 100, 0, 0, 50, 0, 500, 1, 100, 0)",
            [old_hour_bucket],
        )
        .expect("插入测试小时聚合");

    // 插入测试 day_rollup (超 365 天 vs 新鲜)
    let old_day_bucket = (now_ms - TTL_DAY_ROLLUP_MS - 86_400_000) as i64;
    repo.connection()
        .execute(
            "INSERT INTO usage_rollup_day (
                bucket_start_ms, provider, model, source, status,
                request_count, provider_call_count, total_input_tokens,
                cache_read_input_tokens, cache_write_input_tokens, output_tokens,
                reasoning_output_tokens, provider_duration_ms, measured_latency_count,
                priced_cost_micros, unpriced_request_count
             ) VALUES (?1, 'openai', 'gpt-5', 'cli', 'completed', 1, 1, 100, 0, 0, 50, 0, 500, 1, 100, 0)",
            [old_day_bucket],
        )
        .expect("插入测试日聚合");

    // --- 第一步：Dry-Run 预测 ---
    let dry_run_report = repo
        .cleanup_five_tier_retention(now_ms, true)
        .expect("执行 dry-run");
    assert!(dry_run_report.dry_run);
    assert_eq!(
        dry_run_report.stale_active_marked, 1,
        "预测孤儿 active 1 条"
    );
    assert_eq!(dry_run_report.deleted_facts, 1, "预测到期 facts 1 条");
    // project_facts 已将 stale_active 与 old_fact 累加至分钟聚合，加上手动插入的 1 条，共 3 条到期
    assert_eq!(
        dry_run_report.deleted_minute_rollups, 3,
        "预测到期分钟聚合 3 条"
    );
    // project_facts 已将 old_fact (95天前) 聚合至小时表，加上手动插入的 1 条，共 2 条到期
    assert_eq!(
        dry_run_report.deleted_hour_rollups, 2,
        "预测到期小时聚合 2 条"
    );
    assert_eq!(dry_run_report.deleted_day_rollups, 1, "预测到期日聚合 1 条");

    // 验证数据库未被修改
    let total_facts_before: i64 = repo
        .connection()
        .query_row("SELECT count(*) FROM usage_facts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_facts_before, 3);

    // --- 第二步：真实执行清理 ---
    let real_report = repo
        .cleanup_five_tier_retention(now_ms, false)
        .expect("执行真实清理");
    assert!(!real_report.dry_run);
    assert_eq!(
        real_report.stale_active_marked,
        dry_run_report.stale_active_marked
    );
    assert_eq!(real_report.deleted_facts, dry_run_report.deleted_facts);
    assert_eq!(
        real_report.deleted_minute_rollups,
        dry_run_report.deleted_minute_rollups
    );
    assert_eq!(
        real_report.deleted_hour_rollups,
        dry_run_report.deleted_hour_rollups
    );
    assert_eq!(
        real_report.deleted_day_rollups,
        dry_run_report.deleted_day_rollups
    );

    // 验证数据库变更：
    // 孤儿 active 事实被标记为 abandoned
    let orphan_status: String = repo
        .connection()
        .query_row(
            "SELECT status FROM usage_facts WHERE session_id = 's_orphan'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan_status, "abandoned");

    // 到期 facts 被物理删除，剩余 2 条（s_orphan 与 s_fresh）
    let total_facts_after: i64 = repo
        .connection()
        .query_row("SELECT count(*) FROM usage_facts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_facts_after, 2);

    // 到期分钟/小时/日聚合被删除，新鲜分钟聚合保留（fresh_fact 自动生成的分钟桶 + 手动插入的 1 个新鲜分钟桶 = 2）
    let remaining_minutes: i64 = repo
        .connection()
        .query_row("SELECT count(*) FROM usage_rollup_minute", [], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining_minutes, 2);
}

#[test]
fn test_m5_auto_vacuum_migration_and_index_verification() {
    let temp_dir = tempdir().expect("创建临时目录");
    let repo = create_temp_repository(temp_dir.path());

    // 1. auto_vacuum 检测与迁移
    let _ = UsageRepository::ensure_auto_vacuum_incremental(repo.connection())
        .expect("auto_vacuum 迁移");
    // 初次迁移可能变更，再次调用必须幂等返回 Ok(false)
    let mode: i64 = repo
        .connection()
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, 2, "auto_vacuum 模式必须为 2 (INCREMENTAL)");

    let second_run = UsageRepository::ensure_auto_vacuum_incremental(repo.connection())
        .expect("二次调用 auto_vacuum");
    assert!(!second_run, "二次调用必须幂等无需迁移");

    // 2. 索引规格核验
    let indexes = UsageRepository::verify_indexes(repo.connection()).expect("核验索引列表");
    assert!(indexes.contains(&"idx_usage_facts_time".to_owned()));
    assert!(indexes.contains(&"idx_usage_facts_provider_time".to_owned()));
    assert!(indexes.contains(&"idx_usage_facts_model_status_time".to_owned()));
    assert!(indexes.contains(&"idx_usage_facts_audit_query".to_owned()));
    assert!(indexes.contains(&"idx_rollup_min_bucket".to_owned()));
    assert!(indexes.contains(&"idx_pricing_pattern".to_owned()));
}
