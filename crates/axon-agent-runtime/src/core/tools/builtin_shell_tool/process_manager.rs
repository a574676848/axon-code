//! 跨工具调用复用的持久 Shell 进程会话管理器。
//!
//! 管理器负责会话的创建、查找、等待与回收；单个会话的状态机与回执组装见 `session`。

use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

use super::{
    SHELL_COMMAND_CANCELLED_EXIT_CODE, SHELL_SESSION_EXPIRED_ERROR, SHELL_SESSION_NOT_FOUND_ERROR,
    SHELL_SESSION_OWNER_ERROR, ShellCommandExecutionInput, ShellCommandExecutionReceipt,
    ShellCommandInteraction, ShellCommandInteractionKind, ShellCommandOutputObserver,
    WriteStdinExecutionInput,
    output_buffer::SharedProcessOutput,
    process_backend::ProcessBackend,
    session::{
        CancelPollBackoff, PathSnapshot, SessionWaitKind, ShellProcessSession,
        apply_terminal_interaction, effective_yield_duration, emit_pending_output,
    },
    session_reclaim::{ShellSessionReclaim, publish_shell_session_reclaim},
};
use crate::builtin_shell_script::{is_canonical_wsl_script_command, shell_env_overrides};
use crate::runtime_limits::{
    SHELL_ARTIFACT_DIR_NAME, SHELL_ARTIFACT_FILE_SUFFIX, SHELL_ARTIFACT_MAX_BYTES,
};

const SHELL_TERMINAL_RETENTION: Duration = Duration::from_secs(5 * 60);
/// 已回收会话墓碑的保留时长。
///
/// 必须长于 `SHELL_TERMINAL_RETENTION`：会话被回收之后，模型仍可能在若干轮内
/// 拿着旧回执里的 ID 来查，此时应得到「已回收」而不是「不存在」。
const SHELL_TOMBSTONE_RETENTION: Duration = Duration::from_secs(30 * 60);
/// 墓碑容量上界；超出后按 FIFO 淘汰最旧的记录，避免长期运行下无限增长。
const SHELL_TOMBSTONE_CAPACITY: usize = 256;
const SHELL_REAPER_INTERVAL: Duration = Duration::from_secs(1);

/// 已回收会话的墓碑。
///
/// 仅用于区分「曾存在但已回收」与「从未存在」，不承载任何访问能力：
/// 命中墓碑的调用方依然拿不到会话，只会得到一个更诚实的错误原因。
struct SessionTombstone {
    session_id: String,
    retired_at: Instant,
}

/// 会话注册表：会话 ID → 会话句柄。
type SessionRegistry = Arc<Mutex<HashMap<String, Arc<Mutex<ShellProcessSession>>>>>;
/// 已回收会话的墓碑队列，按回收时间从旧到新排列。
type SessionTombstones = Arc<Mutex<VecDeque<SessionTombstone>>>;

static SHELL_PROCESS_MANAGER: LazyLock<ShellProcessManager> =
    LazyLock::new(ShellProcessManager::new);

/// 进程级持久 Shell 会话管理器。
pub struct ShellProcessManager {
    sessions: SessionRegistry,
    tombstones: SessionTombstones,
}

impl ShellProcessManager {
    fn new() -> Self {
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let tombstones = Arc::new(Mutex::new(VecDeque::new()));
        spawn_reaper(Arc::clone(&sessions), Arc::clone(&tombstones));
        Self {
            sessions,
            tombstones,
        }
    }

