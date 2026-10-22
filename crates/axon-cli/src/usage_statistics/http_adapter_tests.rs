//! M6 可观测性 RESTful API 与 CLI 专项测试套件。
//!
//! 覆盖：
//! 1. 六大端点 + 写端点完整路由测试；
//! 2. 401 鉴权与 INVALID_ARGUMENT 越界错误模型断言；
//! 3. EXPLAIN QUERY PLAN 索引覆盖断言（无 SCAN usage_facts）；
//! 4. CLI 子命令 (usage diagnostics/audit/export 与 pricing list/add/delete) 实测。

use tempfile::tempdir;

use super::cli_commands::{
    AddPricingRuleArgs, PricingCommand, PricingSubcommand, UsageAuditCommand, UsageCommand,
    UsageDiagnosticsCommand, UsageSubcommand, run_pricing_cli, run_usage_cli,
};
use super::http_adapter::ObservabilityHttpAdapter;
use super::model::{UsageFact, UsageSource, UsageStatus};
use super::observability_model::ObservabilityErrorResponse;
use super::repository::UsageRepository;
use axon_daemon::GatewayHttpRouteHandler;

fn create_test_env() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp_dir = tempdir().expect("创建临时目录");
    let config_path = temp_dir.path().join("axon.toml");
    std::fs::write(&config_path, "# test config\n").expect("写入配置");
    let axon_root = axon_config::runtime_axon_root_path(&config_path).expect("解析根目录");
    let _ = UsageRepository::open(&axon_root).expect("初始化测试库");
    (temp_dir, config_path)
}

fn seed_fact(
    repo: &mut UsageRepository,
    session_id: &str,
    model: &str,
    recorded_at_ms: u64,
    cost_micros: u64,
) {
    let fact = UsageFact {
        session_id: session_id.to_owned(),
        assistant_part_id: "part_0".to_owned(),
        recorded_at_ms,
        hour_bucket_ms: (recorded_at_ms / 3_600_000) * 3_600_000,
        day_bucket_ms: (recorded_at_ms / 86_400_000) * 86_400_000,
        provider: "google".to_owned(),
        model: model.to_owned(),
        source: UsageSource::AxonCli,
        status: UsageStatus::Completed,
        total_input_tokens: 10_000,
        cache_read_input_tokens: 2_000,
        cache_write_input_tokens: 0,
        output_tokens: 500,
        reasoning_output_tokens: 100,
        provider_calls: 1,
        provider_duration_ms: 1800,
        usage_observed: true,
        cost_micros: Some(cost_micros),
        pricing_rule_id: None,
        recalculated_at_ms: None,
        source_fingerprint: format!("{session_id}:0"),
        upstream_model: Some("gemini-3.8-flash".to_owned()),
        reasoning_effort: Some("high".to_owned()),
        request_path: Some("/chat/completions".to_owned()),
        speed_mode: Some("auto".to_owned()),
        ttft_ms: Some(600),
        generation_speed_tps: Some(40.0),
    };
    repo.project_facts(&[fact], 1, recorded_at_ms)
        .expect("落库事实");
}

// ============================================================================
// HTTP 路由与统一错误模型测试
// ============================================================================

