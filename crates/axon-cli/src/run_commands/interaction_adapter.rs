//! CLI/TUI 统一 Interaction envelope 适配器。

use std::path::Path;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{
    RuntimeStore, SessionTrustScope, SqliteRuntimeStore, WorkspaceRecord, WorkspaceResolver,
    WorkspaceSessionRequest, WorkspaceStore,
};
use uuid::Uuid;

/// 已完成 Workspace 解析的 Interaction 提交输入。
#[derive(Debug, Clone)]
pub(crate) struct PreparedInteraction {
    pub(crate) interaction: InteractionRecord,
    pub(crate) workspace: WorkspaceRecord,
}

/// 带客户端来源的构造函数。
///
/// `session` 是调用方显式给出的会话标识：给出时必须是该 Workspace 下已登记的会话，
/// 未登记直接报错、不隐式新建；为 `None` 时解析该 Workspace 的活跃会话，没有才新建。
/// `client` 是按下发送的那一端（cli / desktop / tui / cron）。
pub(crate) fn prepare_interaction_from_prompt_as(
    session: Option<&DaemonSessionId>,
    cwd: &Path,
    config_path: Option<&Path>,
    channel: &str,
    destination: &str,
    prompt: &str,
    now_ms: u64,
    client: Option<&str>,
) -> Result<PreparedInteraction, String> {
    let workspace = WorkspaceResolver
        .resolve(cwd, now_ms)
        .map_err(|error| error.to_string())?;
    // 交互标识由服务端生成（ULID）；来源幂等键仍是一次性的消息标识。
    let message_id = Uuid::new_v4().to_string();
    let interaction_id = axon_session::new_interaction_id();
    let mut source =
        InteractionSource::new(channel, message_id).map_err(|error| error.to_string())?;
    if let Some(client) = client {
        source = source
            .with_client(client)
            .map_err(|error| error.to_string())?;
    }
    let scope =
        ScopeRef::workspace(workspace.workspace_id.clone()).map_err(|error| error.to_string())?;
    let store = interaction_store(config_path)?;
    store
        .register_workspace(&workspace)
        .map_err(|error| error.to_string())?;
    let session_id = match session {
        Some(session_id) => {
            let record = store
                .get_session(session_id)
                .map_err(|error| format!("会话不可用：{error}"))?;
            // CLI 只续接主会话：定时任务、目标看板与子代理会话不对外提供入口。
            if record.kind != axon_session::SessionKind::Primary {
                return Err(format!(
                    "无法续接会话 {}：只有主会话可以续聊",
                    session_id.as_str()
                ));
            }
            if record.workspace_id.as_deref() != Some(workspace.workspace_id.as_str()) {
                return Err(format!(
                    "无法续接会话 {}：该会话属于其他工作区",
                    session_id.as_str()
                ));
            }
            record.session_id
        }
        None => {
            store
                .resolve_workspace_session(&WorkspaceSessionRequest {
                    workspace_id: workspace.workspace_id.clone(),
                    cwd: workspace.workspace_root.clone(),
                    trust_scope: SessionTrustScope::Private,
                    now_ms,
                })
                .map_err(|error| error.to_string())?
                .session_id
        }
    };
    let interaction = InteractionRecord::new(
        session_id,
        interaction_id,
        source,
        scope,
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new(channel, destination).map_err(|error| error.to_string())?,
        prompt,
        now_ms,
    )
    .map_err(|error| error.to_string())?;
    Ok(PreparedInteraction {
        interaction,
        workspace,
    })
}

pub(crate) fn register_workspace(
    cwd: &Path,
    now_ms: u64,
    config_path: Option<&Path>,
) -> Result<WorkspaceRecord, String> {
    let workspace = WorkspaceResolver
        .resolve(cwd, now_ms)
        .map_err(|error| error.to_string())?;
    register_workspace_record(&workspace, config_path)?;
    Ok(workspace)
}

pub(crate) fn register_workspace_record(
    workspace: &WorkspaceRecord,
    config_path: Option<&Path>,
) -> Result<(), String> {
    interaction_store(config_path)?
        .register_workspace(workspace)
        .map_err(|error| error.to_string())
}

/// 解析当前 Workspace 的会话运行窗口身份。
///
/// 会话身份来自 `sessions` 表：该 Workspace 已有的最近活动主会话直接复用，没有时才新建
/// 一条。渠道与投递目标只决定投递，不再参与会话身份。
pub(crate) fn conversation_session_run_session_id(
    cwd: &Path,
    config_path: Option<&Path>,
    now_ms: u64,
    _channel: &str,
    _destination: &str,
) -> Result<String, String> {
    let workspace = WorkspaceResolver
        .resolve(cwd, now_ms)
        .map_err(|error| error.to_string())?;
    let store = interaction_store(config_path)?;
    // Daemon 主 turn 要求 Interaction 对应 Workspace 已注册，这里与 Session 一并落库。
    store
        .register_workspace(&workspace)
        .map_err(|error| error.to_string())?;
    store
        .resolve_workspace_session(&WorkspaceSessionRequest {
            workspace_id: workspace.workspace_id.clone(),
            cwd: workspace.workspace_root.clone(),
            trust_scope: SessionTrustScope::Private,
            now_ms,
        })
        .map(|runtime| runtime.current_run_id)
        .map_err(|error| error.to_string())
}

