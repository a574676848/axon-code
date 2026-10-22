use std::{
    path::PathBuf,
    sync::{
        atomic::Ordering,
        mpsc::{SyncSender, TrySendError},
    },
};

use axon_domain::{AxonError, AxonResult};
use axon_remote_feishu::{
    RemoteFeishuGatewaySdkPayloadTextContext, RemoteFeishuOapiWsIncomingPayload,
};

use super::super::remote_feishu_gateway_worker::gateway_drain_requested;
use super::{
    RemoteCliContext,
    remote_feishu_sdk_loop_counters::RealSdkLoopCounters,
    remote_feishu_sdk_loop_payload::{
        RealSdkPayloadAppendInput, RealSdkPayloadAppendReport, append_real_sdk_payload_with_steer,
    },
};

pub(super) struct RealSdkPayloadCallbackInput<'a> {
    pub context: &'a RemoteCliContext,
    pub session_id: u64,
    pub payload: &'a RemoteFeishuOapiWsIncomingPayload,
    pub event_source: PathBuf,
    pub text_context: Option<RemoteFeishuGatewaySdkPayloadTextContext>,
    pub tenant_access_token: String,
    pub api_base_url: Option<String>,
    pub counters: &'a RealSdkLoopCounters,
}

pub(super) struct RealSdkPayloadCallbackOwnedInput {
    pub context: RemoteCliContext,
    pub session_id: u64,
    pub payload: RemoteFeishuOapiWsIncomingPayload,
    pub event_source: PathBuf,
    pub text_context: Option<RemoteFeishuGatewaySdkPayloadTextContext>,
    pub tenant_access_token: String,
    pub api_base_url: Option<String>,
    pub counters: RealSdkLoopCounters,
    pub wake: SyncSender<()>,
}

pub(super) async fn handle_real_sdk_payload_callback_blocking(
    input: RealSdkPayloadCallbackOwnedInput,
) -> AxonResult<RealSdkPayloadAppendReport> {
    let wake = input.wake.clone();
    let report = tokio::task::spawn_blocking(move || {
        handle_real_sdk_payload_append(&RealSdkPayloadCallbackInput {
            context: &input.context,
            session_id: input.session_id,
            payload: &input.payload,
            event_source: input.event_source,
            text_context: input.text_context,
            tenant_access_token: input.tenant_access_token,
            api_base_url: input.api_base_url,
            counters: &input.counters,
        })
    })
    .await
    .map_err(|_| AxonError::runtime_failure("Feishu SDK loop blocking 任务失败"))??;
    if report.appended_events > 0 {
        notify_worker_after_durable_append(&wake, report.appended_events);
    }
    Ok(report)
}

/// durable append 是 Feishu callback 的确认边界；worker 唤醒失败由轮询和恢复扫描补偿。
fn notify_worker_after_durable_append(wake: &SyncSender<()>, appended_events: usize) {
    if appended_events == 0 {
        return;
    }
    match wake.try_send(()) {
        Ok(()) | Err(TrySendError::Full(())) => {}
        Err(TrySendError::Disconnected(())) => axon_logger::warning(&format!(
            "Feishu 入站已持久化但后台 worker 不可唤醒，将由恢复扫描处理: appended_events={appended_events}"
        )),
    }
}

fn handle_real_sdk_payload_append(
    input: &RealSdkPayloadCallbackInput<'_>,
) -> AxonResult<RealSdkPayloadAppendReport> {
    if gateway_drain_requested(&input.context.root) {
        return Err(AxonError::runtime_failure(
            "Feishu gateway 正在排空，拒绝新的入站事件",
        ));
    }
    let report = append_real_sdk_payload_with_steer(
        RealSdkPayloadAppendInput {
            app_id: input.context.feishu_websocket.app_id.clone(),
            session_id: input.session_id,
            event_source_file: input.event_source.clone(),
            text_context: input.text_context.clone(),
            tenant_access_token: input.tenant_access_token.clone(),
            api_base_url: input.api_base_url.clone(),
        },
        input.payload,
        |event| {
            crate::remote_commands::remote_feishu_gateway::remote_feishu_gateway_execution::prepare_sdk_event(
                input.context,
                event,
            )
            .map_err(AxonError::runtime_failure)
        },
    )?;
    input
        .counters
        .input_payloads
        .fetch_add(report.input_payloads, Ordering::Relaxed);
    input
        .counters
        .mapped_events
        .fetch_add(report.mapped_events, Ordering::Relaxed);
    input
        .counters
        .appended_events
        .fetch_add(report.appended_events, Ordering::Relaxed);
    input
        .counters
        .received_acked_events
        .fetch_add(report.received_acked_events, Ordering::Relaxed);
    input
        .counters
        .received_ack_failed_events
        .fetch_add(report.received_ack_failed_events, Ordering::Relaxed);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_append_ack_survives_closed_worker_wake_channel() {
        let (wake, receiver) = std::sync::mpsc::sync_channel(1);
        drop(receiver);

        notify_worker_after_durable_append(&wake, 1);
    }

    #[test]
    fn durable_append_wake_signal_is_coalesced_when_worker_is_busy() {
        let (wake, receiver) = std::sync::mpsc::sync_channel(1);
        notify_worker_after_durable_append(&wake, 1);
        notify_worker_after_durable_append(&wake, 1);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }
}
