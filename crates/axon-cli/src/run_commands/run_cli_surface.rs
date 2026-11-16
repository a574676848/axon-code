//! CLI 运行面的解析：这一轮 `axon run` 应该落在哪条会话车道上。
//!
//! 会话标识由（工作区、受众、渠道、投递目标）四元组推导，因此「续接同一会话」不是
//! 记一个 id 就够：必须复用那轮入站事实的渠道与投递目标，Daemon 才能推导出同一个
//! 会话标识。`--session` 与 `--continue` 都在这里解析成 `(channel, destination)`。
//!
//! 未指定时按工作目录取稳定车道：矩阵 C4 要求同一 cwd 的连续 `axon run` 接得上上下文，
//! 因此投递目标由 cwd 派生而不是每次进程随机；会话内串行由 Daemon 保证，同一车道
//! 不会出现两个前台争用同一 interaction。

use std::path::Path;

use axon_session::{InteractionQueryStore as _, RuntimeStore as _, SqliteRuntimeStore};

use crate::run_commands::run_resume_pointer;

/// CLI 渠道名。
const CLI_CHANNEL: &str = "cli";

/// 工作目录对应的稳定 CLI 投递目标。
pub(crate) fn cli_reply_destination_for(cwd: &Path) -> String {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let key = canonical.to_string_lossy().replace('\\', "/");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&key, &mut hasher);
    format!("terminal-{:016x}", std::hash::Hasher::finish(&hasher))
}

/// 解析本轮要用的 `(channel, destination)`；显式指定会话时校验它确实存在且可续接。
pub(crate) fn resolve_cli_run_surface(
    config_path: Option<&Path>,
    session: Option<&str>,
    continue_last: bool,
    cwd: &Path,
) -> Result<StringAndDestination, String> {
    let target = if let Some(session_id) = session {
        Some(session_id.to_owned())
    } else if continue_last {
        let config = resolved_config_path(config_path)?;
        Some(
            run_resume_pointer::snapshot_resume_session(&config)?.ok_or_else(|| {
                "没有可续接的会话：先用 `axon sessions resume <id>` 指定，或用 `--session <id>`"
                    .to_owned()
            })?,
        )
    } else {
        None
    };
    let Some(target) = target else {
        return Ok(StringAndDestination {
            channel: CLI_CHANNEL.to_owned(),
            destination: cli_reply_destination_for(cwd),
            session_id: None,
        });
    };
    let config = resolved_config_path(config_path)?;
    let surface = session_surface(&config, &target)?;
    Ok(StringAndDestination {
        channel: surface.0,
        destination: surface.1,
        session_id: Some(target),
    })
}

/// 一轮运行的会话面。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StringAndDestination {
    /// 入站渠道。
    pub(crate) channel: String,
    /// 投递目标。
    pub(crate) destination: String,
    /// 显式指定的目标会话；`None` 表示按工作目录车道新建。
    pub(crate) session_id: Option<String>,
}

/// 读取目标会话最近一条入站事实的渠道与投递目标。
fn session_surface(config_path: &Path, session_id: &str) -> Result<(String, String), String> {
    let data_root =
        crate::runtime_paths::runtime_axon_data_path_static(config_path).map_err(str::to_owned)?;
    let store = SqliteRuntimeStore::from_root(data_root);
    store.initialize().map_err(|error| error.to_string())?;
    let latest = store
        .interactions_for_session(session_id, 1, None)
        .map_err(|error| error.to_string())?
        .into_iter()
        .next_back()
        .ok_or_else(|| {
            format!(
                "会话不存在或没有入站事实：{session_id}。先用 `axon sessions list` 查看可用会话；`--session` 不会隐式新建会话"
            )
        })?;
    Ok((
        latest.source.channel().to_owned(),
        latest.reply_route.destination().to_owned(),
    ))
}

fn resolved_config_path(config_path: Option<&Path>) -> Result<std::path::PathBuf, String> {
    if let Some(path) = config_path {
        return Ok(path.to_path_buf());
    }
    axon_config::ConfigLoader::default_user_config_path().map_err(|error| format!("{error:?}"))
}
