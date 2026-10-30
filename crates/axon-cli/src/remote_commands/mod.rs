use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use clap::{Args, Subcommand};

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, resolved_config_path},
    run_commands::load_run_env_file,
    setup_commands::env_path_for_config,
};

mod cron_feishu_adapter;
mod remote_feishu;
mod remote_feishu_app_lock;
mod remote_feishu_completion_inbox;
pub(crate) mod remote_feishu_daemon_egress;
mod remote_feishu_daemon_feedback;
mod remote_feishu_delivery;
mod remote_feishu_gateway;
pub(crate) mod remote_feishu_gateway_adapter;
mod remote_feishu_gateway_closure;
mod remote_feishu_gateway_report;
mod remote_feishu_gateway_status;
mod remote_feishu_gateway_worker;
mod remote_feishu_identity;
mod remote_feishu_ingress_watermark;
mod remote_feishu_progress_mailbox;
mod remote_feishu_progress_model;
mod remote_feishu_progress_projection;
pub(crate) mod remote_feishu_progress_service;
pub(crate) mod remote_feishu_question_action;
pub(crate) mod remote_feishu_question_card_tick;
pub(crate) mod remote_feishu_question_delivery;
pub(crate) mod remote_feishu_question_sink;
pub(crate) mod remote_feishu_question_store;
mod remote_feishu_reply_cache;
mod remote_feishu_reply_resolver;
mod remote_feishu_sdk_loop;
mod remote_feishu_simulation;
pub(crate) mod remote_feishu_transport_context;
mod remote_feishu_worker;
mod remote_feishu_worklist;
mod remote_gateway_restart_monitor;
mod remote_render;
mod remote_support;
use crate::runtime_paths::runtime_axon_root_path;
use axon_remote_runtime::RemoteSubscriptionStoreRuntime;

pub(crate) const REMOTE_FEISHU_DAEMON_ENDPOINT: &str = "axon-daemon";
pub(crate) use cron_feishu_adapter::FeishuCronAdapter;
use remote_feishu::{run_remote_feishu_status, run_remote_render_feishu};
pub(crate) use remote_feishu_daemon_feedback::run_daemon_feedback_tick;
use remote_feishu_delivery::run_remote_deliver_feishu;
pub(crate) use remote_feishu_question_sink::warm_feishu_question_card_sink;
pub(crate) use remote_feishu_sdk_loop::{
    run_remote_execute_feishu_sdk_loop, run_remote_execute_feishu_sdk_loop_with_cancellation,
};
use remote_feishu_simulation::run_remote_simulate_feishu;
pub(crate) use remote_feishu_worker::run_daemon_delivery_tick;
use remote_feishu_worklist::run_remote_feishu_worklist;
use remote_render::{
    render_audit, render_outbox, render_revoke, render_status, render_subscriptions,
};
use remote_support::{
    REMOTE_CLI_REVOKE_SOURCE_TRACE, RemoteAuditReport, RemoteCliContext, RemoteOutboxReport,
    RemoteRevokeReport, RemoteStatusReport, RemoteSubscriptionsReport, active_subscriptions,
    bounded_limit, display_path, latest_subscription_for, latest_subscriptions, now_ms,
    outbox_pending, outbox_view, read_outbox, read_subscriptions, revoke_reason_ref,
    subscription_view,
};

pub(crate) use remote_feishu_completion_inbox::{
    drain_feishu_completion_inbox, feishu_channel_enabled,
};

type DeliveryWakeHandler = dyn Fn() + Send + Sync + 'static;
static DELIVERY_WAKE_HANDLER: OnceLock<Arc<DeliveryWakeHandler>> = OnceLock::new();

pub(crate) fn register_delivery_waker(handler: Arc<DeliveryWakeHandler>) {
    let _ = DELIVERY_WAKE_HANDLER.set(handler);
}

pub(crate) fn notify_delivery_waker() {
    if let Some(handler) = DELIVERY_WAKE_HANDLER.get() {
        handler();
    }
}

/// `axon remote` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RemoteCommand {
    /// remote 子命令。
    #[command(subcommand)]
    pub command: Option<RemoteSubcommand>,
}

