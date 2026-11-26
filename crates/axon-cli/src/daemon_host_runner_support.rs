use axon_session::SqliteRuntimeStore;
use axon_session_runtime::{EventPublisher, RuntimeClock};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use super::{SERVICE_STOP_WAKEUP_POLL_MS, now_ms};
use crate::daemon_commands::daemon_service_manager;

pub struct DaemonPidFileGuard {
    path: PathBuf,
    pid: u32,
}

impl DaemonPidFileGuard {
    pub fn acquire(runtime_root: &std::path::Path) -> Result<Self, String> {
        fs::create_dir_all(runtime_root)
            .map_err(|error| format!("创建 Daemon PID 目录失败: {error}"))?;
        let path = runtime_root.join(daemon_service_manager::DAEMON_PID_FILE_NAME);
        let pid = std::process::id();
        let existing_pid = fs::read_to_string(&path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok());
        // 判据必须是「该 PID 仍由 Axon 守护进程持有」：PID 被系统回收给无关进程后，
        // 只看存活会把陈旧 pid 文件当成活实例，启动将永久失败。
        if let Some(existing_pid) =
            existing_pid.filter(|pid| daemon_service_manager::daemon_process_alive(*pid))
        {
            return Err(format!("Axon Daemon 已在运行: pid={existing_pid}"));
        }
        if existing_pid.is_some() {
            let _ = fs::remove_file(&path);
        }
        fs::write(&path, format!("{pid}\n"))
            .map_err(|error| format!("写入 Daemon PID 文件失败: {error}"))?;
        Ok(Self { path, pid })
    }
}

impl Drop for DaemonPidFileGuard {
    fn drop(&mut self) {
        let matches = fs::read_to_string(&self.path)
            .ok()
            .is_some_and(|value| value.trim() == self.pid.to_string());
        if matches {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// 会话事件发布端口。
///
/// 会话事件正文的落库与订阅扇出都由 Daemon 自身完成；本机没有任何外接渠道 egress，
/// 因此这里不做额外动作，只满足执行器对发布端口的要求。
#[derive(Clone, Copy, Default)]
pub struct DaemonEventPublisher;

impl EventPublisher for DaemonEventPublisher {
    fn publish(
        &self,
        _event: &axon_session::SessionEventRecord,
        _cursor: &axon_domain::ConsumerCursor,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct SystemClock;

impl RuntimeClock for SystemClock {
    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

#[derive(Clone, Copy)]
pub struct CliProcessIdentity;

impl axon_daemon::ProcessIdentity for CliProcessIdentity {
    fn pid(&self) -> u32 {
        std::process::id()
    }

    fn process_start_id(&self) -> Option<String> {
        // PID 会复用：把进程启动时刻作为第二身份，回收与认领时做二次校验。
        axon_process::process_start_id(self.pid())
    }
}

#[derive(Default)]
pub struct DaemonMaintenanceSignal {
    pub notified: Mutex<bool>,
    condition: Condvar,
}

impl DaemonMaintenanceSignal {
    /// 置位唤醒标记并唤醒一个等待者；唤醒是提示，等待者必须回到事实源复查。
    pub(crate) fn notify(&self) {
        if let Ok(mut notified) = self.notified.lock() {
            *notified = true;
            self.condition.notify_one();
        }
    }

    pub(super) fn wait(&self, timeout: std::time::Duration) {
        let Ok(mut notified) = self.notified.lock() else {
            return;
        };
        if !*notified {
            if let Ok((guard, _)) = self.condition.wait_timeout(notified, timeout) {
                notified = guard;
            } else {
                return;
            }
        }
        *notified = false;
    }
}

/// Daemon 的两条退出判据。
///
/// 两条判据语义不同，不能合并成一个标志：
///
/// - 外部停止请求是正常停机，进程以零退出码收尾，服务管理器不应重启；
/// - 租约被 fence 是代际失效（另一个 Host 已接管，或租约已过期），后台 Worker 已停，
///   进程必须以非零码退出，把重启交给计划任务 / systemd / launchd 的重启策略。
///
/// 只停 Worker 不退出进程的后果：进程活着、`daemon status` 仍报 running、网关照常应答，
/// 但没有任何 Worker 推进会话，而失败重启策略永远不会触发（任务从未失败）。
#[derive(Clone)]
pub struct DaemonExitSignals {
    /// 外部停止请求；None 表示当前运行形态没有停止通道。
    pub stop_requested: Option<Arc<AtomicBool>>,
    /// 租约已被 fence。
    pub host_fenced: Arc<AtomicBool>,
}

impl DaemonExitSignals {
    /// 接受循环是否该结束：停机请求与 fence 都算。
    #[must_use]
    pub fn should_stop(&self) -> bool {
        self.host_fenced.load(Ordering::Acquire)
            || self
                .stop_requested
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    /// 本次退出是否由 fence 引起；由它决定退出码。
    #[must_use]
    pub fn fenced(&self) -> bool {
        self.host_fenced.load(Ordering::Acquire)
    }
}

/// fence 置位后敲一次 IPC，唤醒阻塞中的 `accept`。
///
/// 接受循环阻塞在 `accept()` 上，只有新连接才会让它回到循环头复查停止判据；没有这次
/// 唤醒，fence 之后进程会一直停在 accept 上，退出判据形同虚设。
pub fn spawn_lease_fence_wake(endpoint: &str, host_fenced: Arc<AtomicBool>) {
    let endpoint = endpoint.to_owned();
    thread::spawn(move || {
        while !host_fenced.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(SERVICE_STOP_WAKEUP_POLL_MS));
        }
        let _ = axon_daemon::infrastructure::ipc::JsonlIpcClient::connect(&endpoint);
    });
}

/// Daemon 打开运行态库：关掉自动 WAL 回写，并对账重建水位超前的会话。
///
/// 自动回写由本模块的回写线程驱动，留在提交线程里会让每一批写入替别人的回写排队；
/// 但纯 CLI 进程没有这条线程，所以开关按进程设置，不能写死在建库路径上。
///
/// 启动对账针对「投影水位超过事件水位」的会话：日志被截断或整体替换过，投影已经错位。
/// 启动时是单写者窗口，重建不会与投影 worker 并发；数量通常为 0。
pub(super) fn open_runtime_store(runtime_root: &std::path::Path) -> SqliteRuntimeStore {
    axon_session::disable_automatic_wal_checkpoint();
    let store = SqliteRuntimeStore::from_root(runtime_root);
    match store.rebuild_rewound_projections() {
        Ok(0) => {}
        Ok(count) => axon_logger::info(&format!("启动投影重建完成：{count} 个会话")),
        Err(error) => axon_logger::warning(&format!("启动投影对账失败，跳过重建：{error}")),
    }
    // 再按日志身份对账：水位看不出「同样长度的另一份日志」，长度与修改时刻可以。
    match store.reconcile_log_identities() {
        Ok(0) => {}
        Ok(count) => axon_logger::info(&format!("日志身份对账重建完成：{count} 个会话")),
        Err(error) => axon_logger::warning(&format!("日志身份对账失败：{error}")),
    }
    store
}
