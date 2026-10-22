//! `process_manager` 的测试夹具。
//!
//! 独立成文件有两个原因：单文件行数上限，以及墓碑回收与空闲回收两组测试需要**同一份**
//! 夹具：「构造一个带真实后端的会话」这件事若有第二份实现，夹具本身就会漂移。

use super::super::{ShellCommandToolRuntime, session::TerminalState};
use super::*;

/// 测试用保留窗口；只需可注入时间即可判定超期，不必等于生产值。
pub(super) const TEST_TERMINAL_RETENTION: Duration = Duration::from_secs(60);

pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn sessions_with(session: Arc<Mutex<ShellProcessSession>>) -> SessionRegistry {
    let id = lock(&session).id.clone();
    Arc::new(Mutex::new(HashMap::from([(id, session)])))
}

pub(super) fn empty_tombstones() -> SessionTombstones {
    Arc::new(Mutex::new(VecDeque::new()))
}

/// 构造一个已处于终态、且结束时间可注入的会话；`reap_once` 只读其终态字段。
pub(super) fn terminal_session(id: &str, ended_at: Instant) -> Arc<Mutex<ShellProcessSession>> {
    let command = ShellCommandToolRuntime::command_from_script("echo tombstone", false)
        .expect("命令应规范化成功");
    session_with(
        id,
        ended_at,
        command,
        Some(TerminalState {
            exit_code: 0,
            timed_out: false,
            cancelled: false,
            ended_at,
        }),
    )
}

/// 构造一个仍在运行、且最后一次触碰时间可注入的会话。
pub(super) fn running_session(id: &str, last_touch: Instant) -> Arc<Mutex<ShellProcessSession>> {
    let command = ShellCommandToolRuntime::command_from_script("echo idle-reclaim", false)
        .expect("命令应规范化成功");
    session_with(id, last_touch, command, None)
}

/// 两组夹具共用的构造路径：字段序只有这一处，新增字段时不会漏掉其中一组。
fn session_with(
    id: &str,
    timestamp: Instant,
    command: Vec<String>,
    terminal: Option<TerminalState>,
) -> Arc<Mutex<ShellProcessSession>> {
    let (artifact, artifact_path) = temp_artifact();
    let output = Arc::new(SharedProcessOutput::new(artifact, SHELL_ARTIFACT_MAX_BYTES));
    let cwd = std::env::current_dir().expect("测试工作目录应可解析");
    let backend = ProcessBackend::spawn(&command, &cwd, false, Arc::clone(&output), &[])
        .expect("后端应启动成功");
    Arc::new(Mutex::new(ShellProcessSession {
        id: id.to_owned(),
        owner_id: format!("process-manager-tests-{id}"),
        command,
        cwd,
        tty: false,
        started: timestamp,
        last_activity: timestamp,
        deadline: None,
        cancel_token: None,
        backend,
        output,
        artifact_path,
        terminal,
        reclaim_pending: false,
        ctrl_c_requested: false,
        interaction_lock: Arc::new(Mutex::new(())),
        path_snapshots: Vec::new(),
    }))
}

pub(super) fn temp_artifact() -> (File, PathBuf) {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-shell-session-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let file = File::create(&path).expect("临时文件应创建成功");
    (file, path)
}