    pub(super) fn global() -> &'static Self {
        &SHELL_PROCESS_MANAGER
    }

    pub(super) fn execute(
        &self,
        input: ShellCommandExecutionInput,
        output_observer: Option<&mut ShellCommandOutputObserver<'_>>,
    ) -> std::io::Result<ShellCommandExecutionReceipt> {
        let session_id = Uuid::new_v4().to_string();
        let cwd = input.cwd.clone();
        let artifact_path = create_artifact(
            input.tee_root.as_deref(),
            &session_id,
            &input.command,
            &cwd,
            input.tty,
        )?;
        let artifact = OpenOptions::new().append(true).open(&artifact_path)?;
        let output = Arc::new(SharedProcessOutput::new(artifact, SHELL_ARTIFACT_MAX_BYTES));
        // WSL 命令的宿主是 wsl.exe，变量需经 WSLENV 声明才能在 Linux 侧可见。
        let env_overrides =
            shell_env_overrides(input.tty, is_canonical_wsl_script_command(&input.command));
        let backend = ProcessBackend::spawn(
            &input.command,
            &cwd,
            input.tty,
            Arc::clone(&output),
            &env_overrides,
        )?;
        let started = Instant::now();
        let deadline = input
            .timeout_ms
            .and_then(|timeout| started.checked_add(Duration::from_millis(timeout)));
        let session = Arc::new(Mutex::new(ShellProcessSession {
            id: session_id.clone(),
            owner_id: input.owner_id,
            command: input.command,
            cwd,
            tty: input.tty,
            started,
            last_activity: started,
            deadline,
            cancel_token: input.cancel_token,
            backend,
            output,
            artifact_path,
            terminal: None,
            reclaim_pending: false,
            ctrl_c_requested: false,
            interaction_lock: Arc::new(Mutex::new(())),
            path_snapshots: input
                .write_paths
                .into_iter()
                .map(|path| {
                    let snapshot = PathSnapshot::capture(&path);
                    (path, snapshot)
                })
                .collect(),
        }));
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.clone(), Arc::clone(&session));
        self.wait_for_session(
            &session_id,
            input.yield_time_ms,
            input.max_output_tokens,
            output_observer,
            SessionWaitKind::Initial,
        )
    }

    pub(super) fn write_stdin(
        &self,
        input: WriteStdinExecutionInput,
        output_observer: Option<&mut ShellCommandOutputObserver<'_>>,
    ) -> std::io::Result<ShellCommandExecutionReceipt> {
        let session = self.session(&input.session_id)?;
        let interaction = ShellCommandInteraction::from_input(input.chars.as_deref(), input.ctrl_c);
        let wait_kind = match interaction.kind {
            ShellCommandInteractionKind::Wait => SessionWaitKind::Background,
            ShellCommandInteractionKind::Input | ShellCommandInteractionKind::Interrupt => {
                SessionWaitKind::Interaction
            }
        };
        let interaction_lock = {
            let session = session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(&session.interaction_lock)
        };
        let _interaction_guard = interaction_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let interaction_applied = {
            let mut session = session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if session.owner_id != input.owner_id {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    SHELL_SESSION_OWNER_ERROR,
                ));
            }
            session.poll(Instant::now())?;
            // 终态会话不写入任何输入（工具描述已就此承诺），故此处如实返回「未施加」。
            let applied = session.terminal.is_none();
            if applied {
                apply_terminal_interaction(&mut session, &input)?;
            }
            applied
        };
        let mut receipt = self.wait_for_session(
            &input.session_id,
            input.yield_time_ms,
            input.max_output_tokens,
            output_observer,
            wait_kind,
        )?;
        // 只有**真正施加过**输入 / 中断时才上报交互；请求本身就是纯等待（`Wait`）
        // 时也如实上报「等待发生了」。终态会话下进程一个字节都没收到，若仍上报
        // `Input` / `Interrupt`，TUI 会渲染成「已交互后台终端 ＋ 输入内容」，
        // 而该输入从未送达（违反 INV-3）。
        if interaction_applied || matches!(interaction.kind, ShellCommandInteractionKind::Wait) {
            receipt.interaction = Some(interaction);
        }
        Ok(receipt)
    }

    pub(super) fn terminate_owner(&self, owner_id: &str) -> usize {
        if owner_id.trim().is_empty() {
            return 0;
        }
        let owned = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids = sessions
                .iter()
                .filter_map(|(id, session)| {
                    let session = session
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (session.owner_id == owner_id).then(|| id.clone())
                })
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| sessions.remove(&id))
                .collect::<Vec<_>>()
        };
        for session in &owned {
            let mut session = session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if session.terminal.is_none() {
                let _ = session.backend.terminate();
                session.mark_terminal(SHELL_COMMAND_CANCELLED_EXIT_CODE, false, true);
            }
        }
        owned.len()
    }

    fn wait_for_session(
        &self,
        session_id: &str,
        yield_time_ms: u64,
        max_output_tokens: Option<usize>,
        mut output_observer: Option<&mut ShellCommandOutputObserver<'_>>,
        wait_kind: SessionWaitKind,
    ) -> std::io::Result<ShellCommandExecutionReceipt> {
        let session = self.session(session_id)?;
        let output = {
            let session = session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(&session.output)
        };
        let wait_started = Instant::now();
        let yield_duration = effective_yield_duration(yield_time_ms, wait_kind);
        let mut cancel_poll_backoff = CancelPollBackoff::default();
        loop {
            let observed = output.change_generation();
            let (terminal, next_check) = {
                let mut session = session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let now = Instant::now();
                session.poll(now)?;
                (
                    session.terminal.is_some(),
                    session.next_check_delay(now, cancel_poll_backoff.delay()),
                )
            };
            let had_output = emit_pending_output(&session, output_observer.as_deref_mut());
            if had_output {
                cancel_poll_backoff.reset();
            }
            let remaining = yield_duration.saturating_sub(wait_started.elapsed());
            if terminal || remaining.is_zero() {
                break;
            }
            output.wait_for_change(
                observed,
                next_check.map_or(remaining, |delay| delay.min(remaining)),
            );
            cancel_poll_backoff.increase();
        }
        emit_pending_output(&session, output_observer);
        Ok(session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .receipt(max_output_tokens))
    }

    fn session(&self, session_id: &str) -> std::io::Result<Arc<Mutex<ShellProcessSession>>> {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned();
        session.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                self.unreachable_session_reason(session_id),
            )
        })
    }

    /// 会话不可达时的原因。
    ///
    /// 墓碑命中说明该会话曾真实存在、只是超出保留窗口被回收；此时若回「不存在」，
    /// 回执就等于否认了一个模型确实持有过的 ID，违反 INV-3。
    fn unreachable_session_reason(&self, session_id: &str) -> &'static str {
        let tombstones = self
            .tombstones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        session_unreachable_reason(&tombstones, session_id)
    }
}

