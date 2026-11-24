//! Post-run governance 持久化任务入口。

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use axon_process::configure_detached_process;
use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireGovernanceStatus, WireLifecycleStatus,
    WireSchemaVersion, WireSessionRef,
};

use super::{
    RunRequest, run_memory_usefulness::RunMemoryUsefulnessEvent,
    run_post_run_governance_store::GovernanceQueue,
    run_post_run_governance_worker::serialize_governance_job, run_session_store::RunSessionRecord,
};

const POST_RUN_GOVERNANCE_STATUS_STARTED: &str = "started";
const POST_RUN_GOVERNANCE_STATUS_FAILED: &str = "failed";
const POST_RUN_GOVERNANCE_EVENT_ID_SUFFIX: &str = "status";
const POST_RUN_GOVERNANCE_WORKER_COMMAND: &str = "__governance-worker";
const POST_RUN_GOVERNANCE_CONFIG_ARG: &str = "--config-path";

/// Post-run governance 启动回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PostRunGovernanceReceipt {
    pub(super) job_id: String,
    pub(super) status: &'static str,
    pub(super) status_path: Option<String>,
    pub(super) reason: Option<String>,
}

impl PostRunGovernanceReceipt {
    pub(super) fn to_wire_event(
        &self,
        session: WireSessionRef,
        sequence: u64,
        recorded_at_ms: i64,
    ) -> WireEventEnvelope {
        WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!(
                "{}:{}",
                self.job_id, POST_RUN_GOVERNANCE_EVENT_ID_SUFFIX
            )),
            correlation_id: WireCorrelationId(self.job_id.clone()),
            session,
            sequence,
            recorded_at_ms,
            kind: WireEventKind::GovernanceStatus(WireGovernanceStatus {
                job_id: self.job_id.clone(),
                status: governance_status_to_wire(self.status),
                status_path: self.status_path.clone(),
                reason: self.reason.clone(),
            }),
        }
    }
}

/// Post-run governance 持久化任务输入。
pub(super) struct PostRunGovernanceJob {
    pub(super) config_path: PathBuf,
    pub(super) request: RunRequest,
    pub(super) session_record: RunSessionRecord,
    pub(super) memory_usefulness: Option<RunMemoryUsefulnessEvent>,
}

/// 将 post-run governance 任务持久化并启动独立 worker。
pub(super) fn start_post_run_governance_job(
    job: &PostRunGovernanceJob,
) -> PostRunGovernanceReceipt {
    let job_id = governance_job_id(
        &job.request.session_id,
        &job.session_record.assistant_part_id,
    );
    let result = enqueue_and_spawn_worker(&job_id, job);
    match result {
        Ok(status_path) => PostRunGovernanceReceipt {
            job_id,
            status: POST_RUN_GOVERNANCE_STATUS_STARTED,
            status_path: Some(status_path),
            reason: None,
        },
        Err(reason) => PostRunGovernanceReceipt {
            job_id,
            status: POST_RUN_GOVERNANCE_STATUS_FAILED,
            status_path: None,
            reason: Some(reason),
        },
    }
}

fn enqueue_and_spawn_worker(job_id: &str, job: &PostRunGovernanceJob) -> Result<String, String> {
    let payload_json = serialize_governance_job(job)?;
    let queue = GovernanceQueue::open(&job.config_path)?;
    queue.enqueue(
        job_id,
        &job.request.session_id,
        &payload_json,
        axon_time::unix_now_ms(),
    )?;
    spawn_governance_worker(&job.config_path)?;
    Ok(queue.path().to_string_lossy().to_string())
}

fn spawn_governance_worker(config_path: &Path) -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|_| "governance worker 可执行文件解析失败".to_owned())?;
    let runtime_root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|error| format!("governance worker 运行态根目录解析失败：{error:?}"))?;
    let mut command = Command::new(executable);
    command
        .arg(POST_RUN_GOVERNANCE_WORKER_COMMAND)
        .arg(POST_RUN_GOVERNANCE_CONFIG_ARG)
        .arg(config_path)
        // 子进程必须继承同一个运行态根目录，否则服务账户会写到自己的用户目录。
        .env(axon_config::AXON_RUNTIME_ROOT_ENV, runtime_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detached_process(&mut command);
    let child = command
        .spawn()
        .map_err(|error| format!("governance worker 启动失败：{error}"))?;
    reap_governance_worker(child)
}

fn reap_governance_worker(child: std::process::Child) -> Result<(), String> {
    // Keep ownership recoverable if the OS cannot create the reaper thread. Dropping a
    // Child without waiting leaves the worker as a zombie under the daemon on Unix.
    let child = Arc::new(Mutex::new(Some(child)));
    let reaper_child = Arc::clone(&child);
    std::thread::Builder::new()
        .name("axon-governance-worker-reaper".to_owned())
        .spawn(move || {
            if let Some(mut child) = reaper_child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                let _ = child.wait();
            }
        })
        .map(|_| ())
        .map_err(|error| {
            if let Some(mut child) = child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                let _ = child.wait();
            }
            format!("governance worker 回收线程启动失败：{error}")
        })
}

fn governance_job_id(session_id: &str, assistant_part_id: &str) -> String {
    format!("post-run-governance:{session_id}:{assistant_part_id}")
}

fn governance_status_to_wire(status: &str) -> WireLifecycleStatus {
    match status {
        POST_RUN_GOVERNANCE_STATUS_STARTED => WireLifecycleStatus::Started,
        _ => WireLifecycleStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn governance_worker_reaper_waits_for_child() {
        let child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        reap_governance_worker(child).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let output = Command::new("ps")
                .args(["-o", "state=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            assert!(!state.split_whitespace().any(|value| value.starts_with('Z')));
            if state.trim().is_empty() || std::time::Instant::now() >= deadline {
                assert!(
                    state.trim().is_empty(),
                    "governance worker remained: {state}"
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn governance_job_id_uses_turn_level_idempotency_key() {
        assert_eq!(
            governance_job_id("session-1", "assistant-part-2"),
            "post-run-governance:session-1:assistant-part-2"
        );
    }

    #[test]
    fn receipt_projects_started_status_to_wire_event() {
        let receipt = PostRunGovernanceReceipt {
            job_id: "post-run-governance:session-1:assistant-part-2".to_owned(),
            status: POST_RUN_GOVERNANCE_STATUS_STARTED,
            status_path: Some("governance.sqlite3".to_owned()),
            reason: None,
        };
        let event = receipt.to_wire_event(
            WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            4,
            1_000,
        );
        assert!(matches!(
            event.kind,
            WireEventKind::GovernanceStatus(WireGovernanceStatus {
                status: WireLifecycleStatus::Started,
                ..
            })
        ));
    }
}
