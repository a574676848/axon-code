use std::path::Path;

use axon_crypto::sha256_hex_prefix;
use axon_daemon::infrastructure::ipc::JsonlIpcClient;
use axon_domain::{
    AudienceContext, AudienceKind, CRON_DELIVERY_ID_PREFIX, CronJob, DisclosurePolicy,
    InteractionId, InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};
use axon_protocol::{
    DaemonCommand, DaemonRequest, EnqueueDeliveryRequest, SubmitInteractionRequest,
};
use axon_session::WorkspaceResolver;

use crate::{
    cron_commands::{CRON_WAKE_SOURCE_PREFIX, CronExecutionAdapter, execute_local_cron_agent},
    runtime_paths::daemon_endpoint_for_root,
};

use super::remote_support::RemoteCliContext;

const CRON_DAEMON_CLIENT_ID: &str = "axon-feishu-cron";

pub(crate) struct FeishuCronAdapter {
    context: RemoteCliContext,
}

impl FeishuCronAdapter {
    pub(crate) fn new(context: RemoteCliContext) -> Self {
        Self { context }
    }

    /// 投递项身份：同一任务同一启动时刻只登记一次，重复登记命中唯一约束。
    fn delivery_id(job_id: &str, now_ms: u64) -> String {
        format!("{CRON_DELIVERY_ID_PREFIX}{job_id}:{now_ms}")
    }

    /// 通过 Daemon IPC 发送一条命令并返回成功响应正文。
    ///
    /// 投递项与唤醒回合的 durable 事实都归 Daemon 所有，因此必须由 Daemon 落库；
    /// 失败一律向上返回错误，绝不在调用端制造一次「假装已投递」的成功。
    fn send_command(
        &self,
        request_id: String,
        command: DaemonCommand,
    ) -> Result<serde_json::Value, String> {
        let request = daemon_request(request_id, command);
        let endpoint = daemon_endpoint_for_root(&self.context.axon_root);
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
            .unwrap_or(&self.context.axon_root)
    }

    /// 解析并注册任务所属 Workspace；投递与唤醒都必须绑定已注册的 Workspace。
    fn resolve_workspace_id(&self, job: &CronJob, now_ms: u64) -> Result<String, String> {
        let workdir = self.workdir(job);
        crate::run_commands::register_workspace(workdir, now_ms, Some(&self.context.config_path))?;
        WorkspaceResolver
            .resolve(workdir, now_ms)
            .map(|workspace| workspace.workspace_id.as_str().to_owned())
            .map_err(|error| format!("解析 Cron 任务 Workspace 失败：{error}"))
    }

    /// 登记一条正文已就绪的投递项，并定向到指定渠道目标。
    fn enqueue_delivery(
        &self,
        job: &CronJob,
        channel: &str,
        destination: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String> {
        let delivery_id = Self::delivery_id(&job.job_id, now_ms);
        let workspace_id = self.resolve_workspace_id(job, now_ms)?;
        let body_json = self.send_command(
            format!("{CRON_DAEMON_CLIENT_ID}-delivery-{delivery_id}"),
            DaemonCommand::EnqueueDelivery(EnqueueDeliveryRequest {
                workspace_id,
                goal_id: job.job_id.clone(),
                delivery_id,
                channel: Some(channel.to_owned()),
                destination: Some(destination.to_owned()),
                content: body.to_owned(),
            }),
        )?;
        body_json
            .get("notificationId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "Daemon 投递登记回执缺少 notificationId".to_owned())
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

impl CronExecutionAdapter for FeishuCronAdapter {
    fn axon_root(&self) -> &Path {
        &self.context.axon_root
    }

    fn config_path(&self) -> &Path {
        &self.context.config_path
    }

    fn execute_agent(&self, job: &CronJob, prompt: &str) -> Result<String, String> {
        execute_local_cron_agent(
            &self.context.config_path,
            &self.context.axon_root,
            job,
            prompt,
        )
    }

    fn enqueue_reply(
        &self,
        job: &CronJob,
        channel: &str,
        destination: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String> {
        self.enqueue_delivery(job, channel, destination, body, now_ms)
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