fn session_unreachable_reason(
    tombstones: &VecDeque<SessionTombstone>,
    session_id: &str,
) -> &'static str {
    if tombstones
        .iter()
        .any(|tombstone| tombstone.session_id == session_id)
    {
        SHELL_SESSION_EXPIRED_ERROR
    } else {
        SHELL_SESSION_NOT_FOUND_ERROR
    }
}

pub(crate) fn create_artifact(
    tee_root: Option<&Path>,
    session_id: &str,
    command: &[String],
    cwd: &Path,
    tty: bool,
) -> std::io::Result<PathBuf> {
    let header = format!(
        "command={}\ncwd={}\ntty={tty}\n",
        command.join(" "),
        cwd.display()
    );
    create_artifact_with_header(tee_root, session_id, &header)
}

/// 创建输出归档并写入调用方给出的头部；返回归档路径。
///
/// 归档目录与后缀对全部生产者统一（`SHELL_ARTIFACT_DIR_NAME` /
/// `SHELL_ARTIFACT_FILE_SUFFIX`），因此清理侧不需要为子代理作业新增识别分支。
pub(crate) fn create_artifact_with_header(
    tee_root: Option<&Path>,
    artifact_name: &str,
    header: &str,
) -> std::io::Result<PathBuf> {
    let root = tee_root.map_or_else(std::env::temp_dir, Path::to_path_buf);
    let directory = root.join(SHELL_ARTIFACT_DIR_NAME);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{artifact_name}{SHELL_ARTIFACT_FILE_SUFFIX}"));
    let mut file = File::create(&path)?;
    file.write_all(header.as_bytes())?;
    file.flush()?;
    Ok(path)
}

