//! Personal Memory 运行时质量观测。

use std::time::Duration;

use axon_domain::{AxonError, AxonResult};
use rusqlite::{Connection, params};

use crate::MemoryConfig;

const MEMORY_RUNTIME_QUALITY_BUSY_TIMEOUT_MS: u64 = 5_000;

/// 单个主代理 turn 的幂等质量观测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRuntimeQualityObservation {
    /// turn 级稳定幂等键。
    pub observation_id: String,
    /// Provider 缓存读取 token。
    pub cache_read_input_tokens: u64,
    /// Provider 完整输入 token。
    pub complete_input_tokens: u64,
    /// 实际启动的深反思数量。
    pub reflection_attempts: u64,
    /// 实际产生 Memory 或 Skill 材料的反思数量。
    pub reflection_yields: u64,
    /// 已完成的语义显著性评估数量。
    pub semantic_assessments: u64,
    /// 因 schema、证据或门禁无效而拒绝的 Memory 评估数量。
    pub false_memory_rejections: u64,
    /// 观测时间。
    pub recorded_at_ms: u64,
}

/// 聚合后的 Personal Memory 运行时质量指标。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryRuntimeQualityReport {
    /// 幂等观测数量。
    pub observation_count: u64,
    /// Provider 缓存读取 token 总量。
    pub cache_read_input_tokens: u64,
    /// Provider 完整输入 token 总量。
    pub complete_input_tokens: u64,
    /// Provider Prompt Cache 命中率。
    pub cache_hit_basis_points: u16,
    /// 深反思启动数量。
    pub reflection_attempts: u64,
    /// 深反思有效产出数量。
    pub reflection_yields: u64,
    /// 深反思有效产出率。
    pub reflection_yield_basis_points: u16,
    /// 语义显著性评估数量。
    pub semantic_assessments: u64,
    /// 拒绝的错误 Memory 评估数量。
    pub false_memory_rejections: u64,
    /// 错误 Memory 评估拒绝率。
    pub false_memory_rejection_basis_points: u16,
}

/// Personal Memory 运行时质量 store。
pub struct MemoryRuntimeQualityStore {
    connection: Connection,
}

