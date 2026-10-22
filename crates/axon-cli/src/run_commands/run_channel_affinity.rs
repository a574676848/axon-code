use std::collections::{HashMap, HashSet};
use std::fs::{self};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use axon_agent_runtime::{
    ChannelHealthRegistry, MetricsRouter, RequestSample, TokenUsageTelemetry,
};
use axon_domain::BreakerPolicy;
use axon_jsonl::ShardedJsonlStore;

const CHANNEL_METRICS_DIR: &str = "channel-metrics";
const CHANNEL_METRICS_FILE_PREFIX: &str = "samples-";
const CHANNEL_METRICS_FILE_SUFFIX: &str = ".jsonl";
const CHANNEL_METRICS_MAX_SHARD_BYTES: u64 = 4 * 1024 * 1024;
const CHANNEL_METRICS_CONNECTION_EVENT: &str = "connection";
/// 渠道指标使用的统一 shard 存储；日期仍作为逻辑 JSONL 的业务分片。
const CHANNEL_METRICS_SHARD_STORE: ShardedJsonlStore =
    ShardedJsonlStore::with_max_shard_bytes(CHANNEL_METRICS_MAX_SHARD_BYTES);

struct AffinityEntry {
    provider: String,
    expires_at_ms: u64,
    ttl_ms: u64,
}

pub(super) struct ChannelAffinityCache {
    entries: HashMap<(String, String), AffinityEntry>,
}

impl ChannelAffinityCache {
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// 查亲和渠道；过期或未命中返回 None。
    pub(super) fn get(&mut self, model_name: &str, session_id: &str, now_ms: u64) -> Option<&str> {
        let key = (model_name.to_owned(), session_id.to_owned());
        let entry = self.entries.get_mut(&key)?;
        if now_ms >= entry.expires_at_ms {
            return None;
        }
        entry.expires_at_ms = now_ms.saturating_add(entry.ttl_ms);
        Some(&entry.provider)
    }

    /// 记录成功渠道，并根据配置的 TTL 刷新过期时间。
    pub(super) fn record(
        &mut self,
        model_name: &str,
        session_id: &str,
        provider: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) {
        let key = (model_name.to_owned(), session_id.to_owned());
        let expires_at_ms = now_ms.saturating_add(ttl_ms);
        self.entries.insert(
            key,
            AffinityEntry {
                provider: provider.to_owned(),
                expires_at_ms,
                ttl_ms,
            },
        );
    }
}

/// 进程级亲和缓存单例（进程退出即销毁，不落盘）。
static AFFINITY_CACHE: LazyLock<Mutex<ChannelAffinityCache>> =
    LazyLock::new(|| Mutex::new(ChannelAffinityCache::new()));
/// 进程级健康注册表单例，按首次访问时的策略惰性初始化。
static HEALTH_REGISTRY: LazyLock<Mutex<Option<ChannelHealthRegistry>>> =
    LazyLock::new(|| Mutex::new(None));
/// 进程级渠道性能指标单例。
static METRICS_ROUTER: LazyLock<Mutex<MetricsRouter>> =
    LazyLock::new(|| Mutex::new(MetricsRouter::new()));

#[cfg(test)]
static SINGLETON_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[cfg(test)]
static SINGLETON_TEST_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 串行化共享渠道单例的用例：并行执行时用例之间会互相重置进程级状态。
///
/// 该锁独立于 [`SINGLETON_TEST_LOCK`]，用例持有期间可继续安全访问单例。
#[cfg(test)]
pub(super) fn channel_singleton_test_guard() -> std::sync::MutexGuard<'static, ()> {
    SINGLETON_TEST_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
fn with_test_singleton_lock<R>(f: impl FnOnce() -> R) -> R {
    let _guard = SINGLETON_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f()
}

#[cfg(not(test))]
fn with_test_singleton_lock<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// 访问进程级亲和缓存。
pub(super) fn with_affinity_cache<R>(f: impl FnOnce(&mut ChannelAffinityCache) -> R) -> R {
    with_test_singleton_lock(|| {
        let mut cache = AFFINITY_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut cache)
    })
}

