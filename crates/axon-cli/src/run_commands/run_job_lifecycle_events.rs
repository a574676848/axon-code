//! 把临时后台作业的生命周期落到会话事件流与指标投影。
//!
//! 作业注册表在 `axon-agent-runtime` 内，是进程内事实源；本模块订阅它的启停事件，
//! 按作业归属的运行会话做两件事：追加 `job_started` / `job_completed` 帧，并把
//! 「在跑的作业数」投影进 `session_metrics`（会话列表的 `openJobCount` 直接读它）。
//!
//! 三条硬约束：
//!
//! 1. 写失败只记日志，绝不反向影响作业执行：作业已起或已停是既有事实，落盘只是观测。
//! 2. 事件按作业归属的会话落，不落到当前进程的「最近会话」上：并发会话下不会串味。
//! 3. 同一段运行窗口已有车道认领时不再补登会话行，避免清单里出现重复空壳会话。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, OnceLock},
};

use axon_agent_runtime::job_runtime::{
    JobLifecycleEvent, JobLifecycleSubscription, JobRuntime, JobTerminalKind,
    subscribe_job_lifecycle,
};
use axon_domain::{DaemonSessionId, RuntimeHostId};
use axon_eventlog::{JobOutcome, JobRef};
use axon_session::{EventLogStore as _, RuntimeStore as _};

/// 事件类型：作业启动。
const JOB_STARTED_EVENT: &str = "job_started";
/// 事件类型：作业进入终态。
const JOB_COMPLETED_EVENT: &str = "job_completed";
/// CLI 侧写事件时申请的 host 租约时长。
const EVENT_HOST_LEASE_MS: u64 = 60_000;

static INSTALLED_CONFIG: OnceLock<PathBuf> = OnceLock::new();
static SUBSCRIPTION: OnceLock<JobLifecycleSubscription> = OnceLock::new();
/// 本轮开跑以来各会话启动的作业数；每轮收口取走并清零。
static STARTED_SINCE_SETTLEMENT: LazyLock<Mutex<HashMap<String, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 安装一次进程级订阅；重复调用是幂等空操作。
///
/// 订阅句柄存在静态里，因此它在进程生命周期内不会被丢弃：事件源是全局注册表，
/// 退订只会让观测断流，不会让作业停跑。
pub(crate) fn install(config_path: &Path) {
    if INSTALLED_CONFIG.set(config_path.to_path_buf()).is_err() {
        return;
    }
    let path = config_path.to_path_buf();
    let subscription = subscribe_job_lifecycle(Arc::new(move |event| {
        record_lifecycle_event(&path, event);
    }));
    let _ = SUBSCRIPTION.set(subscription);
}

/// 取走本轮开跑以来该会话启动的作业数（取走后清零）。
pub(crate) fn take_started_count(session_id: &str) -> u32 {
    STARTED_SINCE_SETTLEMENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id)
        .unwrap_or(0)
}

/// 当前仍在运行的作业数；从注册表读，不依赖本模块的计数。
pub(crate) fn running_count(session_id: &str) -> u32 {
    u32::try_from(JobRuntime::global().pending_job_ids(session_id).len()).unwrap_or(u32::MAX)
}

struct LifecycleRecord {
    session_id: String,
    kind: &'static str,
    payload: String,
    started: bool,
    failed: bool,
}

fn record_lifecycle_event(config_path: &Path, event: &JobLifecycleEvent) {
    let Some(record) = lifecycle_record(event) else {
        return;
    };
    persist(config_path, record);
}

fn lifecycle_record(event: &JobLifecycleEvent) -> Option<LifecycleRecord> {
    match event {
        JobLifecycleEvent::Started(started) => {
            let reference = JobRef {
                job_id: started.job_id.to_string(),
                kind: started.producer.as_str().to_owned(),
                owner: started.owner_id.clone(),
                started_at_ms: started.started_at_ms,
            };
            match serde_json::to_string(&reference) {
                Ok(payload) => Some(LifecycleRecord {
                    session_id: started.owner_id.clone(),
                    kind: JOB_STARTED_EVENT,
                    payload,
                    started: true,
                    failed: false,
                }),
                Err(error) => {
                    axon_logger::warning(&format!("job_started 事件序列化失败：{error}"));
                    None
                }
            }
        }
        JobLifecycleEvent::Completed(completed) => {
            let outcome = JobOutcome {
                job_id: completed.job_id.to_string(),
                status: completed.terminal.as_str().to_owned(),
                elapsed_ms: completed.elapsed_ms,
                exit_code: completed.exit_code,
                terminal_reason: completed.reason.clone(),
            };
            let failed = matches!(
                completed.terminal,
                JobTerminalKind::Failed | JobTerminalKind::Timeout | JobTerminalKind::Unresponsive
            );
            match serde_json::to_string(&outcome) {
                Ok(payload) => Some(LifecycleRecord {
                    session_id: completed.owner_id.clone(),
                    kind: JOB_COMPLETED_EVENT,
                    payload,
                    started: false,
                    failed,
                }),
                Err(error) => {
                    axon_logger::warning(&format!("job_completed 事件序列化失败：{error}"));
                    None
                }
            }
        }
    }
}

fn persist(config_path: &Path, record: LifecycleRecord) {
    // 计数与落盘在同一入口维护：本轮指标只读这份计数，不再自己推。
    if record.started {
        let mut counts = STARTED_SINCE_SETTLEMENT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = counts.entry(record.session_id.clone()).or_insert(0);
        *entry = entry.saturating_add(1);
    }
    let Ok(session_id) = DaemonSessionId::new(record.session_id.clone()) else {
        axon_logger::warning("作业事件归属的会话 ID 不合法，跳过落盘");
        return;
    };
    let Ok(store) = crate::run_commands::run_agent_store::shared_runtime_store(config_path) else {
        return;
    };
    let now_ms = axon_time::unix_now_ms();
    // 作业可能来自尚未登记会话行的直连运行：补登一次，已被车道认领时不重复登记。
    if store.get_session(&session_id).is_err()
        && !crate::session_commands::run_window_has_lane(&store, session_id.as_str())
    {
        let cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = store.create_session(&session_id, &cwd, now_ms);
    }
    // 指标投影与事件落盘走同一份事实：任一侧失败都只影响观测，不影响作业。
    if let Err(error) = store.apply_job_delta(&session_id, record.started, record.failed, now_ms) {
        axon_logger::warning(&format!("作业计数投影失败：{error}"));
    }
    let Ok(host_id) = RuntimeHostId::new(format!("cli-job-{}", std::process::id())) else {
        return;
    };
    let Ok(host) = store.claim_host(
        &host_id,
        std::process::id(),
        None,
        now_ms,
        EVENT_HOST_LEASE_MS,
    ) else {
        axon_logger::warning("作业事件写入未能取得 host 租约，跳过本次落盘");
        return;
    };
    if let Err(error) = store.append_events(
        &session_id,
        &host,
        &[(record.kind.to_owned(), record.payload)],
        now_ms,
    ) {
        axon_logger::warning(&format!("{} 事件写入失败：{error}", record.kind));
    }
}

#[cfg(test)]
#[path = "run_job_lifecycle_events_tests.rs"]
mod tests;
