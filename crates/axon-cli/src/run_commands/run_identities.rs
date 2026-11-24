//! 一轮运行的标识：会话身份与运行窗口。
//!
//! 两个标识空间各有唯一来源，互不回落：
//!
//! - 会话身份是 `sessions.session_id`（服务端 ULID），用户可见：`axon sessions`、
//!   `--session` 与 `--continue` 只认它，入站事实、会话标题与子代理归属都挂在它上面。
//! - 运行窗口是会话行上的 `sessions.current_run_id`：Thread 事件流、工具作业归属与
//!   中断通道只认它，同一会话换窗后窗口随之更新。
//!
//! 任何一轮运行都挂在一条已登记的会话上，因此解析结果必须成对出现：既没有只有窗口
//! 没有会话的入口，也不允许把窗口当会话用。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{JsonlThreadEventStore, RuntimeStore as _};

use super::{RunCommand, run_execution::now_ms};

/// 最近会话查询的候选条数：连读多条才有话可续，超出这批的会话已不再「最近」。
const RECENT_SESSION_LIMIT: usize = 20;

/// 一轮运行的会话身份与运行窗口。
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RunIdentities {
    /// `sessions.session_id`。
    pub(super) session_id: String,
    /// `sessions.current_run_id`。
    pub(super) run_session_id: String,
}

impl RunIdentities {
    pub(crate) fn new(session_id: String, run_session_id: String) -> Self {
        Self {
            session_id,
            run_session_id,
        }
    }
}

/// 解析本轮的会话身份与运行窗口。
pub(super) fn resolve_run_identities(
    config_path: &Path,
    command: &RunCommand,
    cwd: Option<&Path>,
) -> Result<RunIdentities, String> {
    // 宿主桥接：两个标识由宿主从同一行会话取出，直接采用。
    if let Some(run_session_id) = command.run_session_id.as_deref() {
        let session_id = command
            .session
            .clone()
            .ok_or_else(|| "运行窗口缺少会话归属：宿主必须同时给出会话标识".to_owned())?;
        return Ok(RunIdentities::new(session_id, run_session_id.to_owned()));
    }
    if let Some(session_id) = resumed_session_id(config_path, command)? {
        return session_identities(config_path, &session_id);
    }
    let Some(cwd) = cwd else {
        return Err("缺少会话标识与工作目录：无法确定本轮挂在哪条会话上".to_owned());
    };
    // 隐式入口：工作目录决定 Workspace，新建一条主会话。
    super::interaction_adapter::conversation_session_identities(
        cwd,
        Some(config_path),
        now_ms(),
        super::CLI_INTERACTION_CHANNEL,
        super::cli_reply_destination(),
    )
    .map(|(session_id, run_session_id)| RunIdentities::new(session_id, run_session_id))
}

/// 显式续接的目标会话：`--session` 优先，其次 `--continue` 的恢复指针与最近一段对话。
fn resumed_session_id(config_path: &Path, command: &RunCommand) -> Result<Option<String>, String> {
    if let Some(session_id) = command.session.as_deref() {
        return Ok(Some(session_id.trim().to_owned()));
    }
    if !command.continue_last {
        return Ok(None);
    }
    if let Some(pointer) = super::snapshot_resume_session(config_path)? {
        return Ok(Some(pointer));
    }
    latest_resumable_session_id(config_path)
}

/// 会话行决定的两个标识：会话身份就是行本身，运行窗口取行上的 `current_run_id`。
pub(crate) fn session_identities(
    config_path: &Path,
    session_id: &str,
) -> Result<RunIdentities, String> {
    let session = DaemonSessionId::new(session_id.to_owned())
        .map_err(|_| format!("会话 ID 非法：{session_id}"))?;
    let store = super::session_commands::open_control_store(config_path)?;
    let record = store.get_session(&session).map_err(|_| {
        format!("未找到会话：{session_id}。先用 `axon sessions list` 确认可用会话 ID")
    })?;
    Ok(RunIdentities::new(
        record.session_id.as_str().to_owned(),
        record.current_run_id,
    ))
}

/// `--continue` 没有恢复指针时接续的会话：最近一段已有 Thread 的主会话。
fn latest_resumable_session_id(config_path: &Path) -> Result<Option<String>, String> {
    let store = super::session_commands::open_control_store(config_path)?;
    let candidates = store
        .with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT session_id,current_run_id FROM sessions \
                 WHERE kind=?1 AND archived=0 \
                 ORDER BY last_activity_at_ms DESC LIMIT ?2",
            )?;
            let rows = statement.query_map(
                rusqlite::params![
                    axon_session::SessionKind::Primary.as_str(),
                    i64::try_from(RECENT_SESSION_LIMIT).unwrap_or(i64::MAX)
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| error.to_string())?;
    let root = crate::runtime_paths::runtime_axon_root_path_static(config_path)?;
    // 零轮会话没有任何可接续的内容：跳过它，绝不让 `--continue` 开出一段空对话。
    for (session_id, run_session_id) in candidates {
        if JsonlThreadEventStore::thread_shard_paths(&root, &run_session_id)
            .is_ok_and(|paths| !paths.is_empty())
        {
            return Ok(Some(session_id));
        }
    }
    Ok(None)
}