impl MemoryRuntimeQualityStore {
    /// 打开质量 store。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        let connection = Connection::open(&config.db_path)
            .map_err(|_| AxonError::memory_persistence("无法打开 Memory 质量数据库"))?;
        connection
            .busy_timeout(Duration::from_millis(
                MEMORY_RUNTIME_QUALITY_BUSY_TIMEOUT_MS,
            ))
            .map_err(|_| AxonError::memory_persistence("Memory 质量 busy timeout 配置失败"))?;
        Ok(Self { connection })
    }

    /// 初始化质量观测 schema。
    pub fn initialize_schema(&self) -> AxonResult<()> {
        self.connection
            .execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS memory_runtime_quality (
                    observation_id TEXT PRIMARY KEY,
                    cache_read_input_tokens INTEGER NOT NULL,
                    complete_input_tokens INTEGER NOT NULL,
                    reflection_attempts INTEGER NOT NULL,
                    reflection_yields INTEGER NOT NULL,
                    semantic_assessments INTEGER NOT NULL,
                    false_memory_rejections INTEGER NOT NULL,
                    recorded_at_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_memory_runtime_quality_recorded
                    ON memory_runtime_quality(recorded_at_ms DESC);
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory 质量 schema 初始化失败"))
    }

    /// 按 turn 幂等写入质量观测。
    pub fn record_observation(
        &self,
        observation: &MemoryRuntimeQualityObservation,
    ) -> AxonResult<()> {
        validate_observation(observation)?;
        self.initialize_schema()?;
        self.connection
            .execute(
                r"
                INSERT INTO memory_runtime_quality (
                    observation_id, cache_read_input_tokens, complete_input_tokens,
                    reflection_attempts, reflection_yields, semantic_assessments,
                    false_memory_rejections, recorded_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(observation_id) DO UPDATE SET
                    cache_read_input_tokens = excluded.cache_read_input_tokens,
                    complete_input_tokens = excluded.complete_input_tokens,
                    reflection_attempts = excluded.reflection_attempts,
                    reflection_yields = excluded.reflection_yields,
                    semantic_assessments = excluded.semantic_assessments,
                    false_memory_rejections = excluded.false_memory_rejections,
                    recorded_at_ms = excluded.recorded_at_ms
                ",
                params![
                    observation.observation_id,
                    i64_from_u64(observation.cache_read_input_tokens)?,
                    i64_from_u64(observation.complete_input_tokens)?,
                    i64_from_u64(observation.reflection_attempts)?,
                    i64_from_u64(observation.reflection_yields)?,
                    i64_from_u64(observation.semantic_assessments)?,
                    i64_from_u64(observation.false_memory_rejections)?,
                    i64_from_u64(observation.recorded_at_ms)?,
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 质量观测写入失败"))?;
        Ok(())
    }

    /// 聚合全部运行时质量观测。
    pub fn report(&self) -> AxonResult<MemoryRuntimeQualityReport> {
        self.initialize_schema()?;
        let values = self
            .connection
            .query_row(
                r"
                SELECT COUNT(*),
                       COALESCE(SUM(cache_read_input_tokens), 0),
                       COALESCE(SUM(complete_input_tokens), 0),
                       COALESCE(SUM(reflection_attempts), 0),
                       COALESCE(SUM(reflection_yields), 0),
                       COALESCE(SUM(semantic_assessments), 0),
                       COALESCE(SUM(false_memory_rejections), 0)
                FROM memory_runtime_quality
                ",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .map_err(|_| AxonError::memory_persistence("Memory 质量指标聚合失败"))?;
        quality_report(values)
    }
}

fn validate_observation(observation: &MemoryRuntimeQualityObservation) -> AxonResult<()> {
    if observation.observation_id.trim().is_empty()
        || observation.recorded_at_ms == 0
        || observation.cache_read_input_tokens > observation.complete_input_tokens
        || observation.reflection_yields > observation.reflection_attempts
        || observation.false_memory_rejections > observation.semantic_assessments
    {
        return Err(AxonError::memory_persistence("Memory 质量观测无效"));
    }
    Ok(())
}

fn quality_report(
    values: (i64, i64, i64, i64, i64, i64, i64),
) -> AxonResult<MemoryRuntimeQualityReport> {
    let observation_count = u64_from_i64(values.0)?;
    let cache_read_input_tokens = u64_from_i64(values.1)?;
    let complete_input_tokens = u64_from_i64(values.2)?;
    let reflection_attempts = u64_from_i64(values.3)?;
    let reflection_yields = u64_from_i64(values.4)?;
    let semantic_assessments = u64_from_i64(values.5)?;
    let false_memory_rejections = u64_from_i64(values.6)?;
    Ok(MemoryRuntimeQualityReport {
        observation_count,
        cache_read_input_tokens,
        complete_input_tokens,
        cache_hit_basis_points: axon_domain::ratio_basis_points(
            cache_read_input_tokens,
            complete_input_tokens,
        ),
        reflection_attempts,
        reflection_yields,
        reflection_yield_basis_points: axon_domain::ratio_basis_points(
            reflection_yields,
            reflection_attempts,
        ),
        semantic_assessments,
        false_memory_rejections,
        false_memory_rejection_basis_points: axon_domain::ratio_basis_points(
            false_memory_rejections,
            semantic_assessments,
        ),
    })
}

fn i64_from_u64(value: u64) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory 质量数值溢出"))
}

fn u64_from_i64(value: i64) -> AxonResult<u64> {
    u64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory 质量统计无效"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_aggregates_idempotent_runtime_quality_observations() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-memory-runtime-quality-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = MemoryConfig::from_config_dir(&root, false, 1).unwrap();
        std::fs::create_dir_all(&config.memory_dir).unwrap();
        let store = MemoryRuntimeQualityStore::open(&config).unwrap();
        let observations = [
            observation("turn-1", 60, 100, 1, 1, 1, 0, 1_000),
            observation("turn-2", 20, 100, 1, 0, 1, 1, 2_000),
        ];
        for observation in &observations {
            store.record_observation(observation).unwrap();
        }
        store.record_observation(&observations[1]).unwrap();

        let report = store.report().unwrap();

        assert_eq!(report.observation_count, 2);
        assert_eq!(report.cache_hit_basis_points, 4_000);
        assert_eq!(report.reflection_yield_basis_points, 5_000);
        assert_eq!(report.false_memory_rejection_basis_points, 5_000);
        let _ = std::fs::remove_dir_all(root);
    }

    fn observation(
        observation_id: &str,
        cache_read_input_tokens: u64,
        complete_input_tokens: u64,
        reflection_attempts: u64,
        reflection_yields: u64,
        semantic_assessments: u64,
        false_memory_rejections: u64,
        recorded_at_ms: u64,
    ) -> MemoryRuntimeQualityObservation {
        MemoryRuntimeQualityObservation {
            observation_id: observation_id.to_owned(),
            cache_read_input_tokens,
            complete_input_tokens,
            reflection_attempts,
            reflection_yields,
            semantic_assessments,
            false_memory_rejections,
            recorded_at_ms,
        }
    }
}
