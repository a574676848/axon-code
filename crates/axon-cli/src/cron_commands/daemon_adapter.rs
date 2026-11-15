//! Daemon 侧 Cron 执行适配器的装配与实现。
//!
//! Cron 结果只通过 Daemon 原语交付：投递项与唤醒回合的 durable 事实都归 Daemon 所有，
//! 因此这里只负责把「任务 + 启动时刻」派生成稳定的提交身份并转交 IPC，不在调用端
//! 制造任何本地落库的假象。

use std::path::{Path, PathBuf};

use axon_crypto::sha256_hex_prefix;
use axon_daemon::infrastructure::ipc::JsonlIpcClient;
use axon_domain::{
    AudienceContext, AudienceKind, CronJob, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef,
};
use axon_protocol::{DaemonCommand, DaemonRequest, SubmitInteractionRequest};
use axon_session::WorkspaceResolver;

use crate::{
    cron_commands::{CRON_WAKE_SOURCE_PREFIX, CronExecutionAdapter, execute_local_cron_agent},
    runtime_paths::daemon_endpoint_for_root,
};

const CRON_DAEMON_CLIENT_ID: &str = "axon-cron";

/// 按配置路径装配 Cron 适配器。
pub(crate) fn load_cron_adapter(config_path: Option<&Path>) -> Result<CronDaemonAdapter, String> {
    let path = crate::management_commands::resolved_config_path(config_path)?;
    // 与主 turn 一致的凭据来源：`--wake session:` 注入的目标车道可能属于另一个项目，
    // 其配置里的环境变量必须同样可见。
    crate::run_commands::load_run_env_file(&crate::setup_commands::env_path_for_config(&path))?;
    let axon_root = crate::runtime_paths::runtime_axon_root_path(&path)?;
    Ok(CronDaemonAdapter::new(axon_root, path))
}

/// 通过 Daemon IPC 执行 Cron Agent 任务并向目标车道注入一轮主 turn。
pub(crate) struct CronDaemonAdapter {
    axon_root: PathBuf,
    config_path: PathBuf,
}

impl CronDaemonAdapter {
    pub(crate) fn new(axon_root: impl Into<PathBuf>, config_path: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
            config_path: config_path.into(),
        }
    }

    /// 通过 Daemon IPC 发送一条命令并返回成功响应正文。
    fn send_command(
        &self,
        request_id: String,
        command: DaemonCommand,
    ) -> Result<serde_json::Value, String> {
        let request = daemon_request(request_id, command);
        let endpoint = daemon_endpoint_for_root(&self.axon_root);
        let mut client = JsonlIpcClient::connect(&endpoint)
            .map_err(|error| format!("连接 Daemon IPC 失败：{error}"))?;
        let response = client
            .send(&request)
            .map_err(|error| format!("发送 Daemon 命令失败：{error}"))?;
        if !response.ok {
            return Err(format!("Daemon 拒绝命令：{}", response.body));
        }
        Ok(response.body)
    }

    fn workdir<'a>(&'a self, job: &'a CronJob) -> &'a Path {
        job.workdir
            .as_deref()
            .map(Path::new)
            .unwrap_or(&self.axon_root)
    }

    /// 解析并注册任务所属 Workspace；唤醒必须绑定已注册的 Workspace。
    fn resolve_workspace_id(&self, job: &CronJob, now_ms: u64) -> Result<String, String> {
        // 会话唤醒必须落在任务创建时的那条车道上：车道身份含 workspace 维度，
        // 用任务执行目录反推会把结果投进另一个项目的同名会话。
        if let Some(workspace_id) = job
            .wake
            .as_ref()
            .and_then(axon_domain::CronWakeTarget::workspace)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(workspace_id.to_owned());
        }
        let workdir = self.workdir(job);
        crate::run_commands::register_workspace(workdir, now_ms, Some(&self.config_path))?;
        WorkspaceResolver
            .resolve(workdir, now_ms)
            .map(|workspace| workspace.workspace_id.as_str().to_owned())
            .map_err(|error| format!("解析 Cron 任务 Workspace 失败：{error}"))
    }

    /// 向目标复合车道提交一次主 turn 注入。
    ///
    /// 身份按「任务 + 启动时刻」稳定派生：同一轮重复提交命中来源幂等键，不会在目标
    /// 会话里刷出两轮相同的唤醒回合。
    fn submit_wake_turn(
        &self,
        job: &CronJob,
        channel: &str,
        destination: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String> {
        let workspace_id = self.resolve_workspace_id(job, now_ms)?;
        let identity = format!("{}:{now_ms}", job.job_id);
        let interaction_id = InteractionId::new(format!(
            "cron-wake-{}",
            sha256_hex_prefix(identity.as_bytes(), 20)
        ))
        .map_err(|error| error.to_string())?;
        let source =
            InteractionSource::new(channel, format!("{CRON_WAKE_SOURCE_PREFIX}{identity}"))
                .map_err(|error| error.to_string())?;
        let scope = ScopeRef::workspace(workspace_id).map_err(|error| error.to_string())?;
        let route = ReplyRoute::new(channel, destination).map_err(|error| error.to_string())?;
        let interaction = InteractionRecord::new(
            interaction_id,
            source,
            scope,
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            route,
            body,
            now_ms,
        )
        .map_err(|error| error.to_string())?;
        let body_json = self.send_command(
            format!("{CRON_DAEMON_CLIENT_ID}-wake-{identity}"),
            DaemonCommand::SubmitInteraction(SubmitInteractionRequest { interaction }),
        )?;
        body_json
            .get("interactionId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "Daemon 唤醒提交回执缺少 interactionId".to_owned())
    }
}

impl CronExecutionAdapter for CronDaemonAdapter {
    fn axon_root(&self) -> &Path {
        &self.axon_root
    }

    fn config_path(&self) -> &Path {
        &self.config_path
    }

    fn execute_agent(&self, job: &CronJob, prompt: &str) -> Result<String, String> {
        execute_local_cron_agent(&self.config_path, &self.axon_root, job, prompt)
    }

    fn wake_session(
        &self,
        job: &CronJob,
        channel: &str,
        destination: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String> {
        self.submit_wake_turn(job, channel, destination, body, now_ms)
    }
}

fn daemon_request(request_id: String, command: DaemonCommand) -> DaemonRequest {
    DaemonRequest {
        message_type: "request".to_owned(),
        request_id,
        protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
        client_id: CRON_DAEMON_CLIENT_ID.to_owned(),
        session_id: None,
        command,
    }
}
