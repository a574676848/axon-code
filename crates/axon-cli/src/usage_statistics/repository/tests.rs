use std::path::Path;

use crate::usage_statistics::{
    model::{UsageFact, UsageSource, UsageStatus},
    repository::UsageRepository,
};
use axon_domain::{GenerationMetrics, RuntimeObservationFrame};
use axon_session_runtime::{
    TurnPerformanceSnapshot, TurnPerformanceStage, TurnPerformanceStageTiming,
};

fn test_repository(path: &Path) -> UsageRepository {
    UsageRepository::open(path).expect("创建测试 UsageRepository")
}

fn sample_fact(
    session_id: &str,
    part_id: &str,
    status: UsageStatus,
    ttft_ms: Option<u64>,
    speed_tps: Option<f64>,
    fingerprint: &str,
) -> UsageFact {
    UsageFact {
        session_id: session_id.to_owned(),
        assistant_part_id: part_id.to_owned(),
        recorded_at_ms: 1_000,
        hour_bucket_ms: 1_000,
        day_bucket_ms: 1_000,
        provider: "anthropic".to_owned(),
        model: "claude-3-5-sonnet".to_owned(),
        source: UsageSource::AxonCli,
        status,
        total_input_tokens: 500,
        cache_read_input_tokens: 200,
        cache_write_input_tokens: 50,
        output_tokens: 150,
        reasoning_output_tokens: 0,
        provider_calls: 1,
        provider_duration_ms: 1_200,
        usage_observed: true,
        cost_micros: None,
        pricing_rule_id: None,
        recalculated_at_ms: None,
        source_fingerprint: fingerprint.to_owned(),
        upstream_model: Some("claude-3-5-sonnet-20241022".to_owned()),
        reasoning_effort: Some("high".to_owned()),
        request_path: Some("anthropic_primary".to_owned()),
        speed_mode: None,
        ttft_ms,
        generation_speed_tps: speed_tps,
    }
}

#[test]
fn idempotent_projection_leaves_row_count_and_no_change_on_second_run() {
    let temp = axon_test_support::test_temp_dir();
    let mut repo = test_repository(temp.path());

    let fact = sample_fact(
        "sess-idempotent",
        "part-1",
        UsageStatus::Completed,
        Some(120),
        Some(45.0),
        "fp-idempotent-1",
    );

    // 第一次投影
    let changed_1 = repo
        .project_facts(std::slice::from_ref(&fact), 1, 1_000)
        .unwrap();
    assert_eq!(changed_1, 1, "首次投影必须产生 1 个变更");

    let count_1: i64 = repo
        .connection()
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count_1, 1);

    // 重复投影同一条事实（指纹相同）
    let changed_2 = repo.project_facts(&[fact], 1, 1_001).unwrap();
    assert_eq!(
        changed_2, 0,
        "重复投影相同指纹事实必须变更数为 0（幂等去重）"
    );

    let count_2: i64 = repo
        .connection()
        .query_row("SELECT COUNT(*) FROM usage_facts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count_2, 1, "重复投影后事实表总行数不得增加");
}

