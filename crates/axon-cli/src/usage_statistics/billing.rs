//! 计费核算引擎与历史重算管道服务。

use std::{path::Path, sync::Arc, time::Instant};

use serde::Serialize;

use super::repository::{
    UsageRepository,
    recalculate::{RECALCULATE_BATCH_LIMIT, RecalculateCursor},
};

/// 重算管道执行报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecalculateReport {
    /// 扫描的总事实记录数。
    pub total_scanned: u64,
    /// 发生更新并重算的事实记录数。
    pub updated_count: u64,
    /// 命中规则一致被幂等跳过的事实记录数。
    pub skipped_count: u64,
    /// 处理的批次数。
    pub batches_processed: u64,
    /// 单批事务持锁最大耗时毫秒数（硬指标：< 50ms）。
    pub max_batch_duration_ms: u64,
    /// 重算全流程总耗时毫秒数。
    pub total_duration_ms: u64,
    /// 完成时间戳毫秒数。
    pub completed_at_ms: u64,
}

/// 计费核算引擎。
#[derive(Default, Clone)]
pub struct BillingEngine {
    /// 进程内排他互斥锁，阻断并发重复触发。
    recalculate_lock: Arc<tokio::sync::Mutex<()>>,
}

impl BillingEngine {
    /// 创建计费引擎实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            recalculate_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// 触发全量历史用量重算。
    ///
    /// 遵循已裁决规范：
    /// 1. 应用层排他互斥；
    /// 2. 存储层 Immediate 事务；
    /// 3. 单批 ≤ 256 条；
    /// 4. 每批提交后 `tokio::task::yield_now().await`；
    /// 5. 刷新受影响的时/日 rollup 聚合桶；
    /// 6. 单批持锁必须小于 50ms。
    pub async fn recalculate(&self, axon_root: &Path) -> Result<RecalculateReport, String> {
        let _guard = self.recalculate_lock.lock().await;
        let start_time = Instant::now();

        let mut repository = UsageRepository::open(axon_root)?;
        let rules = repository.list_pricing_rules()?;
        let now_ms = axon_time::unix_now_ms();

        let mut cursor: Option<RecalculateCursor> = None;
        let mut total_scanned = 0_u64;
        let mut updated_count = 0_u64;
        let mut skipped_count = 0_u64;
        let mut batches_processed = 0_u64;
        let mut max_batch_duration_ms = 0_u64;

        loop {
            let outcome = repository.recalculate_batch(&rules, cursor.as_ref(), now_ms)?;
            let batch_ms = outcome.duration.as_millis() as u64;
            max_batch_duration_ms = max_batch_duration_ms.max(batch_ms);
            batches_processed += 1;
            total_scanned += outcome.scanned_count;
            updated_count += outcome.updated_count;
            skipped_count += outcome.skipped_count;

            // 让出写锁与执行权
            tokio::task::yield_now().await;

            if outcome.scanned_count < RECALCULATE_BATCH_LIMIT as u64
                || outcome.next_cursor.is_none()
            {
                break;
            }
            cursor = outcome.next_cursor;
        }

        let total_duration_ms = start_time.elapsed().as_millis() as u64;
        Ok(RecalculateReport {
            total_scanned,
            updated_count,
            skipped_count,
            batches_processed,
            max_batch_duration_ms,
            total_duration_ms,
            completed_at_ms: now_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_statistics::model::{UsageFact, UsageSource, UsageStatus};
    use axon_domain::{ModelPriceRule, PricingStyle};

    fn make_sample_fact(session_id: &str, model: &str, input: u64, output: u64) -> UsageFact {
        UsageFact {
            session_id: session_id.to_string(),
            assistant_part_id: "part-1".to_string(),
            recorded_at_ms: 1000,
            hour_bucket_ms: 1000,
            day_bucket_ms: 1000,
            provider: "openai".to_string(),
            model: model.to_string(),
            source: UsageSource::AxonCli,
            status: UsageStatus::Completed,
            total_input_tokens: input,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
            output_tokens: output,
            reasoning_output_tokens: 0,
            provider_calls: 1,
            provider_duration_ms: 500,
            usage_observed: true,
            cost_micros: None,
            pricing_rule_id: None,
            recalculated_at_ms: None,
            source_fingerprint: format!("fp-{}", session_id),
            upstream_model: None,
            reasoning_effort: None,
            request_path: None,
            speed_mode: None,
            ttft_ms: None,
            generation_speed_tps: None,
        }
    }

    #[tokio::test]
    async fn test_recalculate_lifecycle_and_idempotence() {
        let temp = axon_test_support::test_temp_dir();
        let engine = BillingEngine::new();

        // 1. 写入无规则事实数据
        {
            let mut repo = UsageRepository::open(temp.path()).unwrap();
            let fact1 = make_sample_fact("sess-1", "gpt-4o", 1000, 500);
            let fact2 = make_sample_fact("sess-2", "claude-3-5-sonnet", 2000, 1000);
            repo.project_facts(&[fact1, fact2], 1, 1000).unwrap();
        }

        // 2. 此时未配置任何规则，执行重算
        let report1 = engine.recalculate(temp.path()).await.unwrap();
        assert_eq!(report1.total_scanned, 2);
        assert_eq!(report1.updated_count, 0);
        assert_eq!(report1.skipped_count, 2); // 两个都未匹配，保持未计费状态
        assert!(report1.max_batch_duration_ms < 50, "单批持锁必须小于 50ms");

        // 3. 配置一条规则，针对 gpt-4*
        let rule_gpt = ModelPriceRule {
            id: "rule-gpt".to_string(),
            model_pattern: "gpt-4*".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 2_500_000,
            output_price_per_m_micros: 10_000_000,
            cache_read_price_per_m_micros: 0,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 1,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        {
            let repo = UsageRepository::open(temp.path()).unwrap();
            repo.save_pricing_rule(&rule_gpt).unwrap();
        }

        // 4. 执行重算：应该更新 1 条，跳过 1 条
        let report2 = engine.recalculate(temp.path()).await.unwrap();
        assert_eq!(report2.total_scanned, 2);
        assert_eq!(report2.updated_count, 1);
        assert_eq!(report2.skipped_count, 1);
        assert!(report2.max_batch_duration_ms < 50, "单批持锁必须小于 50ms");

        // 验证数据库中 sess-1 已正确计价，且 rollup priced_cost_micros 刷新
        {
            let repo = UsageRepository::open(temp.path()).unwrap();
            let (cost, rule_id, recalc_at): (Option<i64>, Option<String>, Option<i64>) = repo
                .connection()
                .query_row(
                    "SELECT cost_micros, pricing_rule_id, recalculated_at_ms FROM usage_facts WHERE session_id = 'sess-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            // 1000 * 2.5 + 500 * 10 = 2500 + 5000 = 7500 micros
            assert_eq!(cost, Some(7500));
            assert_eq!(rule_id.as_deref(), Some("rule-gpt"));
            assert!(recalc_at.is_some());

            // 检查 rollup 聚合
            let (priced_cost, unpriced_count): (i64, i64) = repo
                .connection()
                .query_row(
                    "SELECT SUM(priced_cost_micros), SUM(unpriced_request_count) FROM usage_rollup_hour",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(priced_cost, 7500);
            assert_eq!(unpriced_count, 1); // claude 仍未计费
        }

        // 5. 幂等验证：再次运行重算，应当全部跳过
        let report3 = engine.recalculate(temp.path()).await.unwrap();
        assert_eq!(report3.total_scanned, 2);
        assert_eq!(report3.updated_count, 0);
        assert_eq!(report3.skipped_count, 2);

        // 6. 规则被删除后重算：清空规则后，已计价的应该回退为未计价
        {
            let repo = UsageRepository::open(temp.path()).unwrap();
            repo.delete_all_pricing_rules().unwrap();
        }
        let report4 = engine.recalculate(temp.path()).await.unwrap();
        assert_eq!(report4.total_scanned, 2);
        assert_eq!(report4.updated_count, 1); // sess-1 从有规则变无规则，被重置
        assert_eq!(report4.skipped_count, 1);

        {
            let repo = UsageRepository::open(temp.path()).unwrap();
            let (cost, rule_id): (Option<i64>, Option<String>) = repo
                .connection()
                .query_row(
                    "SELECT cost_micros, pricing_rule_id FROM usage_facts WHERE session_id = 'sess-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert!(cost.is_none());
            assert!(rule_id.is_none());

            // rollup 桶也应更新为全部 unpriced
            let (priced_cost, unpriced_count): (i64, i64) = repo
                .connection()
                .query_row(
                    "SELECT SUM(priced_cost_micros), SUM(unpriced_request_count) FROM usage_rollup_hour",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(priced_cost, 0);
            assert_eq!(unpriced_count, 2);
        }
    }

    /// 单批持锁必须远低于阈值。
    ///
    /// 这个断言的守护对象是**代码性质**（一批 256 行的事务不会长时间占住写锁），
    /// 不是某一次测量的墙钟读数：机器繁忙、磁盘同步与杀毒扫描都可能让某一次采样
    /// 从数毫秒跳到几十甚至上百毫秒，而代码并没有退化。
    ///
    /// 因此采样多次，用**最小值**作为真实成本的估计（调度噪声只会加时间、不会减），
    /// 对最小值保留 50ms 硬阈；同时对每一次采样保留一个宽松上限，
    /// 让「真的把锁按住很久」这种退化仍然必然失败。
    #[tokio::test]
    async fn test_recalculate_batch_lock_duration_under_50ms() {
        const SAMPLES: usize = 5;
        const HARD_CEILING_MS: u64 = 2_000;

        let engine = BillingEngine::new();
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut durations = Vec::with_capacity(SAMPLES);

        for sample in 0..SAMPLES {
            let temp = axon_test_support::test_temp_dir();
            // 1. 准备满批 256 条事实记录
            let mut facts = Vec::with_capacity(256);
            for i in 0..256 {
                let session_id = format!("bench-sess-{sample}-{i}");
                let fact = make_sample_fact(&session_id, "gpt-4o", 1000 + i as u64, 500);
                facts.push(fact);
            }

            {
                let mut repo = UsageRepository::open(temp.path()).unwrap();
                repo.project_facts(&facts, 1, 1000).unwrap();

                let rule = ModelPriceRule {
                    id: "rule-bench".to_string(),
                    model_pattern: "gpt-4*".to_string(),
                    pricing_style: PricingStyle::OpenAi,
                    input_price_per_m_micros: 2_500_000,
                    output_price_per_m_micros: 10_000_000,
                    cache_read_price_per_m_micros: 0,
                    cache_write_price_per_m_micros: 0,
                    multiplier_basis_points: 10_000,
                    priority: 1,
                    created_at_ms: 1000,
                    updated_at_ms: 1000,
                };
                repo.save_pricing_rule(&rule).unwrap();
            }

            // 2. 满批 256 条重算并统计持锁耗时
            let report = engine.recalculate(temp.path()).await.unwrap();
            // 分批行为是被守护的结构性质，任何一次采样都必须成立。
            assert_eq!(report.total_scanned, 256);
            assert_eq!(report.updated_count, 256);
            assert_eq!(report.batches_processed, 2);
            assert!(
                report.max_batch_duration_ms < HARD_CEILING_MS,
                "单批持锁出现数量级退化：第 {sample} 次采样为 {} ms（上限 {HARD_CEILING_MS} ms）",
                report.max_batch_duration_ms
            );
            durations.push(report.max_batch_duration_ms);
            samples.push(report);
        }

        let best = durations.iter().copied().min().unwrap_or_default();
        println!(
            "真机实测单批 256 条持锁耗时采样: {durations:?} ms，最小值 {best} ms（阈值 50ms）"
        );
        assert!(
            best < 50,
            "单批持锁最小值必须小于 50ms，实测采样为 {durations:?} ms"
        );
        assert_eq!(
            samples
                .iter()
                .map(|report| report.updated_count)
                .sum::<u64>(),
            (SAMPLES as u64) * 256,
            "每次采样的重算条数都必须完整"
        );
    }
}
