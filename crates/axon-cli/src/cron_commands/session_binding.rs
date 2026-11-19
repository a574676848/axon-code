//! 后台作业自己那条会话的绑定：cron 与标题生成共用同一套解析。
//!
//! 一次 cron 运行是**后台任务自身的一轮**，不是用户对话里的追加：它必须落在按
//! `job_id` 稳定的 `kind='cron'` 会话上，重复运行复用同一条，历史因此连续；
//! 绝不能落到「该 Workspace 最近活跃的 primary 会话」，那正是用户的对话。
//!
//! cron 的绑定存在**任务行自己的 `session_id` 列**（cron 库）：创建任务时算一次，
//! 之后每次运行按它回查。不能把 `sessions.execution_id` 当绑定键：那一列记的是
//! 「当前这一轮是谁」，每次执行都会被运行侧覆写，用过一次就再也查不到绑定。
//!
//! 标题生成也复用 `kind='cron'` 的会话，但它没有任务行可挂列，仍按一次性键
//! `session-title:<目标会话>` 解析；标题只生成一次，不参与周期性复用。
//!
//! 绑定在**创建任务时**就完成（`bind_new_job_session`），CLI 与桌面走同一处；
//! 只有任务显式声明 `--wake session:<session-id>` 时，才由唤醒通道把一条唤醒通知
//! 注入用户会话，那是另一条路径（`wake.rs` 的 `dispatch_wake`）。

use std::path::Path;

use axon_domain::{CronJob, DaemonSessionId};
use axon_session::{
    NewSessionRequest, RuntimeStore, SessionKind, SessionTrustScope, WorkspaceResolver,
    WorkspaceStore,
};
use rusqlite::OptionalExtension as _;

use crate::run_commands::interaction_store;

use super::repository::CronRepository;

/// 查一条按 `(kind, key)` 登记的会话；从未绑定过时返回 `None`。
///
/// 只服务标题生成这类一次性后台会话：键是执行标识，跑完一轮后会被覆写，
/// 不适合周期性任务复用。cron 任务走任务行的 `session_id` 列。
pub(crate) fn find_session_bound_to(
    config_path: &Path,
    kind: SessionKind,
    key: &str,
) -> Result<Option<DaemonSessionId>, String> {
    let store = interaction_store(Some(config_path))?;
    let found: Option<String> = store
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT session_id FROM sessions \
                     WHERE kind = ?1 AND execution_id = ?2 LIMIT 1",
                    rusqlite::params![kind.as_str(), key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
        .map_err(|error| error.to_string())?;
    found
        .map(|value| DaemonSessionId::new(value).map_err(|error| error.to_string()))
        .transpose()
}

/// 解析一条按 `(kind, key)` 绑定的一次性后台会话：已登记就复用，没有才建一条。
pub(crate) fn resolve_session_bound_to(
    config_path: &Path,
    kind: SessionKind,
    key: &str,
    preferred_cwd: &Path,
    now_ms: u64,
) -> Result<DaemonSessionId, String> {
    let store = interaction_store(Some(config_path))?;
    // 全新运行态根上第一次建会话时库还没建好：先把最新 DDL 落下去，再查绑定。
    store.initialize().map_err(|error| error.to_string())?;
    if let Some(found) = find_session_bound_to(config_path, kind, key)? {
        return Ok(found);
    }
    let created = create_background_session(&store, kind, preferred_cwd, config_path, now_ms)?;
    // 一次性绑定仍写到执行标识上：标题生成只跑一次，不存在被下一轮覆写的问题。
    store
        .bind_session_execution(&created, key, now_ms)
        .map_err(|error| error.to_string())?;
    Ok(created)
}

/// 查一条任务已登记的 cron 会话；从未创建/绑定过时返回 `None`。
pub(crate) fn find_cron_session(
    axon_root: &Path,
    job_id: &str,
) -> Result<Option<DaemonSessionId>, String> {
    let repository = CronRepository::open(axon_root)?;
    repository
        .session_id(job_id)?
        .map(|value| DaemonSessionId::new(value).map_err(|error| error.to_string()))
        .transpose()
}

/// 解析该任务的 cron 会话：任务行已绑定且会话仍存在就复用，否则新建并写回任务行。
pub(crate) fn resolve_cron_session(
    config_path: &Path,
    job: &CronJob,
    axon_root: &Path,
    now_ms: u64,
) -> Result<DaemonSessionId, String> {
    let repository = CronRepository::open(axon_root)?;
    let store = interaction_store(Some(config_path))?;
    store.initialize().map_err(|error| error.to_string())?;
    if let Some(found) = find_cron_session(axon_root, &job.job_id)?
        && store.get_session(&found).is_ok()
    {
        return Ok(found);
    }
    let preferred = job.workdir.as_deref().map(Path::new).unwrap_or(axon_root);
    let created =
        create_background_session(&store, SessionKind::Cron, preferred, config_path, now_ms)?;
    repository.set_session_id(&job.job_id, created.as_str())?;
    Ok(created)
}

/// 新建一条后台会话（cron 或标题生成）：工作目录、工作区登记与会话行一次落齐。
///
/// `preferred_cwd` 不存在时退回配置所在目录，保证「创建即绑定」不会因为一个还
/// 不存在的路径而失败。会话标识是服务端生成的 ULID，不按键名拼接。
fn create_background_session(
    store: &axon_session::SqliteRuntimeStore,
    kind: SessionKind,
    preferred_cwd: &Path,
    config_path: &Path,
    now_ms: u64,
) -> Result<DaemonSessionId, String> {
    let cwd = if preferred_cwd.is_dir() {
        preferred_cwd.to_path_buf()
    } else {
        config_path
            .parent()
            .map_or_else(|| preferred_cwd.to_path_buf(), Path::to_path_buf)
    };
    let workspace = WorkspaceResolver
        .resolve(&cwd, now_ms)
        .map_err(|error| format!("解析后台会话工作区失败：{error}"))?;
    store
        .register_workspace(&workspace)
        .map_err(|error| error.to_string())?;
    let candidate = NewSessionRequest {
        session_id: axon_session::new_session_id(),
        kind,
        trust_scope: SessionTrustScope::Private,
        workspace_id: Some(workspace.workspace_id.as_str().to_owned()),
        root_session_id: None,
        agent_path: None,
        goal_id: None,
        execution_id: None,
        cwd: workspace.workspace_root.clone(),
        now_ms,
    };
    store
        .create_session_with(&candidate)
        .map_err(|error| error.to_string())?;
    Ok(candidate.session_id)
}

/// 新建任务后的会话绑定：CLI 与桌面共用这一处。
///
/// 无 Agent 的脚本任务不产生轮次，因此不建会话（返回 `None`）；其余任务在**创建时**
/// 就拿到自己那条 cron 会话，自动化列表随即可以打开它，首轮运行也不必再等建会话。
pub(crate) fn bind_new_job_session(
    config_path: &Path,
    axon_root: &Path,
    job: &CronJob,
    now_ms: u64,
) -> Result<Option<DaemonSessionId>, String> {
    if job.no_agent {
        return Ok(None);
    }
    resolve_cron_session(config_path, job, axon_root, now_ms).map(Some)
}
