use std::path::Path;

use axon_daemon::infrastructure::ipc::JsonlIpcClient;
use axon_domain::{CRON_DELIVERY_SOURCE_PREFIX, CronDeliveryAdapter, CronDeliveryTarget, CronJob};
use axon_protocol::{DaemonCommand, DaemonRequest, EnqueueDeliveryRequest};
use axon_session::WorkspaceResolver;

use crate::{
    cron_commands::{CronExecutionAdapter, CronExecutionReceipt, execute_local_cron_agent},
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

    /// Cron 运行身份：同时作为主 turn 来源 ID 与投递项身份，保证两者一一对应。
    fn delivery_source_key(job_id: &str, now_ms: u64) -> String {
        format!("{CRON_DELIVERY_SOURCE_PREFIX}{job_id}:{now_ms}")
    }

    /// 通过 Daemon IPC 发送一条命令并返回成功响应正文。
    ///
    /// Cron 投递的 durable 收件箱归 Daemon 所有，因此投递登记必须由 Daemon 执行；
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

    /// 登记一条正文已就绪的 Cron 投递项，并投递到当时所有可用渠道。
    fn enqueue_delivery(&self, job: &CronJob, body: &str, now_ms: u64) -> Result<String, String> {
        let delivery_id = Self::delivery_source_key(&job.job_id, now_ms);
        let workdir = job
            .workdir
            .as_deref()
            .map(Path::new)
            .unwrap_or(&self.context.axon_root);
        crate::run_commands::register_workspace(workdir, now_ms, Some(&self.context.config_path))?;
        let workspace = WorkspaceResolver
            .resolve(workdir, now_ms)
            .map_err(|error| format!("解析 Feishu Cron Workspace 失败：{error}"))?;
        let body_json = self.send_command(
            format!("{CRON_DAEMON_CLIENT_ID}-delivery-{delivery_id}"),
            DaemonCommand::EnqueueDelivery(EnqueueDeliveryRequest {
                workspace_id: workspace.workspace_id.as_str().to_owned(),
                goal_id: job.job_id.clone(),
                delivery_id,
                content: body.to_owned(),
            }),
        )?;
        body_json
            .get("notificationId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "Daemon 投递登记回执缺少 notificationId".to_owned())
    }
}

impl CronExecutionAdapter for FeishuCronAdapter {
    fn axon_root(&self) -> &Path {
        &self.context.axon_root
    }

    fn config_path(&self) -> &Path {
        &self.context.config_path
    }

    fn delivery_target(
        &self,
        recipient_id: String,
        thread_id: Option<String>,
    ) -> CronDeliveryTarget {
        CronDeliveryTarget {
            adapter: CronDeliveryAdapter::Feishu,
            session_id: self.context.gateway_session_id(),
            recipient_id,
            thread_id,
        }
    }

    fn execute_agent(
        &self,
        job: &CronJob,
        prompt: &str,
        now_ms: u64,
    ) -> Result<CronExecutionReceipt, String> {
        let body = execute_local_cron_agent(
            &self.context.config_path,
            &self.context.axon_root,
            job,
            prompt,
        )?;
        let dispatch_id =
            if job.delivery.adapter != CronDeliveryAdapter::None && !body.trim().is_empty() {
                Some(self.enqueue_delivery(job, &body, now_ms)?)
            } else {
                None
            };
        Ok(CronExecutionReceipt {
            agent_executed: true,
            delivery_enqueued: dispatch_id.is_some(),
            stdout_empty: body.trim().is_empty(),
            dispatch_id,
        })
    }

    fn enqueue_reply(&self, job: &CronJob, body: &str, now_ms: u64) -> Result<String, String> {
        self.enqueue_delivery(job, body, now_ms)
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
