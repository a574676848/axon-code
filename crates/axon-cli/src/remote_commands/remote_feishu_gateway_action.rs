//! Feishu gateway 接受事件后的 Agent 执行投影与 Typing 生命周期。
//!
//! 本模块只做本地状态判定、Daemon IPC 提交与异步入队；除 IPC 提交本身之外不在
//! gateway worker 线程上执行网络 I/O，避免单条消息的远端等待拖住全部 chat。

use axon_remote_feishu::{
    RemoteFeishuGatewayGovernanceEventReport, RemoteFeishuProcessingOutcome,
    RemoteFeishuProcessingRuntime, RemoteFeishuProcessingStateStoreRuntime,
};
use axon_remote_runtime::{RemoteControlDispatchAction, RemoteProtocolResponsePayload};

use crate::remote_commands::{RemoteCliContext, now_ms};

use super::{remote_feishu_control, remote_feishu_processing_queue};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GatewayAgentExecutionProjection {
    pub(super) required: bool,
    pub(super) executed: bool,
    pub(super) processing_started: bool,
    pub(super) processing_failed: bool,
    pub(super) followup_queued: bool,
    pub(super) session_reset: bool,
    pub(super) run_session_key: Option<String>,
    pub(super) reply_enqueued: bool,
    pub(super) failed: bool,
    pub(super) run_session_id: Option<String>,
    pub(super) reply_dispatch_id: Option<String>,
    pub(super) failure: Option<axon_domain::RuntimeFailure>,
    pub(super) error_summary: Option<String>,
}

pub(super) const fn no_agent_execution() -> GatewayAgentExecutionProjection {
    GatewayAgentExecutionProjection {
        required: false,
        executed: false,
        processing_started: false,
        processing_failed: false,
        followup_queued: false,
        session_reset: false,
        run_session_key: None,
        reply_enqueued: false,
        failed: false,
        run_session_id: None,
        reply_dispatch_id: None,
        failure: None,
        error_summary: None,
    }
}

pub(super) const fn queued_agent_execution() -> GatewayAgentExecutionProjection {
    GatewayAgentExecutionProjection {
        required: true,
        executed: false,
        processing_started: false,
        processing_failed: false,
        followup_queued: false,
        session_reset: false,
        run_session_key: None,
        reply_enqueued: false,
        failed: false,
        run_session_id: None,
        reply_dispatch_id: None,
        failure: None,
        error_summary: None,
    }
}

pub(super) fn submit_gateway_agent_action(
    context: &RemoteCliContext,
    endpoint: &str,
    numeric_session_id: u64,
    event_report: &RemoteFeishuGatewayGovernanceEventReport,
) -> Result<GatewayAgentExecutionProjection, String> {
    let Some(dispatch_report) = event_report.decision.dispatch_report.as_ref() else {
        return Ok(no_agent_execution());
    };
    let RemoteProtocolResponsePayload::MessagePlan(plan) =
        &dispatch_report.protocol_response.payload
    else {
        return Ok(no_agent_execution());
    };
    if !plan.receipt.local_execution_required {
        return Ok(no_agent_execution());
    }
    let inline_control = plan.action == RemoteControlDispatchAction::InlineControl;
    if !inline_control {
        let processing = start_processing(context, event_report);
        let mut projection = queued_agent_execution();
        projection.processing_started = processing.started;
        projection.processing_failed = processing.failed;
        return Ok(projection);
    }
    // 控制事务必须绕过 Agent 与 Typing 生命周期，避免控制回复被旧的网络 reaction 卡住。
    let submission = remote_feishu_control::submit_control_lifecycle(
        context,
        endpoint,
        numeric_session_id,
        event_report,
        plan,
    );
    if let Err(error) = submission {
        finish_failed_processing(context, numeric_session_id, event_report);
        return Err(error);
    }
    Ok(queued_agent_execution())
}

#[derive(Default)]
struct ProcessingProjection {
    started: bool,
    failed: bool,
}