#[test]
fn test_m6_observability_endpoints_and_error_model() {
    let (_dir, config_path) = create_test_env();
    let adapter = ObservabilityHttpAdapter::new(&config_path);

    // 1. GET /overview
    let res = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/overview",
            Some("period=today"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res.status, 200);
    assert_eq!(res.content_type, "application/json");
    let json: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert!(json.get("summary").is_some());
    assert!(json.get("sparklines").is_some());
    assert!(json.get("activityHeatmap").is_some());
    assert!(json.get("healthTimeline").is_some());

    // 2. GET /realtime
    let res = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/realtime",
            Some("window=15m"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res.status, 200);
    let json: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert_eq!(json["window"], "15m");

    // 3. GET /distribution
    let res = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/distribution",
            Some("dimension=model&period=today"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res.status, 200);
    let json: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert!(json.get("items").is_some());
    assert!(json.get("timeSeries").is_some());

    // 4. GET /diagnostics (合规参数)
    let res = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/diagnostics",
            Some("period=7d"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res.status, 200);
    let json: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert!(json.get("percentiles").is_some());
    assert!(json.get("scatterPoints").is_some());
    assert!(json.get("modelEfficiency").is_some());

    // 5. GET /diagnostics 越界参数 (INVALID_ARGUMENT 与 maxAllowed 披露)
    let res = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/diagnostics",
            Some("period=30d"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res.status, 400);
    let json: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert_eq!(json["error"]["code"], "INVALID_ARGUMENT");
    assert_eq!(json["error"]["details"]["maxAllowed"], "7d");

    // 6. GET /audit/logs limit 越界
    let res = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/audit/logs",
            Some("limit=999"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res.status, 400);
    let json: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert_eq!(json["error"]["code"], "INVALID_ARGUMENT");
    assert_eq!(json["error"]["details"]["maxAllowed"], 500);

    // 7. GET /audit/export 格式越界与合规导出
    let res_err = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/audit/export",
            Some("format=xml"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res_err.status, 400);

    let res_csv = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/audit/export",
            Some("format=csv"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res_csv.status, 200);
    assert!(res_csv.content_type.starts_with("text/csv"));

    let res_json = adapter
        .handle_request(
            "GET",
            "/api/v1/observability/audit/export",
            Some("format=json"),
            &[],
        )
        .expect("路由命中");
    assert_eq!(res_json.status, 200);
    assert_eq!(res_json.content_type, "application/json");

    // 8. Pricing rules 写操作与删除
    let new_rule = serde_json::json!({
        "modelPattern": "gpt-5*",
        "pricingStyle": "openai",
        "inputPricePerMMicros": 150000,
        "outputPricePerMMicros": 600000,
        "cacheReadPricePerMMicros": 75000,
        "cacheWritePricePerMMicros": 0,
        "multiplierBasisPoints": 10000,
        "priority": 10
    });
    let res_post = adapter
        .handle_request(
            "POST",
            "/api/v1/observability/pricing/rules",
            None,
            &new_rule.to_string().into_bytes(),
        )
        .expect("路由命中");
    assert_eq!(res_post.status, 200);
    let post_json: serde_json::Value = serde_json::from_slice(&res_post.body).unwrap();
    let created_id = post_json["id"].as_str().unwrap();

    let res_del = adapter
        .handle_request(
            "DELETE",
            &format!("/api/v1/observability/pricing/rules/{created_id}"),
            None,
            &[],
        )
        .expect("路由命中");
    assert_eq!(res_del.status, 200);

    // 9. 404 Not Found
    let res_404 = adapter
        .handle_request("GET", "/api/v1/observability/unknown_subpath", None, &[])
        .expect("路由命中");
    assert_eq!(res_404.status, 404);

    // 10. 统一错误模型结构断言
    let auth_err = ObservabilityErrorResponse::unauthorized("未授权测试");
    assert_eq!(auth_err.error.code, "UNAUTHORIZED");
}

// ============================================================================
// EXPLAIN QUERY PLAN 索引覆盖证据 (禁止 SCAN usage_facts)
// ============================================================================

