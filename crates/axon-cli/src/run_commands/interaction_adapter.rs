//! CLI/TUI 统一 Interaction envelope 适配器。

use std::path::Path;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{
    RuntimeStore, SqliteRuntimeStore, WorkspaceRecord, WorkspaceResolver, WorkspaceStore,
    session_lane_id,
};
use uuid::Uuid;

const INTERACTION_ID_PREFIX: &str = "interaction-";

/// 已完成 Workspace 解析的 Interaction 提交输入。
#[derive(Debug, Clone)]
pub(crate) struct PreparedInteraction {
    pub(crate) interaction: InteractionRecord,
    pub(crate) workspace: WorkspaceRecord,
}

/// 从入口文本构造统一 Interaction envelope。
pub(crate) fn interaction_from_prompt(
    channel: &str,
    destination: &str,
    prompt: &str,
    cwd: &Path,
    now_ms: u64,
) -> Result<InteractionRecord, String> {
    prepare_interaction_from_prompt(channel, destination, prompt, cwd, now_ms)
        .map(|prepared| prepared.interaction)
}

/// 从入口文本构造 Interaction，并保留同次解析得到的 Workspace。
pub(crate) fn prepare_interaction_from_prompt(
    channel: &str,
    destination: &str,
    prompt: &str,
    cwd: &Path,
    now_ms: u64,
) -> Result<PreparedInteraction, String> {
    let workspace = WorkspaceResolver
        .resolve(cwd, now_ms)
        .map_err(|error| error.to_string())?;
    let interaction_uuid = Uuid::new_v4().to_string();
    let interaction_id = InteractionId::new(format!("{INTERACTION_ID_PREFIX}{interaction_uuid}"))
        .map_err(|error| error.to_string())?;
    let source =
        InteractionSource::new(channel, interaction_uuid).map_err(|error| error.to_string())?;
    let scope =
        ScopeRef::workspace(workspace.workspace_id.clone()).map_err(|error| error.to_string())?;
    let interaction = InteractionRecord::new(
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

/// 解析当前会话表面的复合车道运行窗口身份。
///
/// 飞书、TUI 与 CLI 共用同一条派生规则：Workspace、trust scope 与
/// channel/destination 共同决定 Session，Session 再决定 Provider rollout 身份。
/// 已有 Session 直接加载其 `run_session_id`，只有首次进入该复合车道才创建，
/// 使同表面的相邻输入延续同一段上下文，而不是各起一段新历史。
pub(crate) fn conversation_lane_run_session_id(
    cwd: &Path,
    config_path: Option<&Path>,
    now_ms: u64,
    channel: &str,
    destination: &str,
) -> Result<String, String> {
    let workspace = WorkspaceResolver
        .resolve(cwd, now_ms)
        .map_err(|error| error.to_string())?;
    let session_id = DaemonSessionId::new(session_lane_id(
        &workspace.workspace_id,
        &AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        channel,
        destination,
    ))
    .map_err(|error| error.to_string())?;
    let store = interaction_store(config_path)?;
    // Daemon 主 turn 要求 Interaction 对应 Workspace 已注册，这里与 Session 一并落库。
    store
        .register_workspace(&workspace)
        .map_err(|error| error.to_string())?;
    store
        .create_session(&session_id, &workspace.workspace_root, now_ms)
        .map(|runtime| runtime.run_session_id)
        .map_err(|error| error.to_string())
}

fn interaction_store(config_path: Option<&Path>) -> Result<SqliteRuntimeStore, String> {
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
    use super::{conversation_lane_run_session_id, interaction_from_prompt, interaction_store};

    use crate::run_commands::{CLI_INTERACTION_CHANNEL, cli_reply_destination};
    use axon_session::RuntimeStore;

    #[test]
    fn cli_envelope_uses_a_process_scoped_terminal_route() {
        let cwd = axon_test_support::test_temp_dir();
        let destination = cli_reply_destination();
        let interaction = interaction_from_prompt(
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
        let first = conversation_lane_run_session_id(
            root.path(),
            Some(&config_path),
            1,
            CLI_INTERACTION_CHANNEL,
            "terminal-a",
        )
        .expect("首个 CLI 进程");
        let second = conversation_lane_run_session_id(
            root.path(),
            Some(&config_path),
            2,
            CLI_INTERACTION_CHANNEL,
            "terminal-b",
        )
        .expect("第二个 CLI 进程");

        assert_ne!(
            first, second,
            "每次 axon run 必须独占一条车道，不得共用运行窗口"
        );
    }

    #[test]
    fn workspace_lane_loads_existing_run_window_instead_of_creating_one() {
        let root = axon_test_support::test_temp_dir();
        let config_path = root.path().join("isolated-config.toml");
        std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").expect("写入配置");

        let first = conversation_lane_run_session_id(
            root.path(),
            Some(&config_path),
            1,
            CLI_INTERACTION_CHANNEL,
            cli_reply_destination(),
        )
        .expect("首次进入 Workspace");
        let interaction = interaction_from_prompt(
            CLI_INTERACTION_CHANNEL,
            cli_reply_destination(),
            "检查仓库",
            root.path(),
            1,
        )
        .expect("构造 CLI Interaction");
        // CLI 与 Daemon 必须解析到同一条车道，否则两个入口会各自维护一段历史。
        let lane = axon_daemon::primary_session_id_for_interaction(&interaction)
            .expect("Daemon 车道身份一致");
        let runtime = interaction_store(Some(&config_path))
            .expect("打开运行库")
            .get_session(&lane)
            .expect("车道 Session 已建立");
        assert_eq!(runtime.session_id, lane);
        assert_eq!(runtime.run_session_id, first);

        let second = conversation_lane_run_session_id(
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

        let first = conversation_lane_run_session_id(
            root.path(),
            Some(&config_path),
            1,
            CLI_INTERACTION_CHANNEL,
            cli_reply_destination(),
        )
        .expect("首个 Workspace");
        let second = conversation_lane_run_session_id(
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