pub(crate) fn interaction_store(config_path: Option<&Path>) -> Result<SqliteRuntimeStore, String> {
    let data_root = match config_path {
        Some(path) => {
            crate::runtime_paths::runtime_axon_data_path_static(path).map_err(str::to_owned)?
        }
        None => {
            let path = axon_config::ConfigLoader::default_user_config_path()
                .map_err(|_| "无法解析用户配置路径".to_owned())?;
            crate::runtime_paths::runtime_axon_data_path_static(&path).map_err(str::to_owned)?
        }
    };
    Ok(SqliteRuntimeStore::from_root(data_root))
}

#[cfg(test)]
mod tests {
    use super::{
        conversation_session_run_session_id, interaction_store, prepare_interaction_from_prompt_as,
    };

    use crate::run_commands::{CLI_INTERACTION_CHANNEL, cli_reply_destination};
    use axon_session::RuntimeStore;

    fn interaction_from_prompt(
        config_path: Option<&std::path::Path>,
        channel: &str,
        destination: &str,
        prompt: &str,
        cwd: &std::path::Path,
        now_ms: u64,
    ) -> Result<axon_domain::InteractionRecord, String> {
        prepare_interaction_from_prompt_as(
            None,
            cwd,
            config_path,
            channel,
            destination,
            prompt,
            now_ms,
            None,
        )
        .map(|prepared| prepared.interaction)
    }

    #[test]
    fn cli_envelope_uses_a_process_scoped_terminal_route() {
        let cwd = axon_test_support::test_temp_dir();
        // 运行库必须落在临时根：测试绝不能写进真实用户的运行态库。
        let config_path = cwd.path().join("isolated-config.toml");
        std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").expect("写入配置");
        let destination = cli_reply_destination();
        let interaction = interaction_from_prompt(
            Some(&config_path),
            CLI_INTERACTION_CHANNEL,
            destination,
            "检查仓库",
            cwd.path(),
            42,
        )
        .expect("构造 CLI Interaction");

        assert_eq!(interaction.source.channel(), "cli");
        assert_eq!(interaction.reply_route.channel(), "cli");
        assert_eq!(interaction.reply_route.destination(), destination);
        assert!(
            destination.starts_with("terminal-"),
            "CLI 会话表面 ID 必须带 terminal- 前缀：{destination}"
        );
        // 提交路径与车道解析路径必须落到同一条车道，因此同一进程内取值必须稳定。
        assert_eq!(cli_reply_destination(), destination);
        assert!(interaction.scope.workspace_id().is_some());
        assert_eq!(interaction.content, "检查仓库");
        assert_eq!(interaction.received_at_ms, 42);
    }

    #[test]
    fn concurrent_cli_surfaces_do_not_share_a_run_window() {
        let root = axon_test_support::test_temp_dir();
        let config_path = root.path().join("isolated-config.toml");
        std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").expect("写入配置");

        // 模拟两个独立 CLI 进程：各自持有不同的会话表面 ID。
        let first = conversation_session_run_session_id(
            root.path(),
            Some(&config_path),
            1,
            CLI_INTERACTION_CHANNEL,
            "terminal-a",
        )
        .expect("首个 CLI 进程");
        let second = conversation_session_run_session_id(
            root.path(),
            Some(&config_path),
            2,
            CLI_INTERACTION_CHANNEL,
            "terminal-b",
        )
        .expect("第二个 CLI 进程");

        assert_eq!(
            first, second,
            "同一 Workspace 的连续 axon run 必须复用同一条会话的运行窗口"
        );
    }

    #[test]
    fn workspace_session_loads_existing_run_window_instead_of_creating_one() {
        let root = axon_test_support::test_temp_dir();
        let config_path = root.path().join("isolated-config.toml");
        std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").expect("写入配置");

        let first = conversation_session_run_session_id(
            root.path(),
            Some(&config_path),
            1,
            CLI_INTERACTION_CHANNEL,
            cli_reply_destination(),
        )
        .expect("首次进入 Workspace");
        let interaction = interaction_from_prompt(
            Some(&config_path),
            CLI_INTERACTION_CHANNEL,
            cli_reply_destination(),
            "检查仓库",
            root.path(),
            1,
        )
        .expect("构造 CLI Interaction");
        // CLI 与 Daemon 必须解析到同一条车道，否则两个入口会各自维护一段历史。
        let session = axon_daemon::primary_session_id_for_interaction(&interaction)
            .expect("Daemon 车道身份一致");
        let runtime = interaction_store(Some(&config_path))
            .expect("打开运行库")
            .get_session(&session)
            .expect("车道 Session 已建立");
        assert_eq!(runtime.session_id, session);
        assert_eq!(runtime.current_run_id, first);

        let second = conversation_session_run_session_id(
            root.path(),
            Some(&config_path),
            2,
            CLI_INTERACTION_CHANNEL,
            cli_reply_destination(),
        )
        .expect("再次进入 Workspace");
        assert_eq!(second, first, "同一复合车道必须复用已有运行窗口");
    }

    #[test]
    fn distinct_workspaces_do_not_share_run_window() {
        let root = axon_test_support::test_temp_dir();
        let other = axon_test_support::test_temp_dir();
        let config_path = root.path().join("isolated-config.toml");
        std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").expect("写入配置");

        let first = conversation_session_run_session_id(
            root.path(),
            Some(&config_path),
            1,
            CLI_INTERACTION_CHANNEL,
            cli_reply_destination(),
        )
        .expect("首个 Workspace");
        let second = conversation_session_run_session_id(
            other.path(),
            Some(&config_path),
            2,
            "cli",
            "terminal",
        )
        .expect("另一个 Workspace");
        assert_ne!(first, second, "不同 Workspace 不得共用运行窗口");
    }
}