/// 后台回收线程：按固定间隔推进会话状态、回收超期会话并维护墓碑。
///
/// 只有本线程会回收「仍可能被模型查询」的会话；`terminate_owner` 是 run 拆除路径，
/// 调用之后模型已无法再调用工具，因此不需要埋墓碑。
fn spawn_reaper(sessions: SessionRegistry, tombstones: SessionTombstones) {
    thread::spawn(move || {
        loop {
            thread::sleep(SHELL_REAPER_INTERVAL);
            reap_once(
                &sessions,
                &tombstones,
                Instant::now(),
                SHELL_TERMINAL_RETENTION,
            );
        }
    });
}

/// 单次回收：本轮的全部判定都以注入的 `now` 与 `terminal_retention` 为准。
///
/// 抽成独立函数是为了能用注入时间确定性覆盖「超期回收 → 埋墓碑」这条路径，
/// 无需真的等待保留窗口。
fn reap_once(
    sessions: &SessionRegistry,
    tombstones: &SessionTombstones,
    now: Instant,
    terminal_retention: Duration,
) {
    let snapshot = sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|(id, session)| (id.clone(), Arc::clone(session)))
        .collect::<Vec<_>>();
    let mut expired = Vec::new();
    let mut reclaims = Vec::new();
    for (id, session) in snapshot {
        let mut session = session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session.poll(now).is_err() && session.terminal.is_none() {
            let _ = session.backend.terminate();
            session.mark_terminal(1, false, false);
        }
        if let Some(notice) = take_idle_reclaim_notice(&mut session) {
            reclaims.push(notice);
        }
        if session
            .terminal
            .as_ref()
            .is_some_and(|terminal| now.duration_since(terminal.ended_at) >= terminal_retention)
        {
            expired.push(id);
        }
    }
    // 通知在锁外投递：消费方是跨 crate 的回调，不持会话锁也就不承担锁序风险。
    for notice in reclaims {
        publish_shell_session_reclaim(notice);
    }
    if !expired.is_empty() {
        sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|id, _| !expired.contains(id));
        record_tombstones(tombstones, expired, now);
    } else {
        prune_tombstones(tombstones, now);
    }
}

fn record_tombstones(tombstones: &SessionTombstones, expired: Vec<String>, now: Instant) {
    let mut tombstones = tombstones
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tombstones.extend(expired.into_iter().map(|session_id| SessionTombstone {
        session_id,
        retired_at: now,
    }));
    while tombstones.len() > SHELL_TOMBSTONE_CAPACITY {
        tombstones.pop_front();
    }
    prune_tombstones_locked(&mut tombstones, now);
}

fn prune_tombstones(tombstones: &SessionTombstones, now: Instant) {
    let mut tombstones = tombstones
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    prune_tombstones_locked(&mut tombstones, now);
}

/// 取出会话的空闲回收通知载荷；未发生回收或已通知过时返回 `None`。
///
/// 终态可能被回收线程、`wait_for_session` 或 `write_stdin` 任一路径先行观察到，
/// 因此这里读的是状态机上的一次性标记，而不是「谁先看到谁通知」：
/// 回收线程每 `SHELL_REAPER_INTERVAL` 轮询一次，无论谁先标记它都会把通知投出去。
fn take_idle_reclaim_notice(session: &mut ShellProcessSession) -> Option<ShellSessionReclaim> {
    if !session.take_reclaim_pending() {
        return None;
    }
    Some(ShellSessionReclaim::new(
        &session.id,
        &session.owner_id,
        &session.command.join(" "),
        &session.cwd.display().to_string(),
        SHELL_COMMAND_CANCELLED_EXIT_CODE,
        Instant::now().saturating_duration_since(session.last_activity),
        &session.artifact_path.display().to_string(),
    ))
}

fn prune_tombstones_locked(tombstones: &mut VecDeque<SessionTombstone>, now: Instant) {
    while tombstones.front().is_some_and(|tombstone| {
        now.duration_since(tombstone.retired_at) >= SHELL_TOMBSTONE_RETENTION
    }) {
        tombstones.pop_front();
    }
}

#[cfg(test)]
#[path = "process_manager_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "process_manager_tombstone_tests.rs"]
mod tombstone_tests;

#[cfg(test)]
#[path = "process_manager_reclaim_tests.rs"]
mod idle_reclaim_tests;

#[cfg(test)]
#[path = "process_manager_output_drain_tests.rs"]
mod output_drain_tests;
