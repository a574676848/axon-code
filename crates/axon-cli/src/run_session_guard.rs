//! `axon run` 入口的会话守卫。
//!
//! 两件事都只在**用户显式 `--session`** 时生效：
//! 1. 指定会话必须已存在（拼错不能静默新建）；
//! 2. 运行结束后把 resume 指针还原（跨会话追问不能改掉 `--continue` 的默认目标）。

use crate::{AxonCli, CliRunResult, RunCommand, run_command_from_cli};
use std::io::Write as IoWrite;

/// `axon run --session` 指向的会话必须已存在。
///
/// 只校验用户显式指定的这一条路径：子代理与测试会用 `--session` 生成新的子会话，
/// 它们不经过这里。拼错 ID 时若静默新建，用户会以为历史还在，而模型什么都没看到。
pub(crate) fn validate_explicit_session(cli: &AxonCli, command: &RunCommand) -> Result<(), String> {
    let Some(session_id) = command.session.as_deref() else {
        return Ok(());
    };
    let config_path =
        crate::management_commands::resolved_config_path(cli.global.config.as_deref())?;
    let data_root = crate::runtime_paths::runtime_axon_data_path_static(&config_path)?;
    let store = axon_session::SqliteRuntimeStore::from_root(data_root);
    store.initialize().map_err(|error| error.to_string())?;
    let parsed = axon_domain::DaemonSessionId::new(session_id.to_owned())
        .map_err(|_| format!("会话 ID 非法：{session_id}"))?;
    use axon_session::RuntimeStore as _;
    store.get_session(&parsed).map(|_| ()).map_err(|_| {
        format!(
            "会话不存在：{session_id}。先用 `axon sessions list` 查看可用会话；\
             --session 不会隐式新建会话"
        )
    })
}

/// 执行一次 `axon run`，并在显式 `--session` 时保住 resume 指针。
///
/// `--session` 是「这一轮去哪个会话」，不是「把默认会话换成它」：如果不还原指针，
/// 一次跨会话追问就会悄悄把 `--continue` 的默认目标改掉，用户下次直接 `--continue`
/// 会落到一个他并没有选中过的会话。
pub(crate) fn run_with_selection_restore(
    cli: &AxonCli,
    command: &RunCommand,
    run_json_writer: Option<&mut dyn IoWrite>,
) -> CliRunResult {
    if command.session.is_none() {
        return run_command_from_cli(cli, command, run_json_writer);
    }
    let config_path =
        crate::management_commands::resolved_config_path(cli.global.config.as_deref()).ok();
    let snapshot = config_path
        .as_deref()
        .and_then(|path| crate::run_commands::snapshot_selected_session(path).ok());
    let result = run_command_from_cli(cli, command, run_json_writer);
    if let (Some(path), Some(snapshot)) = (config_path.as_deref(), snapshot)
        && let Err(error) = crate::run_commands::restore_selected_session(path, snapshot.as_deref())
    {
        axon_logger::warning(&format!("恢复 resume 指针失败: {error}"));
    }
    result
}

#[cfg(test)]
#[path = "tests/explicit_session_tests.rs"]
mod explicit_session_tests;