/// `axon remote` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum RemoteSubcommand {
    /// 查看远控本机治理状态。
    Status,
    /// 列出本机远控订阅。
    Subscriptions(RemoteListCommand),
    /// 列出本机远控通知 outbox。
    Outbox(RemoteListCommand),
    /// 撤销本机远控订阅。
    Revoke(RemoteRevokeCommand),
    /// 导出本机远控审计摘要。
    Audit(RemoteListCommand),
    /// 查看 Feishu WebSocket 远控 adapter 状态。
    FeishuStatus,
    /// 将远控 outbox 渲染为指定 adapter 载荷。
    RenderFeishu(RemoteRenderCommand),
    /// 将 Feishu outbox 转为真实 delivery 请求计划。
    DeliverFeishu(RemoteRenderCommand),
    /// 执行 Feishu SDK loop daemon 入口。
    ExecuteFeishuSdkLoop(RemoteFeishuSdkLoopCommand),
    /// 查看 Feishu 高频工作台视图。
    FeishuWorklist(RemoteListCommand),
    /// 模拟 Feishu WebSocket 入站并输出本地响应计划。
    SimulateFeishu(RemoteSimulateCommand),
}

/// 远控列表参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RemoteListCommand {
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
}

/// 远控撤销参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RemoteRevokeCommand {
    /// 会话 ID。
    pub session_id: u64,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 本机撤销原因。
    #[arg(long)]
    pub reason: Option<String>,
}

/// 远控渲染参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RemoteRenderCommand {
    /// 目标会话 ID。
    pub session_id: u64,
    /// 取多少条 pending 记录生成渲染批次。
    #[arg(long)]
    pub limit: Option<usize>,
    /// 是否真正执行 Feishu token/message API。
    #[arg(long)]
    pub execute: bool,
    /// 显式 app_secret，仅供本机联调用；为空时从环境变量读取。
    #[arg(long)]
    pub app_secret: Option<String>,
    /// 后台 worker 轮询间隔毫秒。
    #[arg(long)]
    pub interval_ms: Option<u64>,
    /// 后台 worker 最多等待多少毫秒后返回。
    #[arg(long)]
    pub wait_ms: Option<u64>,
}

/// Feishu gateway 前置 runner 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RemoteGatewayCommand {
    /// 目标会话 ID。
    pub session_id: u64,
    /// 单条或数组形式的 Feishu 入站事件 JSON。
    #[arg(long)]
    pub event_json: Option<String>,
    /// 单条或数组形式的 Feishu 入站事件 JSON 文件。
    #[arg(long)]
    pub event_file: Option<std::path::PathBuf>,
    /// 以 daemon shell 模式循环读取事件源。
    #[arg(long)]
    pub daemon: bool,
    /// daemon shell 轮询间隔毫秒。
    #[arg(long)]
    pub interval_ms: Option<u64>,
    /// daemon shell 最大轮询次数；为空时持续运行。
    #[arg(long)]
    pub max_ticks: Option<usize>,
    /// daemon shell cursor/ack 状态文件；为空时使用事件源旁路 `.cursor.json`。
    #[arg(long)]
    pub cursor_file: Option<std::path::PathBuf>,
    /// 通用 Daemon IPC endpoint；Gateway 始终通过该端点提交持久 Action。
    #[arg(long, default_value = REMOTE_FEISHU_DAEMON_ENDPOINT)]
    pub daemon_endpoint: String,
}

