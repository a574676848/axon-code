//! 文件工具使用的非阻塞 LSP 文档触达调度器。

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, TryLockError};
use std::thread;

use serde::Serialize;

use crate::lsp_document::path_to_file_uri;
use crate::runtime_limits::LSP_BACKGROUND_QUEUE_CAPACITY;
use crate::{LspCatalogRuntime, LspClientPoolRuntime};

const LSP_BACKGROUND_WORKER_NAME: &str = "axon-lsp-background";
const SCHEDULE_STATUS_SCHEDULED: &str = "scheduled";
const SCHEDULE_STATUS_REPLACED_OLD_PENDING: &str = "replaced_old_pending";
const SCHEDULE_STATUS_SKIPPED: &str = "skipped";
const SCHEDULE_STATUS_BUSY: &str = "busy";
const SCHEDULE_STATUS_UNAVAILABLE: &str = "unavailable";

fn queue() -> Option<&'static Arc<LspBackgroundQueue>> {
    static QUEUE: OnceLock<Option<Arc<LspBackgroundQueue>>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let queue = Arc::new(LspBackgroundQueue::default());
            let worker_queue = Arc::clone(&queue);
            thread::Builder::new()
                .name(LSP_BACKGROUND_WORKER_NAME.to_owned())
                .spawn(move || worker_loop(&worker_queue))
                .ok()
                .map(|_| queue)
        })
        .as_ref()
}

#[derive(Default)]
struct LspBackgroundQueue {
    pending: Mutex<VecDeque<LspBackgroundJob>>,
    wake: Condvar,
}

#[derive(Clone)]
struct LspBackgroundJob {
    workspace_root: PathBuf,
    path: PathBuf,
}

impl LspBackgroundJob {
    fn same_document(&self, other: &Self) -> bool {
        self.workspace_root == other.workspace_root && self.path == other.path
    }
}

/// 后台 LSP 触达的入队结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspBackgroundSchedule {
    /// 调度状态。
    pub status: &'static str,
    /// 已纳入任务的本机文件。
    pub files: Vec<String>,
    /// 本次被合并的同文档旧任务数。
    pub coalesced: usize,
    /// 为保留最新状态而淘汰的最旧待处理任务数。
    pub evicted: usize,
}

/// 非阻塞 LSP 文档触达 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspBackgroundRuntime;

impl LspBackgroundRuntime {
    /// 将支持 LSP 的现存文件放入有界最新队列；本方法不启动或等待语言服务器。
    #[must_use]
    pub fn schedule(
        workspace_root: &Path,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> LspBackgroundSchedule {
        let paths = normalize_paths(workspace_root, paths);
        let files = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return schedule_report(SCHEDULE_STATUS_SKIPPED, files, 0, 0);
        }
        let Some(queue) = queue() else {
            return schedule_report(SCHEDULE_STATUS_UNAVAILABLE, files, 0, 0);
        };
        let mut pending = match queue.pending.try_lock() {
            Ok(pending) => pending,
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
                return schedule_report(SCHEDULE_STATUS_BUSY, files, 0, 0);
            }
        };
        let workspace_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let mut coalesced = 0_usize;
        let mut evicted = 0_usize;
        for path in paths {
            let job = LspBackgroundJob {
                workspace_root: workspace_root.clone(),
                path,
            };
            let (job_coalesced, job_evicted) = enqueue_latest(&mut pending, job);
            coalesced = coalesced.saturating_add(job_coalesced);
            evicted = evicted.saturating_add(job_evicted);
        }
        drop(pending);
        queue.wake.notify_one();
        let status = if evicted == 0 {
            SCHEDULE_STATUS_SCHEDULED
        } else {
            SCHEDULE_STATUS_REPLACED_OLD_PENDING
        };
        schedule_report(status, files, coalesced, evicted)
    }
}

fn schedule_report(
    status: &'static str,
    files: Vec<String>,
    coalesced: usize,
    evicted: usize,
) -> LspBackgroundSchedule {
    LspBackgroundSchedule {
        status,
        files,
        coalesced,
        evicted,
    }
}

