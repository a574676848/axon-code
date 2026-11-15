//! 进程资源遥测：定期采样自身常驻内存与 CPU，超阈值时按 WARN 等级输出结构化遥测。
//!
//! 内存异常增长（例如唤醒路由表只增不删、事件正文滞留）过去只能在进程被拖垮之后
//! 回看日志，而日志里没有任何量化读数，无法判断是「真的泄漏」还是「上下文本来就大」。
//! 这里补上量化面：每 `RESOURCE_SAMPLE_INTERVAL` 采样一次，样本可选落盘，
//! 越过高水位并连续 `RESOURCE_HIGH_STREAK` 次后输出一条 WARN 遥测，事后可按时间轴
//! 还原增长曲线。
//!
//! 高水位口径与桌面端 UI 看门狗一致：物理内存的八分之一，夹在 768 MiB 与 4 GiB 之间。
//! 两者判据同源，避免同一台机器上出现两套互相矛盾的「内存高危」标准。

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, System};

/// 采样间隔：只做趋势观测，不需要高频。
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
/// 内存高水位下限：小内存机器上低于此值的内存占用仍然正常。
const RESOURCE_RSS_FLOOR_BYTES: u64 = 768 * 1024 * 1024;
/// 内存高水位上限：大内存机器上也只把 4 GiB 当分界，避免阈值随物理内存无限抬高。
const RESOURCE_RSS_CEILING_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 单核 CPU 占用高水位（百分比）。
const RESOURCE_CPU_HIGH_PERCENT: f32 = 80.0;
/// 连续越线次数达到该值才告警，避免单次抖动刷屏。
const RESOURCE_HIGH_STREAK: u32 = 3;
/// 样本目录名，与 `channel-metrics` 并列。
pub(crate) const RESOURCE_METRICS_DIR_NAME: &str = "resource-metrics";
const RESOURCE_METRICS_FILE_PREFIX: &str = "samples-";
const RESOURCE_METRICS_FILE_SUFFIX: &str = ".jsonl";
const RESOURCE_METRICS_MAX_SHARD_BYTES: u64 = 4 * 1024 * 1024;
const RESOURCE_METRICS_SHARD_STORE: ShardedJsonlStore =
    ShardedJsonlStore::with_max_shard_bytes(RESOURCE_METRICS_MAX_SHARD_BYTES);

/// 一次进程资源采样。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceSample {
    /// 采样时刻（毫秒时间戳）。
    pub sampled_at_ms: u64,
    /// 进程标识。
    pub pid: u32,
    /// 常驻内存字节数。
    pub rss_bytes: u64,
    /// 自上次采样以来的平均 CPU 占用（单核百分比）。
    pub cpu_percent: f32,
    /// 物理内存总量。
    pub total_memory_bytes: u64,
    /// 当前生效的内存高水位。
    pub high_water_bytes: u64,
    /// 常驻内存占物理内存的比例（百分比）。
    pub rss_ratio_percent: f32,
}

impl ResourceSample {
    /// 是否越过内存高水位。
    fn rss_high(&self) -> bool {
        self.rss_bytes >= self.high_water_bytes
    }

    /// 是否越过 CPU 高水位。
    fn cpu_high(&self) -> bool {
        self.cpu_percent >= RESOURCE_CPU_HIGH_PERCENT
    }
}

/// 启动守护进程内的资源遥测线程。
///
/// 样本始终落盘：这是事后唯一能还原内存曲线的量化证据，体积也很小（30 秒一条，
/// 单日不足 1 MiB），不值得为它再增加一个配置面。进程退出即线程结束，
/// 不需要额外停止信号，退出时最多丢掉最后一个未完成的采样。
pub(crate) fn spawn_reporter(axon_root: PathBuf) {
    let spawned = thread::Builder::new()
        .name("axon-resource-telemetry".to_owned())
        .spawn(move || run_reporter(axon_root));
    if let Err(error) = spawned {
        axon_logger::warning(&format!("资源遥测线程启动失败：{error}"));
    }
}

fn run_reporter(axon_root: PathBuf) {
    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    let mut rss_streak = 0_u32;
    let mut cpu_streak = 0_u32;
    // sysinfo 的 CPU 占用是「距上次刷新的区间平均」，首次刷新没有基线，读数恒为 0。
    let mut cpu_baseline_ready = false;
    loop {
        thread::sleep(RESOURCE_SAMPLE_INTERVAL);
        let Some(sample) = take_sample(&mut system, pid) else {
            continue;
        };
        if let Err(error) = append_sample(&axon_root, &sample) {
            axon_logger::warning(&format!("资源遥测落盘失败：{error}"));
        }
        rss_streak = if sample.rss_high() {
            rss_streak.saturating_add(1)
        } else {
            0
        };
        cpu_streak = if cpu_baseline_ready && sample.cpu_high() {
            cpu_streak.saturating_add(1)
        } else {
            0
        };
        cpu_baseline_ready = true;
        if rss_streak >= RESOURCE_HIGH_STREAK || cpu_streak >= RESOURCE_HIGH_STREAK {
            report_high_water(&sample, rss_streak, cpu_streak);
            // 告警后重新计数：越线状态持续时按告警间隔重复播报，而不是每轮刷。
            rss_streak = 0;
            cpu_streak = 0;
        }
    }
}