/// Feishu SDK loop daemon 入口参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RemoteFeishuSdkLoopCommand {
    /// 目标会话 ID。
    pub session_id: u64,
    /// SDK 回调 payload JSONL 文件，每行包含 kind 与 payload；未启用 --connect-sdk 时必填。
    #[arg(long)]
    pub payload_file: Option<std::path::PathBuf>,
    /// 写入 gateway daemon 消费的 JSONL 事件源；为空时使用默认事件源。
    #[arg(long)]
    pub event_source_file: Option<std::path::PathBuf>,
    /// SDK loop daemon cursor/ack 状态文件；为空时使用 payload 源旁路 `.cursor.json`。
    #[arg(long)]
    pub cursor_file: Option<std::path::PathBuf>,
    /// SDK loop daemon 轮询间隔毫秒。
    #[arg(long)]
    pub interval_ms: Option<u64>,
    /// SDK loop daemon 最大轮询次数；为空时持续运行。
    #[arg(long)]
    pub max_ticks: Option<usize>,
    /// 真实 Feishu WS SDK loop 最多处理多少个入站事件；仅 --connect-sdk 生效。
    #[arg(long)]
    pub max_events: Option<usize>,
    /// 真实 Feishu WS SDK loop 处理首个入站事件并 ack 后退出；仅 --connect-sdk 生效。
    #[arg(long)]
    pub stop_after_first_event: bool,
    /// 真实 Feishu WS SDK loop 最大等待毫秒；到期后优雅退出并输出连接诊断，仅 --connect-sdk 生效。
    #[arg(long)]
    pub wait_ms: Option<u64>,
    /// 连接真实 Feishu 官方 WebSocket SDK loop。
    #[arg(long)]
    pub connect_sdk: bool,
    /// 显式 app_secret，仅供本机联调用；为空时从 remote.feishu_websocket.app_secret_env 读取。
    #[arg(long)]
    pub app_secret: Option<String>,
}

/// Feishu WebSocket 入站仿真参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RemoteSimulateCommand {
    /// 目标会话 ID。
    pub session_id: u64,
    /// 仿真模式：message 或 card-action。
    pub mode: String,
    /// 文本消息或 card action tag。
    pub value: String,
    /// card-action 模式下的 JSON 字符串；message 模式忽略。
    #[arg(long)]
    pub action_json: Option<String>,
    /// message 模式下覆盖 sender open_id，便于联调 allowlist。
    #[arg(long)]
    pub sender_open_id: Option<String>,
}

pub(crate) fn run_remote(
    command: &RemoteCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let (_, context) = match load_remote_context(config_path) {
        Ok(context) => context,
        Err(error) => return cli_error(error),
    };
    let feishu_context = context.feishu_runtime();
    match command
        .command
        .as_ref()
        .unwrap_or(&RemoteSubcommand::Status)
    {
        RemoteSubcommand::Status => run_remote_status(format, &context),
        RemoteSubcommand::Subscriptions(command) => {
            run_remote_subscriptions(format, &context, command)
        }
        RemoteSubcommand::Outbox(command) => run_remote_outbox(format, &context, command),
        RemoteSubcommand::Revoke(command) => run_remote_revoke(format, &context, command),
        RemoteSubcommand::Audit(command) => run_remote_audit(format, &context, command),
        RemoteSubcommand::FeishuStatus => run_remote_feishu_status(format, &feishu_context),
        RemoteSubcommand::RenderFeishu(command) => {
            run_remote_render_feishu(format, &feishu_context, command)
        }
        RemoteSubcommand::DeliverFeishu(command) => {
            run_remote_deliver_feishu(format, &feishu_context, command)
        }
        RemoteSubcommand::ExecuteFeishuSdkLoop(command) => {
            run_remote_execute_feishu_sdk_loop(format, &feishu_context, command)
        }
        RemoteSubcommand::FeishuWorklist(command) => {
            run_remote_feishu_worklist(format, &feishu_context, command)
        }
        RemoteSubcommand::SimulateFeishu(command) => {
            run_remote_simulate_feishu(format, &feishu_context, command)
        }
    }
}

pub(crate) fn load_remote_context(
    config_path: Option<&Path>,
) -> Result<(std::path::PathBuf, RemoteCliContext), String> {
    let path = resolved_config_path(config_path)?;
    load_run_env_file(&env_path_for_config(&path))?;
    let loaded =
        axon_config::ConfigLoader::load_from_path(&path).map_err(|error| format!("{error:?}"))?;
    let root = runtime_axon_root_path(&path)?;
    let context = RemoteCliContext::new(path.clone(), root, loaded.config.remote.feishu_websocket);
    Ok((path, context))
}