fn worker_loop(queue: &LspBackgroundQueue) {
    loop {
        let job = {
            let mut pending = queue
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while pending.is_empty() {
                pending = queue
                    .wake
                    .wait(pending)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            pending.pop_front()
        };
        if let Some(job) = job {
            touch_document(&job);
        }
    }
}

fn enqueue_latest(
    pending: &mut VecDeque<LspBackgroundJob>,
    job: LspBackgroundJob,
) -> (usize, usize) {
    let coalesced =
        if let Some(index) = pending.iter().position(|queued| queued.same_document(&job)) {
            pending.remove(index);
            1
        } else {
            0
        };
    let evicted = if pending.len() == LSP_BACKGROUND_QUEUE_CAPACITY {
        pending.pop_front();
        1
    } else {
        0
    };
    pending.push_back(job);
    (coalesced, evicted)
}

fn normalize_paths(
    workspace_root: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            }
        })
        .filter(|path| path.is_file() && LspCatalogRuntime::supports_path(path))
        .map(|path| path.canonicalize().unwrap_or(path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn touch_document(job: &LspBackgroundJob) {
    let Ok(uri) = path_to_file_uri(&job.path) else {
        return;
    };
    let Ok(plans) = LspCatalogRuntime::resolve_document_launch_plans(&job.workspace_root, &uri)
    else {
        return;
    };
    for plan in plans {
        let _ = LspClientPoolRuntime::with_client(&plan, |client| {
            client.touch_document(&uri).map(|_| ())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn background_job(workspace: &Path, file_name: impl AsRef<Path>) -> LspBackgroundJob {
        LspBackgroundJob {
            workspace_root: workspace.to_path_buf(),
            path: workspace.join(file_name),
        }
    }

    #[test]
    fn enqueue_latest_coalesces_document_and_moves_it_to_queue_tail() {
        let workspace = PathBuf::from("workspace");
        let first = background_job(&workspace, "first.rs");
        let second = background_job(&workspace, "second.rs");
        let third = background_job(&workspace, "third.rs");
        let mut pending = VecDeque::from([first.clone(), second.clone(), third.clone()]);

        let result = enqueue_latest(&mut pending, first.clone());

        assert_eq!(result, (1, 0));
        assert_eq!(
            pending
                .iter()
                .map(|job| job.path.clone())
                .collect::<Vec<_>>(),
            vec![second.path, third.path, first.path],
        );
    }

    #[test]
    fn enqueue_latest_evicts_oldest_pending_job_when_capacity_is_full() {
        let workspace = PathBuf::from("workspace");
        let mut pending = (0..LSP_BACKGROUND_QUEUE_CAPACITY)
            .map(|index| background_job(&workspace, format!("{index}.rs")))
            .collect::<VecDeque<_>>();
        let oldest_path = pending.front().unwrap().path.clone();
        let second_path = pending.get(1).unwrap().path.clone();
        let newest = background_job(&workspace, "newest.rs");

        let result = enqueue_latest(&mut pending, newest.clone());

        assert_eq!(result, (0, 1));
        assert_eq!(pending.len(), LSP_BACKGROUND_QUEUE_CAPACITY);
        assert!(!pending.iter().any(|job| job.path == oldest_path));
        assert_eq!(pending.front().unwrap().path, second_path);
        assert_eq!(pending.back().unwrap().path, newest.path);
    }

    #[test]
    fn schedule_degrades_to_busy_when_queue_is_contended() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-background-busy");
        std::fs::create_dir_all(&workspace).unwrap();
        let file = workspace.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let queue = queue().expect("LSP 后台队列应可初始化");
        let _pending = queue
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let report = LspBackgroundRuntime::schedule(&workspace, [file]);

        assert_eq!(report.status, SCHEDULE_STATUS_BUSY);
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.coalesced, 0);
        assert_eq!(report.evicted, 0);
    }

    #[test]
    fn schedule_skips_unsupported_files_without_starting_worker() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-background-skip");
        std::fs::create_dir_all(&workspace).unwrap();
        let file = workspace.join("notes.unknown");
        std::fs::write(&file, "content").unwrap();

        let report = LspBackgroundRuntime::schedule(&workspace, [file]);

        assert_eq!(report.status, SCHEDULE_STATUS_SKIPPED);
        assert!(report.files.is_empty());
        assert_eq!(report.coalesced, 0);
        assert_eq!(report.evicted, 0);
    }
}