#[test]
fn null_ttft_and_generation_speed_semantics_preserved() {
    let temp = axon_test_support::test_temp_dir();
    let mut repo = test_repository(temp.path());

    let null_fact = sample_fact(
        "sess-null",
        "part-null",
        UsageStatus::Failed,
        None,
        None,
        "fp-null",
    );
    repo.project_facts(&[null_fact], 1, 1_000).unwrap();

    let conn = repo.connection();
    let (is_null_ttft, is_null_speed, raw_ttft, raw_speed): (bool, bool, Option<i64>, Option<f64>) =
        conn.query_row(
            "SELECT ttft_ms IS NULL, generation_speed_tps IS NULL, ttft_ms, generation_speed_tps
             FROM usage_facts WHERE session_id = 'sess-null'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    assert!(is_null_ttft, "未测得首字必须严格落盘为 SQL NULL");
    assert!(is_null_speed, "无首字时的生成速率必须严格落盘为 SQL NULL");
    assert_eq!(raw_ttft, None, "禁止用 0 毫秒冒充未测得首字");
    assert_eq!(raw_speed, None, "禁止用 0.0 TPS 冒充未测得速率");
}

#[test]
fn three_terminal_outcomes_projection() {
    let temp = axon_test_support::test_temp_dir();
    let mut repo = test_repository(temp.path());

    // 1. 正常完成
    let completed = sample_fact(
        "sess-term",
        "part-completed",
        UsageStatus::Completed,
        Some(150),
        Some(40.5),
        "fp-completed",
    );
    // 2. 异常失败 (报错无首字)
    let failed = sample_fact(
        "sess-term",
        "part-failed",
        UsageStatus::Failed,
        None,
        None,
        "fp-failed",
    );
    // 3. 流式中断 (中途测得了首字后中断)
    let interrupted = sample_fact(
        "sess-term",
        "part-interrupted",
        UsageStatus::Interrupted,
        Some(210),
        None,
        "fp-interrupted",
    );

    repo.project_facts(&[completed, failed, interrupted], 3, 2_000)
        .unwrap();

    let conn = repo.connection();

    // 校验 Completed
    let (status, ttft, speed): (String, Option<i64>, Option<f64>) = conn
        .query_row(
            "SELECT status, ttft_ms, generation_speed_tps FROM usage_facts WHERE assistant_part_id = 'part-completed'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert_eq!(ttft, Some(150));
    assert_eq!(speed, Some(40.5));

    // 校验 Failed
    let (status, ttft, speed): (String, Option<i64>, Option<f64>) = conn
        .query_row(
            "SELECT status, ttft_ms, generation_speed_tps FROM usage_facts WHERE assistant_part_id = 'part-failed'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(ttft, None);
    assert_eq!(speed, None);

    // 校验 Interrupted
    let (status, ttft, speed): (String, Option<i64>, Option<f64>) = conn
        .query_row(
            "SELECT status, ttft_ms, generation_speed_tps FROM usage_facts WHERE assistant_part_id = 'part-interrupted'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "interrupted");
    assert_eq!(ttft, Some(210));
    assert_eq!(speed, None);
}

#[test]
fn numerical_consistency_between_tui_and_usage_facts() {
    let session_id = "sess-consistent";
    let part_id = "part-consistent-1";

    // 1. 同一 Turn 性能快照打点 (start: 1000us, first_token: 1320us -> 320ms)
    let snapshot = TurnPerformanceSnapshot {
        stages: vec![
            TurnPerformanceStageTiming {
                stage: TurnPerformanceStage::ProviderRequestStarted,
                elapsed_us: 1_000_000,
            },
            TurnPerformanceStageTiming {
                stage: TurnPerformanceStage::ProviderFirstDeltaParsed,
                elapsed_us: 1_320_000,
            },
        ],
        ..Default::default()
    };

    let measured_ttft_ms = {
        let start = snapshot
            .stages
            .iter()
            .find(|s| s.stage == TurnPerformanceStage::ProviderRequestStarted)
            .unwrap();
        let first = snapshot
            .stages
            .iter()
            .find(|s| s.stage == TurnPerformanceStage::ProviderFirstDeltaParsed)
            .unwrap();
        (first.elapsed_us - start.elapsed_us) / 1000
    };
    assert_eq!(measured_ttft_ms, 320);

    // 2. TUI 接收 Observation 帧，提取终态展示值
    let frame = RuntimeObservationFrame::new(1_000, session_id, part_id).with_generation(
        GenerationMetrics {
            ttft_ms: Some(measured_ttft_ms),
            decode_ms: Some(680),
            output_tokens: 34,
            generation_speed_tps: Some(50.0),
            ..Default::default()
        },
    );
    let tui_display_ttft = frame.generation.unwrap().ttft_ms.unwrap();
    let tui_display_tokens = frame.generation.unwrap().output_tokens;
    assert_eq!(tui_display_ttft, 320);
    assert_eq!(tui_display_tokens, 34);

    // 3. 落盘至 usage_facts (由持久化事实投影)
    let temp = axon_test_support::test_temp_dir();
    let mut repo = test_repository(temp.path());

    let fact = UsageFact {
        session_id: session_id.to_owned(),
        assistant_part_id: part_id.to_owned(),
        recorded_at_ms: 2_000,
        hour_bucket_ms: 2_000,
        day_bucket_ms: 2_000,
        provider: "anthropic".to_owned(),
        model: "claude-3-5".to_owned(),
        source: UsageSource::AxonCli,
        status: UsageStatus::Completed,
        total_input_tokens: 100,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        output_tokens: tui_display_tokens,
        reasoning_output_tokens: 0,
        provider_calls: 1,
        provider_duration_ms: 1_000,
        usage_observed: true,
        cost_micros: None,
        pricing_rule_id: None,
        recalculated_at_ms: None,
        source_fingerprint: "fp-consistent".to_owned(),
        upstream_model: Some("claude-3-5".to_owned()),
        reasoning_effort: None,
        request_path: Some("primary".to_owned()),
        speed_mode: None,
        ttft_ms: Some(tui_display_ttft),
        generation_speed_tps: Some(50.0),
    };
    repo.project_facts(&[fact], 1, 2_000).unwrap();

    // 4. 从 usage_facts 查询落盘数值
    let (db_ttft, db_output_tokens): (i64, i64) = repo
        .connection()
        .query_row(
            "SELECT ttft_ms, output_tokens FROM usage_facts WHERE session_id = ?1 AND assistant_part_id = ?2",
            [session_id, part_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    // 5. 严格逐字段一致性断言 (误差为 0)
    assert_eq!(
        db_ttft as u64, tui_display_ttft,
        "TTFT 必须与 TUI 终态展示值严格逐字段一致（误差 0）"
    );
    assert_eq!(
        db_output_tokens as u64, tui_display_tokens,
        "Output Token 必须与 TUI 终态展示值严格逐字段一致（误差 0）"
    );
}

#[test]
fn test_usage_repository_pricing_rule_crud() {
    let temp = axon_test_support::test_temp_dir();
    let mut repo = test_repository(temp.path());

    let rule1 = axon_domain::ModelPriceRule {
        id: "repo_rule_1".to_string(),
        model_pattern: "gpt-4*".to_string(),
        pricing_style: axon_domain::PricingStyle::OpenAi,
        input_price_per_m_micros: 2_500_000,
        output_price_per_m_micros: 10_000_000,
        cache_read_price_per_m_micros: 1_250_000,
        cache_write_price_per_m_micros: 0,
        multiplier_basis_points: 10_000,
        priority: 0,
        created_at_ms: 1000,
        updated_at_ms: 1000,
    };
    let rule2 = axon_domain::ModelPriceRule {
        id: "repo_rule_2".to_string(),
        model_pattern: "gpt-4o".to_string(),
        pricing_style: axon_domain::PricingStyle::OpenAi,
        input_price_per_m_micros: 2_500_000,
        output_price_per_m_micros: 10_000_000,
        cache_read_price_per_m_micros: 1_250_000,
        cache_write_price_per_m_micros: 0,
        multiplier_basis_points: 10_000,
        priority: 1,
        created_at_ms: 1000,
        updated_at_ms: 1000,
    };

    repo.save_pricing_rule(&rule1).unwrap();
    let fetched = repo.get_pricing_rule("repo_rule_1").unwrap();
    assert_eq!(fetched, Some(rule1));

    repo.save_pricing_rules(&[rule2]).unwrap();
    let list = repo.list_pricing_rules().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "repo_rule_2");

    assert!(repo.delete_pricing_rule("repo_rule_1").unwrap());
    assert!(!repo.delete_pricing_rule("repo_rule_1").unwrap());
    assert_eq!(repo.list_pricing_rules().unwrap().len(), 1);

    assert_eq!(repo.delete_all_pricing_rules().unwrap(), 1);
    assert!(repo.list_pricing_rules().unwrap().is_empty());
}

#[test]
fn idempotent_projection_leaves_minute_rollup_and_window_unchanged_on_second_run() {
    let temp = axon_test_support::test_temp_dir();
    let mut repo = test_repository(temp.path());
    let window = std::sync::Arc::new(axon_domain::SlidingWindowRingBuffer::new());
    repo.set_window(std::sync::Arc::clone(&window));

    let fact = sample_fact(
        "sess-idempotent-min",
        "part-min-1",
        UsageStatus::Completed,
        Some(150),
        Some(50.0),
        "fp-idempotent-minute-1",
    );

    // 第一次投影
    let changed_1 = repo
        .project_facts(std::slice::from_ref(&fact), 1, 1000)
        .expect("首次投影应成功");
    assert_eq!(changed_1, 1, "首次投影必须产生 1 条变更");

    let minute_rows_1 = repo
        .query_minute_rollups(0, 10)
        .expect("查询分钟聚合应成功");
    assert_eq!(minute_rows_1.len(), 1);
    assert_eq!(minute_rows_1[0].request_count, 1);
    assert_eq!(minute_rows_1[0].total_input_tokens, 500);
    assert_eq!(minute_rows_1[0].output_tokens, 150);

    let window_snap_1 = window.snapshot(1000, 15);
    let initial_window_requests = window_snap_1.window_request_count;
    let initial_window_tokens = window_snap_1.window_total_tokens;
    assert_eq!(initial_window_requests, 1, "初始窗口必须正好 1 次请求");

    // 第二次投影同一条记录（指纹相同）
    let changed_2 = repo
        .project_facts(&[fact], 2, 2000)
        .expect("二次投影应成功");
    assert_eq!(changed_2, 0, "二次投影无变更，changed 必须为 0");

    let minute_rows_2 = repo
        .query_minute_rollups(0, 10)
        .expect("查询分钟聚合应成功");
    assert_eq!(minute_rows_2.len(), 1);
    assert_eq!(
        minute_rows_2[0].request_count, 1,
        "幂等去重后 request_count 绝不增加"
    );
    assert_eq!(minute_rows_2[0].total_input_tokens, 500);

    let window_snap_2 = window.snapshot(1000, 15);
    assert_eq!(
        window_snap_2.window_request_count, initial_window_requests,
        "重复投影事实时滑动窗口计数必须保持不变"
    );
    assert_eq!(
        window_snap_2.window_total_tokens, initial_window_tokens,
        "重复投影事实时滑动窗口 Token 必须保持不变"
    );
}

#[test]
fn rebuild_minute_bucket_is_equivalent_to_incremental_upsert() {
    let temp = axon_test_support::test_temp_dir();
    let mut repo = test_repository(temp.path());

    let mut fact1 = sample_fact(
        "sess-rebuild-1",
        "part-1",
        UsageStatus::Completed,
        Some(120),
        Some(40.0),
        "fp-rebuild-1",
    );
    fact1.recorded_at_ms = 65_000; // 落在第 1 分钟桶 (60,000)
    fact1.cost_micros = Some(1500);

    let mut fact2 = sample_fact(
        "sess-rebuild-2",
        "part-2",
        UsageStatus::Completed,
        Some(180),
        Some(60.0),
        "fp-rebuild-2",
    );
    fact2.recorded_at_ms = 75_000; // 同落在第 1 分钟桶
    fact2.total_input_tokens = 800;
    fact2.output_tokens = 200;
    fact2.cost_micros = Some(2500);

    let mut fact3 = sample_fact(
        "sess-rebuild-3",
        "part-3",
        UsageStatus::Failed,
        None,
        None,
        "fp-rebuild-3",
    );
    fact3.recorded_at_ms = 85_000; // 同落在第 1 分钟桶，但为 Failed 状态且未计价
    fact3.status = UsageStatus::Failed;
    fact3.cost_micros = None;
    fact3.ttft_ms = None;

    // 1. 增量写入这 3 条事实
    let changed = repo
        .project_facts(&[fact1, fact2, fact3], 1, 90_000)
        .expect("投影多条事实应成功");
    assert_eq!(changed, 3);

    // 2. 读取增量累加生成的分钟聚合行
    let mut incremental_rows = repo
        .query_minute_rollups(60_000, 10)
        .expect("查询增量分钟聚合行");
    incremental_rows.sort_by(|a, b| {
        (&a.provider, &a.model, &a.status).cmp(&(&b.provider, &b.model, &b.status))
    });

    assert!(!incremental_rows.is_empty(), "必须有生成的增量分钟聚合行");

    // 3. 执行按桶物理重建
    repo.rebuild_minute_bucket(60_000)
        .expect("按桶物理重建应成功");

    // 4. 读取重建生成的分钟聚合行
    let mut rebuilt_rows = repo
        .query_minute_rollups(60_000, 10)
        .expect("查询重建分钟聚合行");
    rebuilt_rows.sort_by(|a, b| {
        (&a.provider, &a.model, &a.status).cmp(&(&b.provider, &b.model, &b.status))
    });

    // 5. 逐行逐字段对照等价性断言
    assert_eq!(
        incremental_rows.len(),
        rebuilt_rows.len(),
        "增量累加与物理重建的聚合行数必须一致"
    );
    for (i, (inc, reb)) in incremental_rows.iter().zip(rebuilt_rows.iter()).enumerate() {
        assert_eq!(
            inc.bucket_start_ms, reb.bucket_start_ms,
            "row {i} bucket_start_ms 等价"
        );
        assert_eq!(inc.provider, reb.provider, "row {i} provider 等价");
        assert_eq!(inc.model, reb.model, "row {i} model 等价");
        assert_eq!(inc.source, reb.source, "row {i} source 等价");
        assert_eq!(inc.status, reb.status, "row {i} status 等价");
        assert_eq!(
            inc.request_count, reb.request_count,
            "row {i} request_count 等价"
        );
        assert_eq!(
            inc.total_input_tokens, reb.total_input_tokens,
            "row {i} total_input_tokens 等价"
        );
        assert_eq!(
            inc.cache_read_input_tokens, reb.cache_read_input_tokens,
            "row {i} cache_read 等价"
        );
        assert_eq!(
            inc.cache_write_input_tokens, reb.cache_write_input_tokens,
            "row {i} cache_write 等价"
        );
        assert_eq!(
            inc.output_tokens, reb.output_tokens,
            "row {i} output_tokens 等价"
        );
        assert_eq!(
            inc.reasoning_output_tokens, reb.reasoning_output_tokens,
            "row {i} reasoning_tokens 等价"
        );
        assert_eq!(
            inc.provider_duration_ms, reb.provider_duration_ms,
            "row {i} provider_duration_ms 等价"
        );
        assert_eq!(
            inc.ttft_duration_ms, reb.ttft_duration_ms,
            "row {i} ttft_duration_ms 等价"
        );
        assert_eq!(
            inc.measured_ttft_count, reb.measured_ttft_count,
            "row {i} measured_ttft_count 等价"
        );
        assert_eq!(
            inc.priced_cost_micros, reb.priced_cost_micros,
            "row {i} priced_cost_micros 等价"
        );
        assert_eq!(
            inc.unpriced_request_count, reb.unpriced_request_count,
            "row {i} unpriced_request_count 等价"
        );
    }
}