/// 以指定断路器策略访问进程级健康注册表（首次访问按策略初始化）。
pub(super) fn with_health_registry<R>(
    policy: BreakerPolicy,
    f: impl FnOnce(&mut ChannelHealthRegistry) -> R,
) -> R {
    with_test_singleton_lock(|| {
        let mut guard = HEALTH_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let registry = guard.get_or_insert_with(|| ChannelHealthRegistry::new(policy));
        f(registry)
    })
}

/// 访问进程级渠道性能指标。
pub(crate) fn with_metrics_router<R>(f: impl FnOnce(&mut MetricsRouter) -> R) -> R {
    with_test_singleton_lock(|| {
        let mut metrics = METRICS_ROUTER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut metrics)
    })
}

/// 记录一次渠道请求样本；不持久化 prompt 明文。
pub(crate) fn record_channel_sample(
    config_path: &Path,
    persistence_enabled: bool,
    channel_key: String,
    latency_ms: u64,
    success: bool,
    token_usage: Option<TokenUsageTelemetry>,
    timestamp_ms: u64,
) {
    let usage = token_usage.unwrap_or_default();
    let cache_hit_rate_basis_points = usage.cache_hit_rate_basis_points();
    axon_logger::debug(&format!(
        "channel_metrics provider_key={channel_key:?} latency_ms={latency_ms} success={success} input_tokens={} output_tokens={} cache_hit_rate_bps={cache_hit_rate_basis_points}",
        usage.complete_input_tokens(),
        usage.output_tokens
    ));
    with_metrics_router(|metrics| {
        metrics.record_sample(RequestSample {
            channel_key: channel_key.clone(),
            latency_ms,
            success,
            input_tokens: usage.complete_input_tokens(),
            output_tokens: usage.output_tokens,
            total_cost: 0.0,
            timestamp_ms,
        });
        metrics.record_cache_hit_rate(&channel_key, cache_hit_rate_basis_points, timestamp_ms);
    });
    let sample = PersistedChannelSample {
        channel_key,
        latency_ms,
        success,
        input_tokens: usage.complete_input_tokens(),
        output_tokens: usage.output_tokens,
        cache_hit_rate_basis_points,
        timestamp_ms,
        event_type: None,
        connection_reused: None,
        connection_age_ms: None,
        fallback_activated: false,
    };
    if persistence_enabled && let Err(error) = append_channel_sample(config_path, &sample) {
        axon_logger::warning(&format!("渠道指标落盘失败：{error}"));
    }
}

/// 返回渠道指标分片目录，供 debug 和 cron 复用。
pub(crate) fn channel_metrics_path(config_path: &Path) -> Result<PathBuf, String> {
    let data_dir =
        crate::runtime_paths::runtime_axon_data_path_static(config_path).map_err(str::to_owned)?;
    Ok(data_dir.join(CHANNEL_METRICS_DIR))
}