/// 采集一次自身资源读数。
fn take_sample(system: &mut System, pid: Pid) -> Option<ResourceSample> {
    system.refresh_memory();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let process = system.process(pid)?;
    let rss_bytes = process.memory();
    let cpu_percent = process.cpu_usage();
    let total_memory_bytes = system.total_memory();
    Some(ResourceSample {
        sampled_at_ms: current_ms(),
        pid: pid.as_u32(),
        rss_bytes,
        cpu_percent,
        total_memory_bytes,
        high_water_bytes: memory_high_water(total_memory_bytes),
        rss_ratio_percent: ratio_percent(rss_bytes, total_memory_bytes),
    })
}

/// 内存高水位：物理内存的八分之一，夹在 768 MiB 与 4 GiB 之间。
fn memory_high_water(total_memory_bytes: u64) -> u64 {
    if total_memory_bytes == 0 {
        return RESOURCE_RSS_FLOOR_BYTES;
    }
    let eighth = total_memory_bytes / 8;
    eighth.clamp(RESOURCE_RSS_FLOOR_BYTES, RESOURCE_RSS_CEILING_BYTES)
}

fn ratio_percent(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    // 保留一位小数足够趋势判断，避免浮点噪声让同一状态产生不同读数。
    let value = (part as f64 / whole as f64) * 100.0;
    (value * 10.0).round() as f32 / 10.0
}

fn report_high_water(sample: &ResourceSample, rss_streak: u32, cpu_streak: u32) {
    let payload = serde_json::to_string(&sample).unwrap_or_else(|_| "{}".to_owned());
    let reason = if sample.rss_high() && sample.cpu_high() {
        "内存与 CPU 同时越线"
    } else if sample.rss_high() {
        "内存越线"
    } else {
        "CPU 越线"
    };
    axon_logger::warning(&format!(
        "[resource-telemetry] {reason}：rss={}MiB（{:.1}% 物理内存，阈值 {}MiB），cpu={:.1}%，连续 rss={rss_streak} cpu={cpu_streak}，样本={payload}",
        sample.rss_bytes / (1024 * 1024),
        sample.rss_ratio_percent,
        sample.high_water_bytes / (1024 * 1024),
        sample.cpu_percent
    ));
}

fn append_sample(axon_root: &Path, sample: &ResourceSample) -> Result<(), String> {
    let directory = axon_root
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(RESOURCE_METRICS_DIR_NAME);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("创建资源遥测目录失败：{error}"))?;
    let line =
        serde_json::to_string(sample).map_err(|error| format!("序列化资源样本失败：{error}"))?;
    let date = sample_date(sample.sampled_at_ms);
    RESOURCE_METRICS_SHARD_STORE
        .append_line(logical_path(&directory, &date), line.as_bytes())
        .map_err(|error| format!("追加资源样本失败：{error}"))
}

fn logical_path(directory: &Path, date: &str) -> PathBuf {
    directory.join(format!(
        "{RESOURCE_METRICS_FILE_PREFIX}{date}{RESOURCE_METRICS_FILE_SUFFIX}"
    ))
}

fn sample_date(timestamp_ms: u64) -> String {
    let timestamp_ms = i64::try_from(timestamp_ms).unwrap_or(i64::MAX);
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn current_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 高水位按物理内存八分之一计算，并夹在既定上下限内。
    #[test]
    fn high_water_clamps_between_floor_and_ceiling() {
        assert_eq!(memory_high_water(0), RESOURCE_RSS_FLOOR_BYTES);
        assert_eq!(
            memory_high_water(1024 * 1024 * 1024),
            RESOURCE_RSS_FLOOR_BYTES
        );
        assert_eq!(
            memory_high_water(16 * 1024 * 1024 * 1024),
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(
            memory_high_water(64 * 1024 * 1024 * 1024),
            RESOURCE_RSS_CEILING_BYTES
        );
    }

    /// 越线判定同时看内存与 CPU 两个维度。
    #[test]
    fn high_water_detection_covers_both_dimensions() {
        let mut sample = ResourceSample {
            sampled_at_ms: 1,
            pid: 1,
            rss_bytes: 100,
            cpu_percent: 0.0,
            total_memory_bytes: 1024,
            high_water_bytes: 200,
            rss_ratio_percent: 9.8,
        };
        assert!(!sample.rss_high() && !sample.cpu_high());

        sample.rss_bytes = 200;
        assert!(sample.rss_high(), "达到阈值即算越线");

        sample.rss_bytes = 0;
        sample.cpu_percent = RESOURCE_CPU_HIGH_PERCENT;
        assert!(sample.cpu_high());
    }

    /// 样本能以 camelCase 落盘并原样回读，供事后按时间轴分析。
    #[test]
    fn samples_round_trip_through_json() {
        let sample = ResourceSample {
            sampled_at_ms: 1_700_000_000_000,
            pid: 42,
            rss_bytes: 512 * 1024 * 1024,
            cpu_percent: 12.5,
            total_memory_bytes: 16 * 1024 * 1024 * 1024,
            high_water_bytes: 2 * 1024 * 1024 * 1024,
            rss_ratio_percent: 3.1,
        };
        let line = serde_json::to_string(&sample).expect("序列化");
        assert!(
            line.contains("\"rssBytes\""),
            "落盘字段为 camelCase：{line}"
        );
        let decoded: ResourceSample = serde_json::from_str(&line).expect("回读");
        assert_eq!(decoded, sample);
    }

    /// 落盘路径按日期分片，避免单文件无限增长。
    #[test]
    fn samples_are_sharded_by_date() {
        let directory = PathBuf::from("root");
        let path = logical_path(&directory, "2026-11-15");
        assert_eq!(path, PathBuf::from("root").join("samples-2026-11-15.jsonl"));
    }
}
