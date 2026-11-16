//! `axon sessions bind` / `unbind`：把无 UI 入站渠道固定到某个会话。
//!
//! 飞书群、外部 webhook 没有客户端状态，入站时必须先回答「这条消息属于哪个会话」。
//! 绑定是这份答案的唯一事实源；未绑定的渠道仍按路由派生会话身份。

use std::path::Path;

use serde::Serialize;

use axon_domain::DaemonSessionId;
use axon_session::{ChannelBindingStore as _, RuntimeStore as _, SqliteRuntimeStore};

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::SessionBindCommand;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelBindingReport {
    status: &'static str,
    channel: String,
    destination: String,
    session_id: String,
    changed: bool,
}

pub(super) fn run_session_bind(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionBindCommand,
) -> CliRunResult {
    let store = match open_store(config_path) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let Some(session_id) = command.session.as_deref() else {
        return cli_error("bind 需要 --session <id>");
    };
    let session = match DaemonSessionId::new(session_id.to_owned()) {
        Ok(session) => session,
        Err(_) => return cli_error(format!("会话 ID 非法：{session_id}")),
    };
    // 绑定到不存在的会话等于把消息投进黑洞，先校验目标存在。
    if store.get_session(&session).is_err() {
        return cli_error(format!(
            "会话不存在：{session_id}。先用 `axon sessions list` 查看可用会话"
        ));
    }
    let now_ms = axon_time::unix_now_ms();
    if let Err(error) = store.bind_channel(
        command.channel.as_str(),
        command.destination.as_str(),
        &session,
        now_ms,
    ) {
        return cli_error(error.to_string());
    }
    let report = ChannelBindingReport {
        status: "bound",
        channel: command.channel.clone(),
        destination: command.destination.clone(),
        session_id: session.as_str().to_owned(),
        changed: true,
    };
    match format {
        OutputFormat::Json => json_result(&report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "已绑定：{} / {} -> {}",
                report.channel, report.destination, report.session_id
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn run_session_unbind(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionBindCommand,
) -> CliRunResult {
    let store = match open_store(config_path) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let changed = match store.unbind_channel(command.channel.as_str(), command.destination.as_str())
    {
        Ok(changed) => changed,
        Err(error) => return cli_error(error.to_string()),
    };
    let report = ChannelBindingReport {
        status: if changed { "unbound" } else { "absent" },
        channel: command.channel.clone(),
        destination: command.destination.clone(),
        session_id: command.session.clone().unwrap_or_default(),
        changed,
    };
    match format {
        OutputFormat::Json => json_result(&report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: if changed {
                format!("已解除：{} / {}", report.channel, report.destination)
            } else {
                format!(
                    "本来就没有绑定：{} / {}",
                    report.channel, report.destination
                )
            },
            stderr: String::new(),
        },
    }
}

fn open_store(config_path: &Path) -> Result<SqliteRuntimeStore, String> {
    let data_root =
        crate::runtime_paths::runtime_axon_data_path_static(config_path).map_err(str::to_owned)?;
    let store = SqliteRuntimeStore::from_root(data_root);
    store.initialize().map_err(|error| error.to_string())?;
    Ok(store)
}