/// 按查询条件从日期分片重建渠道指标。
pub(crate) fn load_channel_metrics(
    config_path: &Path,
    since_ms: Option<u64>,
    model_filter: Option<&str>,
    provider_filter: Option<&str>,
    max_samples_per_channel: usize,
    max_channels: usize,
) -> Result<MetricsRouter, String> {
    let directory = channel_metrics_path(config_path)?;
    let mut paths = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (name.starts_with(CHANNEL_METRICS_FILE_PREFIX)
                    && name.ends_with(CHANNEL_METRICS_FILE_SUFFIX))
                .then_some(entry.path())
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MetricsRouter::new());
        }
        Err(error) => return Err(format!("读取渠道指标目录失败：{error}")),
    };
    paths.sort();
    paths.reverse();
    let mut router = MetricsRouter::new();
    let mut admitted_channels = HashSet::new();
    for path in paths {
        if max_channels == 0 {
            break;
        }
        if shard_is_before_cutoff(&path, since_ms) {
            continue;
        }
        let file =
            fs::File::open(&path).map_err(|error| format!("打开渠道指标文件失败：{error}"))?;
        for (line_number, line) in BufReader::new(file).lines().enumerate() {
            let line = line
                .map_err(|error| format!("读取渠道指标第 {} 行失败：{error}", line_number + 1))?;
            if line.trim().is_empty() {
                continue;
            }
            let sample: PersistedChannelSample = serde_json::from_str(&line)
                .map_err(|error| format!("解析渠道指标第 {} 行失败：{error}", line_number + 1))?;
            if sample.event_type.is_some() {
                continue;
            }
            if !channel_sample_matches(&sample, since_ms, model_filter, provider_filter) {
                continue;
            }
            let current_count = router
                .get_metrics(&sample.channel_key)
                .map_or(0, |metrics| metrics.sample_count as usize);
            if current_count == 0
                && admitted_channels.len() >= max_channels
                && !admitted_channels.contains(&sample.channel_key)
            {
                continue;
            }
            if current_count >= max_samples_per_channel {
                continue;
            }
            admitted_channels.insert(sample.channel_key.clone());
            router.record_sample(RequestSample {
                channel_key: sample.channel_key.clone(),
                latency_ms: sample.latency_ms,
                success: sample.success,
                input_tokens: sample.input_tokens,
                output_tokens: sample.output_tokens,
                total_cost: 0.0,
                timestamp_ms: sample.timestamp_ms,
            });
            router.record_cache_hit_rate(
                &sample.channel_key,
                sample.cache_hit_rate_basis_points,
                sample.timestamp_ms,
            );
        }
    }
    Ok(router)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ChannelConnectionMetrics {
    pub observation_count: u32,
    pub reusable_observation_count: u32,
    pub reused_connection_count: u32,
    pub fallback_count: u32,
    pub connection_age_total_ms: u64,
    pub connection_age_count: u32,
}

impl ChannelConnectionMetrics {
    fn record(&mut self, reused: Option<bool>, age_ms: Option<u64>, fallback: bool) {
        self.observation_count = self.observation_count.saturating_add(1);
        if let Some(reused) = reused {
            self.reusable_observation_count = self.reusable_observation_count.saturating_add(1);
            if reused {
                self.reused_connection_count = self.reused_connection_count.saturating_add(1);
            }
        }
        if fallback {
            self.fallback_count = self.fallback_count.saturating_add(1);
        }
        if let Some(age_ms) = age_ms {
            self.connection_age_total_ms = self.connection_age_total_ms.saturating_add(age_ms);
            self.connection_age_count = self.connection_age_count.saturating_add(1);
        }
    }
}

pub(crate) fn load_channel_connection_metrics(
    config_path: &Path,
    since_ms: Option<u64>,
    model_filter: Option<&str>,
    provider_filter: Option<&str>,
    max_channels: usize,
) -> Result<HashMap<String, ChannelConnectionMetrics>, String> {
    let directory = channel_metrics_path(config_path)?;
    let mut paths = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (name.starts_with(CHANNEL_METRICS_FILE_PREFIX)
                    && name.ends_with(CHANNEL_METRICS_FILE_SUFFIX))
                .then_some(entry.path())
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(format!("读取渠道指标目录失败：{error}")),
    };
    paths.sort();
    paths.reverse();
    let mut metrics: HashMap<String, ChannelConnectionMetrics> = HashMap::new();
    for path in paths {
        if max_channels == 0 {
            break;
        }
        if shard_is_before_cutoff(&path, since_ms) {
            continue;
        }
        let file =
            fs::File::open(&path).map_err(|error| format!("打开渠道指标文件失败：{error}"))?;
        for (line_number, line) in BufReader::new(file).lines().enumerate() {
            let line = line
                .map_err(|error| format!("读取渠道指标第 {} 行失败：{error}", line_number + 1))?;
            if line.trim().is_empty() {
                continue;
            }
            let sample: PersistedChannelSample = serde_json::from_str(&line)
                .map_err(|error| format!("解析渠道指标第 {} 行失败：{error}", line_number + 1))?;
            if sample.event_type.as_deref() != Some(CHANNEL_METRICS_CONNECTION_EVENT)
                || !channel_sample_matches(&sample, since_ms, model_filter, provider_filter)
            {
                continue;
            }
            if !metrics.contains_key(&sample.channel_key) && metrics.len() >= max_channels {
                continue;
            }
            metrics.entry(sample.channel_key).or_default().record(
                sample.connection_reused,
                sample.connection_age_ms,
                sample.fallback_activated,
            );
        }
    }
    Ok(metrics)
}