fn run_remote_status(format: OutputFormat, context: &RemoteCliContext) -> CliRunResult {
    let subscriptions = match read_subscriptions(context) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let outbox = match read_outbox(context) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let now_ms = now_ms();
    let active_subscription_records = active_subscriptions(&subscriptions, now_ms).len();
    let pending_outbox_records = outbox
        .iter()
        .filter(|record| outbox_pending(record, now_ms))
        .count();
    let report = RemoteStatusReport {
        status: "remote_ready",
        axon_root: display_path(&context.axon_root),
        remote_runtime_dir: display_path(&context.root),
        subscription_store_path: display_path(&context.subscription_store_path),
        outbox_store_path: display_path(&context.outbox_store_path),
        total_subscription_records: subscriptions.len(),
        active_subscription_records,
        total_outbox_records: outbox.len(),
        pending_outbox_records,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        raw_body_persisted: false,
        raw_payload_persisted: false,
    };
    render_status(format, &report)
}

fn run_remote_subscriptions(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteListCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let subscriptions = match read_subscriptions(context) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let active = active_subscriptions(&subscriptions, now_ms());
    let report = RemoteSubscriptionsReport {
        status: "remote_ready",
        store_path: display_path(&context.subscription_store_path),
        limit,
        total_records: subscriptions.len(),
        active_records: active.len(),
        subscriptions: active.iter().take(limit).map(subscription_view).collect(),
    };
    render_subscriptions(format, &report)
}

fn run_remote_outbox(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteListCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let outbox = match read_outbox(context) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let now_ms = now_ms();
    let pending_records = outbox
        .iter()
        .filter(|record| outbox_pending(record, now_ms))
        .count();
    let report = RemoteOutboxReport {
        status: "remote_ready",
        store_path: display_path(&context.outbox_store_path),
        limit,
        total_records: outbox.len(),
        pending_records,
        dispatches: outbox.iter().take(limit).map(outbox_view).collect(),
    };
    render_outbox(format, &report)
}

fn run_remote_revoke(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteRevokeCommand,
) -> CliRunResult {
    let subscriptions = match read_subscriptions(context) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let Some(latest) =
        latest_subscription_for(&subscriptions, command.session_id, &command.subscriber_id)
    else {
        return cli_error("未找到可撤销的远控订阅");
    };
    if latest.revoked {
        return cli_error("远控订阅已撤销");
    }
    let now_ms = now_ms();
    let mut revoked = latest.clone();
    revoked.recorded_at_ms = now_ms;
    revoked.updated_at_ms = now_ms;
    revoked.revoked = true;
    REMOTE_CLI_REVOKE_SOURCE_TRACE.clone_into(&mut revoked.source_trace);
    revoked.reason = revoke_reason_ref(command.reason.as_deref());
    if let Err(error) =
        RemoteSubscriptionStoreRuntime::append_record(&context.subscription_store_path, &revoked)
    {
        return cli_error(format!("{error:?}"));
    }
    let report = RemoteRevokeReport {
        status: "remote_revoked",
        store_path: display_path(&context.subscription_store_path),
        subscription: subscription_view(&revoked),
        revoked: true,
        network_send_allowed: false,
        raw_body_persisted: false,
        raw_payload_persisted: false,
    };
    render_revoke(format, &report)
}

fn run_remote_audit(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteListCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let subscriptions = match read_subscriptions(context) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let outbox = match read_outbox(context) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let now_ms = now_ms();
    let latest_subscriptions = latest_subscriptions(&subscriptions);
    let active_subscription_records = latest_subscriptions
        .iter()
        .filter(|record| !record.revoked)
        .filter(|record| record.expires_at_ms.is_none_or(|expires| expires > now_ms))
        .count();
    let revoked_subscription_records = latest_subscriptions
        .iter()
        .filter(|record| record.revoked)
        .count();
    let pending_outbox_records = outbox
        .iter()
        .filter(|record| outbox_pending(record, now_ms))
        .count();
    let report = RemoteAuditReport {
        status: "remote_audit_ready",
        subscription_store_path: display_path(&context.subscription_store_path),
        outbox_store_path: display_path(&context.outbox_store_path),
        limit,
        total_subscription_records: subscriptions.len(),
        latest_subscription_records: latest_subscriptions.len(),
        active_subscription_records,
        revoked_subscription_records,
        total_outbox_records: outbox.len(),
        pending_outbox_records,
        subscriptions: latest_subscriptions
            .iter()
            .take(limit)
            .map(subscription_view)
            .collect(),
        dispatches: outbox.iter().take(limit).map(outbox_view).collect(),
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        raw_body_persisted: false,
        raw_payload_persisted: false,
    };
    render_audit(format, &report)
}