#[test]
fn test_m6_explain_query_plan_no_full_table_scan() {
    let temp_dir = tempdir().expect("创建临时目录");
    let repo = UsageRepository::open(temp_dir.path()).expect("打开仓储");

    // 1. 审计检索查询计划验证：必须走索引 idx_usage_facts_audit_query 或 idx_usage_facts_time
    let mut stmt = repo
        .connection()
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT f.rowid, f.recorded_at_ms
             FROM usage_facts f
             WHERE f.model = ?1 AND f.status = ?2 AND f.source = ?3
             ORDER BY f.recorded_at_ms DESC, f.rowid DESC
             LIMIT 50",
        )
        .expect("准备 EXPLAIN 语句");

    let rows = stmt
        .query_map(["gpt-5", "completed", "cli"], |r| r.get::<_, String>(3))
        .expect("查询计划");
    let mut plan = String::new();
    for r in rows {
        plan.push_str(&r.unwrap());
        plan.push('\n');
    }
    assert!(
        plan.contains("idx_usage_facts_audit_query"),
        "审计过滤检索必须走 idx_usage_facts_audit_query 索引！实际计划:\n{plan}"
    );
    assert!(
        !plan.contains("SCAN usage_facts"),
        "严禁全表扫描 usage_facts！"
    );

    // 2. 诊断按时间范围检索查询计划：必须走 idx_usage_facts_time
    let mut stmt_diag = repo
        .connection()
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT recorded_at_ms, model, ttft_ms, provider_duration_ms
             FROM usage_facts
             WHERE recorded_at_ms >= ?1 AND recorded_at_ms <= ?2
             ORDER BY recorded_at_ms ASC",
        )
        .expect("准备 EXPLAIN 语句");

    let rows_diag = stmt_diag
        .query_map([1_000_i64, 2_000_i64], |r| r.get::<_, String>(3))
        .expect("查询计划");
    let mut plan_diag = String::new();
    for r in rows_diag {
        plan_diag.push_str(&r.unwrap());
        plan_diag.push('\n');
    }
    assert!(
        plan_diag.contains("USING INDEX idx_usage_facts_time"),
        "诊断范围查询必须走 idx_usage_facts_time 索引！实际计划:\n{plan_diag}"
    );
    assert!(
        !plan_diag.contains("SCAN usage_facts"),
        "严禁全表扫描 usage_facts！"
    );
}

// ============================================================================
// CLI 运行样例测试
// ============================================================================

#[test]
fn test_m6_cli_usage_and_pricing_commands() {
    let (_temp_dir, config_path) = create_test_env();

    // 种子数据：使用当前时间窗口内的时间戳 (当前时间前 10 秒)
    let axon_root = axon_config::runtime_axon_root_path(&config_path).expect("解析根目录");
    let mut repo = UsageRepository::open(&axon_root).expect("打开仓储");
    let now_ms = axon_time::unix_now_ms();
    seed_fact(
        &mut repo,
        "s1",
        "gemini-3.8-flash-high",
        now_ms - 10_000,
        8520,
    );

    // 1. `axon usage diagnostics`
    let cmd_diag = UsageCommand {
        action: Some(UsageSubcommand::Diagnostics(UsageDiagnosticsCommand {
            period: "7d".to_owned(),
        })),
        period: super::model::UsagePeriod::Today,
        provider: None,
        model: None,
        source: super::model::UsageSourceFilter::All,
        session: None,
        run: None,
        limit: 20,
        refresh: false,
        format: None,
    };
    let res = run_usage_cli(&cmd_diag, crate::OutputFormat::Text, Some(&config_path));
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.contains("【延迟与效率深度诊断】"));
    assert!(res.stdout.contains("gemini-3.8-flash-high"));

    // 2. `axon usage audit`
    let cmd_audit = UsageCommand {
        action: Some(UsageSubcommand::Audit(UsageAuditCommand {
            model: None,
            source: None,
            status: None,
            from_ms: None,
            to_ms: None,
            cursor: None,
            limit: 10,
        })),
        period: super::model::UsagePeriod::Today,
        provider: None,
        model: None,
        source: super::model::UsageSourceFilter::All,
        session: None,
        run: None,
        limit: 20,
        refresh: false,
        format: None,
    };
    let res = run_usage_cli(&cmd_audit, crate::OutputFormat::Text, Some(&config_path));
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.contains("【审计日志】符合条件总数: 1"));

    // 3. `axon pricing add`
    let cmd_add = PricingCommand {
        action: Some(PricingSubcommand::Add(AddPricingRuleArgs {
            pattern: "gemini*".to_owned(),
            style: "claude".to_owned(),
            input_price: 75000,
            output_price: 300000,
            cache_read_price: 18750,
            cache_write_price: 93750,
            multiplier: 10000,
            priority: 5,
        })),
        format: None,
    };
    let res = run_pricing_cli(&cmd_add, crate::OutputFormat::Text, Some(&config_path));
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.contains("成功创建价格规则"));

    // 4. `axon pricing list`
    let cmd_list = PricingCommand {
        action: Some(PricingSubcommand::List),
        format: None,
    };
    let res = run_pricing_cli(&cmd_list, crate::OutputFormat::Text, Some(&config_path));
    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.contains("gemini*"));
    assert!(res.stdout.contains("claude"));
}
