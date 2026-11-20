//! Daemon 侧 Cron 执行适配器的装配与实现。
//!
//! Cron 结果只通过 Daemon 原语交付：投递项与唤醒回合的 durable 事实都归 Daemon 所有，
//! 因此这里只负责把「任务 + 启动时刻」派生成稳定的提交身份并转交 IPC，不在调用端
//! 制造任何本地落库的假象。

use std::path::{Path, PathBuf};

use axon_daemon::infrastructure::ipc::JsonlIpcClient;
use axon_domain::{
    AudienceContext, AudienceKind, CronJob, DisclosurePolicy, InteractionRecord, InteractionSource,
    ReplyRoute, ScopeRef,
};
use axon_protocol::{DaemonCommand, DaemonRequest, SubmitInteractionRequest};
use axon_session::RuntimeStore as _;

use crate::{
    cron_commands::{
        CRON_AGENT_CHANNEL, CRON_AGENT_CLIENT, CRON_WAKE_SOURCE_PREFIX, CronExecutionAdapter,
        execute_local_cron_agent,
    },
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
            .send_with_timeout(&request, crate::daemon_commands::DAEMON_IPC_TIMEOUT)
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

    /// 向目标会话提交一次主 turn 注入。
    ///
    /// 身份按「任务 + 启动时刻」稳定派生：同一轮重复提交命中来源幂等键，不会在目标
    /// 会话里刷出两轮相同的唤醒回合。目标会话必须是已登记的既有会话：会话自带
    /// Workspace，唤醒不再按任务执行目录反推归属。
    fn submit_wake_turn(
        &self,
        job: &CronJob,
        session_id: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String> {
        let identity = format!("{}:{now_ms}", job.job_id);
        let store = crate::run_commands::interaction_store(Some(&self.config_path))?;
        let session_id = axon_domain::DaemonSessionId::new(session_id.to_owned())
            .map_err(|error| format!("唤醒目标会话标识无效：{error}"))?;
        let record = store
            .get_session(&session_id)
            .map_err(|error| format!("唤醒目标会话不可用：{error}"))?;
        if record.kind == axon_session::SessionKind::Cron {
            return Err("唤醒目标不能是 Cron 自身会话".to_owned());
        }
        let workspace_id = match record.workspace_id.as_deref() {
            Some(owner) => owner.to_owned(),
            None => crate::run_commands::register_workspace(
                self.workdir(job),
                now_ms,
                Some(&self.config_path),
            )?
            .workspace_id
            .as_str()
            .to_owned(),
        };
        // 交互标识由服务端生成（ULID）：幂等由来源键 `cron-wake:{identity}` 承担，
        // 同一轮重复提交命中已有行并拿回它的标识。
        let interaction_id = axon_session::new_interaction_id();
        let source = InteractionSource::new(
            CRON_AGENT_CHANNEL,
            format!("{CRON_WAKE_SOURCE_PREFIX}{identity}"),
        )
        .and_then(|source| source.with_client(CRON_AGENT_CLIENT))
        .map_err(|error| error.to_string())?;
        let scope = ScopeRef::workspace(workspace_id).map_err(|error| error.to_string())?;
        let route = ReplyRoute::new(CRON_AGENT_CHANNEL, session_id.as_str())
            .map_err(|error| error.to_string())?;
        let interaction = InteractionRecord::new(
            session_id,
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
        session_id: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String> {
        self.submit_wake_turn(job, session_id, body, now_ms)
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