/// 确认 Typing 生命周期是否已启动；未启动则交给异步队列，绝不在本线程发起网络 I/O。
///
/// 入站 callback 已经通过同一异步队列发起过 reaction，worker 只需一次本地状态确认。
/// 历史上这里同步等待 Feishu reaction API（全局 30s 超时），会让任意一个 chat 的
/// Typing 启动阻塞其他所有 chat 的事件治理与 Interaction 提交。
fn start_processing(
    context: &RemoteCliContext,
    event_report: &RemoteFeishuGatewayGovernanceEventReport,
) -> ProcessingProjection {
    if !context.feishu_websocket.enabled || !context.feishu_websocket.enable_notification_sending {
        return ProcessingProjection::default();
    }
    let Some(message_id) = source_message_id(event_report) else {
        return ProcessingProjection::default();
    };
    if processing_started(context, message_id) {
        return ProcessingProjection {
            started: true,
            failed: false,
        };
    }
    enqueue_processing_start(context, message_id);
    // 尚未创建成功不算失败：真实结果由异步 worker 落盘，这里不谎报已启动。
    ProcessingProjection::default()
}

/// 把 Typing 启动请求投入有界异步队列（按 message_id LRU 去重）。
pub(super) fn enqueue_processing_start(context: &RemoteCliContext, message_id: &str) {
    remote_feishu_processing_queue::start_processing_async(
        context,
        message_id,
        |context, message_id| {
            let _ = start_processing_message(context, message_id);
        },
    );
}

fn processing_started(context: &RemoteCliContext, message_id: &str) -> bool {
    let store_path = RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(&context.root);
    RemoteFeishuProcessingStateStoreRuntime::active_reaction(&store_path, message_id)
        .ok()
        .flatten()
        .is_some()
}

fn start_processing_message(context: &RemoteCliContext, message_id: &str) -> ProcessingProjection {
    if !message_id.starts_with("om_") {
        return ProcessingProjection::default();
    }
    let Some(app_secret) = context.feishu_app_secret() else {
        return ProcessingProjection {
            failed: true,
            ..ProcessingProjection::default()
        };
    };
    let store_path = RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(&context.root);
    if RemoteFeishuProcessingStateStoreRuntime::active_reaction(&store_path, message_id)
        .ok()
        .flatten()
        .is_some()
    {
        return ProcessingProjection {
            started: true,
            failed: false,
        };
    }
    let result = RemoteFeishuProcessingRuntime::execute_start_with_secret(
        &context.feishu_websocket.app_id,
        &app_secret,
        feishu_api_base_url(context),
        message_id,
        now_ms(),
    )
    .and_then(|state| RemoteFeishuProcessingStateStoreRuntime::append_record(&store_path, &state));
    match result {
        Ok(()) => ProcessingProjection {
            started: true,
            failed: false,
        },
        Err(error) => {
            axon_logger::warning(&format!("Feishu Typing 启动失败: {error:?}"));
            ProcessingProjection {
                started: false,
                failed: true,
            }
        }
    }
}

/// 控制事务失败时收尾 Typing：只写持久清理任务，不在入站主链上等待飞书 API。
///
/// 与成功路径共用同一条 durable 清理链路（Daemon feedback worker 执行），因此失败
/// 收尾与正常收尾的语义、重试和崩溃补偿完全一致。
fn finish_failed_processing(
    context: &RemoteCliContext,
    numeric_session_id: u64,
    event_report: &RemoteFeishuGatewayGovernanceEventReport,
) {
    let dispatch_id = format!("feishu-typing-failure:{}", event_report.meta.request_id);
    if let Err(error) = remote_feishu_control::enqueue_processing_cleanup(
        context,
        numeric_session_id,
        event_report,
        &dispatch_id,
        RemoteFeishuProcessingOutcome::Failure,
    ) {
        axon_logger::warning(&format!("Feishu Typing 失败收尾任务写入失败: {error}"));
    }
}

fn source_message_id(report: &RemoteFeishuGatewayGovernanceEventReport) -> Option<&str> {
    report
        .meta
        .message_id
        .as_deref()
        .filter(|message_id| message_id.starts_with("om_"))
}

fn feishu_api_base_url(context: &RemoteCliContext) -> Option<&str> {
    (!context.feishu_websocket.api_base_url.trim().is_empty())
        .then_some(context.feishu_websocket.api_base_url.as_str())
}
