//! 后台作业自己那条会话的绑定：cron 与标题生成共用同一套。
//!
//! 一次 cron 运行是**后台任务自身的一轮**，不是用户对话里的追加：它必须落在按
//! `job_id` 稳定的 `kind='cron'` 会话上，重复运行复用同一条，历史因此连续；
//! 绝不能落到「该 Workspace 最近活跃的 primary 会话」，那正是用户的对话。
//!
//! 标题生成走同一套绑定：键是 `session-title:<目标会话>`，kind 仍是 `cron`
//! （不进侧栏清单），因此标题轮次同样有自己的会话与逐字稿，且不产生用户可见轮次。
//!
//! 绑定在**创建任务时**就完成（`bind_new_job_session`），CLI 与桌面走同一处；
//! 之后每次运行按 `execution_id = job_id` 找回同一条会话。只有任务显式声明
//! `--wake session:<session-id>` 时，才由唤醒通道把一条唤醒通知注入用户会话，
//! 那是另一条路径（`wake.rs` 的 `dispatch_wake`）。
//!
//! 唯一性由 `sessions_cron_binding` 部分唯一索引保证：并发运行撞索引时回读已有行，
//! 同一任务（同一个键）永远只有一条会话。

use std::path::Path;

use axon_domain::{CronJob, DaemonSessionId};
use axon_session::{
    NewSessionRequest, RuntimeStore, SessionKind, SessionTrustScope, SqliteRuntimeStore,
    WorkspaceResolver, WorkspaceStore,
};
use rusqlite::OptionalExtension as _;

use crate::run_commands::interaction_store;

/// 查一条按 `(kind, key)` 登记的会话；从未绑定过时返回 `None`。
///
/// cron 与标题生成共用这一处：两者都是「后台作业自己的一轮」，都按一个稳定键绑定
/// 同一条会话，历史因此连续，且绝不落到用户的对话会话上。
pub(crate) fn find_session_bound_to(
    store: &SqliteRuntimeStore,
    kind: SessionKind,
    key: &str,
) -> Result<Option<DaemonSessionId>, String> {
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

/// 解析一条按 `(kind, key)` 绑定的会话：已登记就复用，没有才建一条。
///
/// `preferred_cwd` 是这一轮的工作目录：目录不存在时退回到配置所在目录，保证
/// 「创建即绑定」不会因为一个还不存在的路径而失败。会话标识是服务端生成的 ULID，
/// 不按键名拼接。
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
    if let Some(found) = find_session_bound_to(&store, kind, key)? {
        return Ok(found);
    }
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
        execution_id: Some(key.to_owned()),
        cwd: workspace.workspace_root.clone(),
        now_ms,
    };
    if let Err(error) = store.create_session_with(&candidate) {
        // 并发下唯一索引可能已被另一路占位：回读成功就用它。
        return match find_session_bound_to(&store, kind, key)? {
            Some(existing) => Ok(existing),
            None => Err(error.to_string()),
        };
    }
    find_session_bound_to(&store, kind, key)?.ok_or_else(|| "后台会话登记后回读为空".to_owned())
}

/// 查一条任务已登记的 cron 会话；从未运行/绑定过时返回 `None`。
pub(crate) fn find_cron_session(
    store: &SqliteRuntimeStore,
    job_id: &str,
) -> Result<Option<DaemonSessionId>, String> {
    find_session_bound_to(store, SessionKind::Cron, job_id)
}

/// 解析该任务的 cron 会话：已登记就复用，没有才建一条。
pub(crate) fn resolve_cron_session(
    config_path: &Path,
    job: &CronJob,
    axon_root: &Path,
    now_ms: u64,
) -> Result<DaemonSessionId, String> {
    let preferred = job.workdir.as_deref().map(Path::new).unwrap_or(axon_root);
    resolve_session_bound_to(
        config_path,
        SessionKind::Cron,
        &job.job_id,
        preferred,
        now_ms,
    )
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