fn channel_sample_matches(
    sample: &PersistedChannelSample,
    since_ms: Option<u64>,
    model_filter: Option<&str>,
    provider_filter: Option<&str>,
) -> bool {
    if since_ms.is_some_and(|cutoff| sample.timestamp_ms < cutoff) {
        return false;
    }
    let Some((model, provider)) = sample.channel_key.split_once('|') else {
        return model_filter.is_none() && provider_filter.is_none();
    };
    model_filter.is_none_or(|expected| expected == model)
        && provider_filter.is_none_or(|expected| expected == provider)
}

fn shard_is_before_cutoff(path: &Path, since_ms: Option<u64>) -> bool {
    let Some(cutoff_ms) = since_ms else {
        return false;
    };
    let cutoff_date = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
        i64::try_from(cutoff_ms).unwrap_or(i64::MAX),
    )
    .map(|value| value.format("%Y-%m-%d").to_string());
    let Some(cutoff_date) = cutoff_date else {
        return false;
    };
    let Some(file_date) = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(CHANNEL_METRICS_FILE_PREFIX))
        .and_then(|name| name.split_once('.').map(|(date, _)| date))
    else {
        return false;
    };
    file_date < cutoff_date.as_str()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedChannelSample {
    channel_key: String,
    latency_ms: u64,
    success: bool,
    input_tokens: u64,
    output_tokens: u64,
    cache_hit_rate_basis_points: u16,
    timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    connection_reused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    connection_age_ms: Option<u64>,
    #[serde(default)]
    fallback_activated: bool,
}

pub(crate) fn record_channel_connection_observation(
    config_path: &Path,
    persistence_enabled: bool,
    channel_key: String,
    connection_reused: Option<bool>,
    connection_age_ms: Option<u64>,
    fallback_activated: bool,
    timestamp_ms: u64,
) {
    if !persistence_enabled {
        return;
    }
    let sample = PersistedChannelSample {
        channel_key,
        latency_ms: 0,
        success: true,
        input_tokens: 0,
        output_tokens: 0,
        cache_hit_rate_basis_points: 0,
        timestamp_ms,
        event_type: Some(CHANNEL_METRICS_CONNECTION_EVENT.to_owned()),
        connection_reused,
        connection_age_ms,
        fallback_activated,
    };
    if let Err(error) = append_channel_sample(config_path, &sample) {
        axon_logger::warning(&format!("连接遥测落盘失败：{error}"));
    }
}

fn append_channel_sample(
    config_path: &Path,
    sample: &PersistedChannelSample,
) -> Result<(), String> {
    let directory = channel_metrics_path(config_path)?;
    let line =
        serde_json::to_string(sample).map_err(|error| format!("序列化渠道指标失败：{error}"))?;
    let date = channel_metrics_date(sample.timestamp_ms);
    CHANNEL_METRICS_SHARD_STORE
        .append_line(
            channel_metrics_logical_path(&directory, &date),
            line.as_bytes(),
        )
        .map_err(|error| format!("追加渠道指标失败：{error}"))
}

fn channel_metrics_logical_path(directory: &Path, date: &str) -> PathBuf {
    directory.join(format!(
        "{CHANNEL_METRICS_FILE_PREFIX}{date}{CHANNEL_METRICS_FILE_SUFFIX}"
    ))
}

fn channel_metrics_date(timestamp_ms: u64) -> String {
    let timestamp_ms = i64::try_from(timestamp_ms).unwrap_or(i64::MAX);
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
pub(super) fn reset_channel_singletons_for_test() {
    with_test_singleton_lock(|| {
        *AFFINITY_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ChannelAffinityCache::new();
        *HEALTH_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *METRICS_ROUTER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = MetricsRouter::new();
    });
}

#[cfg(test)]
#[path = "run_channel_affinity_tests.rs"]
mod tests;
