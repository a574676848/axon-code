use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering, mpsc},
    thread,
    time::Duration,
};

use axon_remote_feishu::{
    REMOTE_FEISHU_API_BASE_URL, REMOTE_FEISHU_OAPI_DEFAULT_DOMAIN,
    REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH, RemoteFeishuDeliveryRuntime,
    RemoteFeishuGatewaySdkPayloadTextContext, RemoteFeishuHttpRequestPlan,
    RemoteFeishuOapiExecutorRuntime, RemoteFeishuOapiWsClientConfigState,
    RemoteFeishuOapiWsLoopControl, RemoteFeishuOapiWsLoopReport, RemoteFeishuOapiWsSdkConfig,
    RemoteFeishuOapiWsSdkRuntime,
};
use serde_json::Value;

use crate::display_units::format_duration_millis_compact;

use super::super::{
    remote_feishu_app_lock::FeishuAppLockGuard,
    remote_feishu_gateway_closure::FeishuGatewayClosureReport,
    remote_feishu_gateway_status::{GatewayRuntimeState, GatewayRuntimeStatus},
    remote_feishu_gateway_worker::{
        FeishuGatewayBackgroundWorker, GatewayClosureTiming, gateway_drain_requested,
    },
};
use super::{
    RemoteCliContext, RemoteFeishuSdkLoopCommand, RemoteFeishuSdkLoopDaemonReport,
    feishu_api_base_url_owned, now_ms,
    remote_feishu_sdk_loop_callback::{
        RealSdkPayloadCallbackOwnedInput, handle_real_sdk_payload_callback_blocking,
    },
    remote_feishu_sdk_loop_counters::{RealSdkLoopCounterSnapshot, RealSdkLoopCounters},
    resolve_app_secret, sdk_payload_text_context, websocket_url,
};

const SDK_LOOP_TOKEN_DISPATCH_ID: &str = "feishu-sdk-loop-token";
const CONTENT_TYPE_HEADER: &str = "Content-Type";
const APPLICATION_JSON: &str = "application/json";
const FEISHU_WS_MAX_CONNECTION_LIFETIME_MS: u64 = 30 * 60 * 1_000;

struct RealSdkLoopExecutionContext {
    sdk_config: RemoteFeishuOapiWsSdkConfig,
    tenant_access_token: String,
    api_base_url: Option<String>,
    event_source: PathBuf,
    text_context: Option<RemoteFeishuGatewaySdkPayloadTextContext>,
}

struct RealSdkLoopReportInput<'a> {
    context: &'a RemoteCliContext,
    command: &'a RemoteFeishuSdkLoopCommand,
    event_source_file: &'a Path,
    interval_ms: u64,
    max_events: Option<usize>,
    loop_report: RemoteFeishuOapiWsLoopReport,
    counters: RealSdkLoopCounterSnapshot,
    closure: &'a FeishuGatewayClosureReport,
    reconnect_attempts: usize,
    loop_errors: usize,
}

struct RealSdkLoopReconnectInput<'a> {
    context: &'a RemoteCliContext,
    command: &'a RemoteFeishuSdkLoopCommand,
    event_source_file: &'a Path,
    interval_ms: u64,
    max_events: Option<usize>,
    execution: &'a RealSdkLoopExecutionContext,
    counters: &'a RealSdkLoopCounters,
    worker: &'a FeishuGatewayBackgroundWorker,
    runtime: &'a tokio::runtime::Runtime,
}

pub(super) fn execute_real_sdk_loop(
    context: &RemoteCliContext,
    command: &RemoteFeishuSdkLoopCommand,
    event_source_file: &Path,
    interval_ms: u64,
    max_events: Option<usize>,
    cancellation: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<RemoteFeishuSdkLoopDaemonReport, String> {
    let daemon = should_reconnect(command, max_events);
    if daemon {
        GatewayRuntimeStatus::transition(
            &context.root,
            std::process::id(),
            GatewayRuntimeState::Connecting,
            Some(0),
            None,
        )?;
    }
    let result = (|| {
        let execution = real_sdk_loop_execution_context(context, command, event_source_file)?;
        let _app_lock =
            FeishuAppLockGuard::acquire(&context.root, &context.feishu_websocket.app_id, now_ms())?;
        let counters = RealSdkLoopCounters::default();
        let worker = FeishuGatewayBackgroundWorker::start_with_cancellation(
            context.clone(),
            command.session_id,
            event_source_file,
            execution.tenant_access_token.clone(),
            execution.api_base_url.clone(),
            !daemon,
            cancellation,
            GatewayClosureTiming::PRODUCTION,
        );
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|error| format!("Feishu SDK loop tokio runtime 创建失败: {error}"))?;
        run_real_sdk_loop_with_reconnect(RealSdkLoopReconnectInput {
            context,
            command,
            event_source_file,
            interval_ms,
            max_events,
            execution: &execution,
            counters: &counters,
            worker: &worker,
            runtime: &runtime,
        })
    })();
    if daemon {
        let (state, error) = match &result {
            Ok(_) if gateway_drain_requested(&context.root) => (GatewayRuntimeState::Drained, None),
            Ok(_) => (GatewayRuntimeState::Stopped, None),
            Err(error) => (GatewayRuntimeState::Failed, Some(error.as_str())),
        };
        GatewayRuntimeStatus::transition(&context.root, std::process::id(), state, None, error)?;
    }
    result
}

fn run_real_sdk_loop_with_reconnect(
    input: RealSdkLoopReconnectInput<'_>,
) -> Result<RemoteFeishuSdkLoopDaemonReport, String> {
    let RealSdkLoopReconnectInput {
        context,
        command,
        event_source_file,
        interval_ms,
        max_events,
        execution,
        counters,
        worker,
        runtime,
    } = input;
    let mut loop_report = RemoteFeishuOapiWsLoopReport::default();
    let mut reconnect_attempts = 0;
    let mut loop_errors = 0;
    let mut reconnect_cycle_attempts = 0;
    let client_config_state = RemoteFeishuOapiWsClientConfigState::default();
    loop {
        GatewayRuntimeStatus::transition(
            &context.root,
            std::process::id(),
            GatewayRuntimeState::Connecting,
            Some(reconnect_attempts),
            None,
        )?;
        axon_logger::debug(&format!(
            "Feishu WS 生命周期进入 connecting: reconnect_attempts={reconnect_attempts}"
        ));
        match run_real_sdk_connection(&RealSdkConnectionInput {
            context,
            command,
            max_events,
            execution,
            counters,
            worker,
            runtime,
            client_config_state: &client_config_state,
        }) {
            Ok(report) => {
                accumulate_loop_report(&mut loop_report, &report);
                if should_reconnect(command, max_events)
                    && !worker.cancellation().load(Ordering::SeqCst)
                {
                    let policy = client_config_state.snapshot().reconnect_policy();
                    let retry_number = 1;
                    if !policy.permits_retry(retry_number) {
                        break;
                    }
                    reconnect_cycle_attempts = retry_number;
                    reconnect_attempts += 1;
                    GatewayRuntimeStatus::transition(
                        &context.root,
                        std::process::id(),
                        GatewayRuntimeState::Reconnecting,
                        Some(reconnect_attempts),
                        Some("Feishu WebSocket 连接已关闭"),
                    )?;
                    let delay_ms = reconnect_delay_ms(policy, retry_number);
                    let delay = format_duration_millis_compact(delay_ms);
                    axon_logger::warning(&format!(
                        "Feishu WS 连接正常结束，准备按服务端策略重连: attempt={reconnect_attempts}, delay={delay}"
                    ));
                    thread::sleep(Duration::from_millis(delay_ms));
                    continue;
                }
                break;
            }
            Err(error)
                if should_reconnect(command, max_events)
                    && !worker.cancellation().load(Ordering::SeqCst) =>
            {
                loop_errors += 1;
                let retry_number = reconnect_cycle_attempts.saturating_add(1);
                let policy = client_config_state.snapshot().reconnect_policy();
                if !policy.permits_retry(retry_number) {
                    return Err(format!(
                        "Feishu WS 已达到服务端重连上限: retries={reconnect_cycle_attempts}, error={error}"
                    ));
                }
                reconnect_cycle_attempts = retry_number;
                reconnect_attempts += 1;
                GatewayRuntimeStatus::transition(
                    &context.root,
                    std::process::id(),
                    GatewayRuntimeState::Reconnecting,
                    Some(reconnect_attempts),
                    Some(&error),
                )?;
                let delay_ms = reconnect_delay_ms(policy, retry_number);
                let delay = format_duration_millis_compact(delay_ms);
                axon_logger::warning(&format!(
                    "Feishu WS 连接失败，准备按服务端策略重连: retry={retry_number}, delay={delay}, error={error}"
                ));
                thread::sleep(Duration::from_millis(delay_ms));
            }
            Err(error) => return Err(error),
        }
    }
    let closure = worker.snapshot();
    Ok(real_sdk_loop_report(&RealSdkLoopReportInput {
        context,
        command,
        event_source_file,
        interval_ms,
        max_events,
        loop_report,
        counters: counters.snapshot(),
        closure: &closure,
        reconnect_attempts,
        loop_errors,
    }))
}

fn reconnect_delay_ms(
    policy: axon_remote_feishu::RemoteFeishuOapiWsReconnectPolicy,
    retry_number: usize,
) -> u64 {
    let entropy_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let entropy_millis = u64::try_from(entropy_millis).unwrap_or(u64::MAX);
    let delay = policy.delay_before_retry(retry_number, entropy_millis);
    u64::try_from(delay.as_millis()).unwrap_or(u64::MAX)
}

struct RealSdkConnectionInput<'a> {
    context: &'a RemoteCliContext,
    command: &'a RemoteFeishuSdkLoopCommand,
    max_events: Option<usize>,
    execution: &'a RealSdkLoopExecutionContext,
    counters: &'a RealSdkLoopCounters,
    worker: &'a FeishuGatewayBackgroundWorker,
    runtime: &'a tokio::runtime::Runtime,
    client_config_state: &'a RemoteFeishuOapiWsClientConfigState,
}

fn run_real_sdk_connection(
    input: &RealSdkConnectionInput<'_>,
) -> Result<RemoteFeishuOapiWsLoopReport, String> {
    let context = input.context;
    let command = input.command;
    let max_events = input.max_events;
    let execution = input.execution;
    let counters = input.counters;
    let worker = input.worker;
    let runtime = input.runtime;
    let client_config_state = input.client_config_state;
    let callback_context = context.clone();
    let session_id = command.session_id;
    let counters = counters.clone();
    let wake = worker.wake_sender();
    let cancellation = worker.cancellation();
    let (connected_sender, connected_receiver) = mpsc::channel::<()>();
    let status_context = context.clone();
    let status_event_source = execution.event_source.clone();
    let status_thread = should_reconnect(command, max_events).then(|| {
        thread::spawn(move || {
            if connected_receiver.recv().is_ok() {
                axon_logger::debug(&format!(
                    "Feishu WS 生命周期进入 connected: pid={}",
                    std::process::id()
                ));
                let _ = GatewayRuntimeStatus::transition(
                    &status_context.root,
                    std::process::id(),
                    GatewayRuntimeState::Connected,
                    None,
                    None,
                );
                if let Err(error) =
                    super::super::remote_gateway_restart_monitor::record_gateway_start(
                        &status_context,
                        &status_event_source,
                        std::process::id(),
                    )
                {
                    axon_logger::warning(&format!(
                        "Feishu gateway 重启监控记录失败（不影响服务运行）: {error}"
                    ));
                }
            }
        })
    });
    let connection_lifetime = Duration::from_millis(FEISHU_WS_MAX_CONNECTION_LIFETIME_MS);
    let requested_wait = command.wait_ms.map(Duration::from_millis);
    let max_duration = if should_reconnect(command, max_events) {
        requested_wait.map_or(Some(connection_lifetime), |wait| {
            Some(wait.min(connection_lifetime))
        })
    } else {
        requested_wait
    };
    let result = runtime
        .block_on(RemoteFeishuOapiWsSdkRuntime::run_until_closed_with_control(
            execution.sdk_config.clone(),
            RemoteFeishuOapiWsLoopControl::new(max_events)
                .with_max_duration(max_duration)
                .with_cancellation(cancellation)
                .with_connected_signal(connected_sender)
                .with_client_config_state((*client_config_state).clone()),
            move |payload| {
                let context = callback_context.clone();
                let event_source = execution.event_source.clone();
                let text_context = execution.text_context.clone();
                let tenant_access_token = execution.tenant_access_token.clone();
                let api_base_url = execution.api_base_url.clone();
                let counters = counters.clone();
                let wake = wake.clone();
                async move {
                    let callback_input = RealSdkPayloadCallbackOwnedInput {
                        context,
                        session_id,
                        payload,
                        event_source,
                        text_context,
                        tenant_access_token,
                        api_base_url,
                        counters,
                        wake,
                    };
                    let report = handle_real_sdk_payload_callback_blocking(callback_input).await?;
                    Ok(stop_response_for_appended_event(
                        command.stop_after_first_event,
                        report.appended_events,
                    ))
                }
            },
        ))
        .map_err(|error| format!("{error:?}"));
    if let Some(status_thread) = status_thread {
        let _ = status_thread.join();
    }
    if should_reconnect(command, max_events) && result.as_ref().is_ok_and(|report| report.connected)
    {
        axon_logger::debug(&format!(
            "Feishu WS 连接达到主动轮换周期，将重新 bootstrap: lifetime_ms={FEISHU_WS_MAX_CONNECTION_LIFETIME_MS}"
        ));
    }
    record_connection_result(context, &result);
    result
}

fn record_connection_result(
    context: &RemoteCliContext,
    result: &Result<RemoteFeishuOapiWsLoopReport, String>,
) {
    let Err(error) = result else {
        return;
    };
    axon_logger::error(&format!("Feishu WS 连接生命周期结束: error={error}"));
    let _ = GatewayRuntimeStatus::transition(
        &context.root,
        std::process::id(),
        GatewayRuntimeState::Reconnecting,
        None,
        Some(error),
    );
}

fn real_sdk_loop_execution_context(
    context: &RemoteCliContext,
    command: &RemoteFeishuSdkLoopCommand,
    event_source_file: &Path,
) -> Result<RealSdkLoopExecutionContext, String> {
    let app_secret = resolve_app_secret(context, command)?;
    let mut sdk_config = RemoteFeishuOapiWsSdkConfig::new(
        context.feishu_websocket.app_id.clone(),
        app_secret.clone(),
    );
    if context.feishu_websocket.api_base_url.trim().is_empty() {
        REMOTE_FEISHU_OAPI_DEFAULT_DOMAIN.clone_into(&mut sdk_config.domain);
    } else {
        sdk_config
            .domain
            .clone_from(&context.feishu_websocket.api_base_url);
    }
    Ok(RealSdkLoopExecutionContext {
        sdk_config,
        tenant_access_token: fetch_sdk_loop_tenant_access_token(context, &app_secret)?,
        api_base_url: feishu_api_base_url_owned(context),
        event_source: event_source_file.to_path_buf(),
        text_context: sdk_payload_text_context(context),
    })
}

fn real_sdk_loop_report(input: &RealSdkLoopReportInput<'_>) -> RemoteFeishuSdkLoopDaemonReport {
    RemoteFeishuSdkLoopDaemonReport {
        status: "remote_feishu_sdk_loop_daemon_ready",
        session_id: input.command.session_id,
        transport: "feishu",
        daemon: true,
        sdk_connection_mode: "officialOapiWsSdk",
        sdk_connected: input.loop_report.connected,
        websocket_url: websocket_url(input.context),
        app_id_configured: !input.context.feishu_websocket.app_id.trim().is_empty(),
        app_secret_env_configured: !input
            .context
            .feishu_websocket
            .app_secret_env
            .trim()
            .is_empty(),
        payload_source: None,
        event_source_file: input.event_source_file.to_string_lossy().to_string(),
        interval_ms: input.interval_ms,
        max_ticks: input.command.max_ticks,
        max_events: input.max_events,
        wait_ms: input.command.wait_ms,
        stop_after_first_event: input.command.stop_after_first_event,
        completed_ticks: input.loop_report.handled_events,
        empty_ticks: 0,
        acked_offset: 0,
        cursor_file: String::new(),
        sdk_received_binary_frames: input.loop_report.received_binary_frames,
        sdk_received_data_frames: input.loop_report.received_data_frames,
        sdk_skipped_data_frames: input.loop_report.skipped_data_frames,
        input_payloads: input.counters.input_payloads,
        mapped_events: input.counters.mapped_events,
        appended_events: input.counters.appended_events,
        sdk_sent_pings: input.loop_report.sent_pings,
        sdk_received_pongs: input.loop_report.received_pongs,
        received_acked_events: input.counters.received_acked_events,
        received_ack_failed_events: input.counters.received_ack_failed_events,
        closure_gateway_input_events: input.closure.gateway_input_events,
        closure_agent_executed_events: input.closure.gateway_agent_executed_events,
        closure_reply_enqueued_events: input.closure.gateway_reply_enqueued_events,
        closure_session_reset_events: input.closure.gateway_session_reset_events,
        closure_worker_delivered_count: input.closure.worker_delivered_count,
        closure_feedback_completed_count: input.closure.feedback_completed_count,
        closure_failed_events: input.counters.closure_failed_events
            + input.closure.gateway_agent_failed_events
            + input.closure.feedback_retryable_count
            + input.closure.feedback_permanent_count,
        sdk_reconnect_attempts: input.reconnect_attempts,
        sdk_loop_errors: input.loop_errors,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        raw_payload_persisted: false,
    }
}

fn fetch_sdk_loop_tenant_access_token(
    context: &RemoteCliContext,
    app_secret: &str,
) -> Result<String, String> {
    let mut http = RemoteFeishuOapiExecutorRuntime::execute_json;
    let template = token_request_template(context)?;
    RemoteFeishuDeliveryRuntime::fetch_tenant_access_token_with_http_and_secret(
        &context.feishu_websocket.app_id,
        app_secret,
        &template,
        &mut http,
    )
    .map_err(|error| format!("{error:?}"))
}

fn token_request_template(
    context: &RemoteCliContext,
) -> Result<RemoteFeishuHttpRequestPlan, String> {
    let base =
        feishu_api_base_url_owned(context).unwrap_or_else(|| REMOTE_FEISHU_API_BASE_URL.to_owned());
    let base = base.trim().trim_end_matches('/');
    if !base.starts_with("https://") && !base.starts_with("http://") {
        return Err("Feishu SDK loop api_base_url 必须为 http/https".to_owned());
    }
    Ok(RemoteFeishuHttpRequestPlan {
        dispatch_id: SDK_LOOP_TOKEN_DISPATCH_ID.to_owned(),
        url: format!("{base}{REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH}"),
        method: "POST".to_owned(),
        headers: vec![(CONTENT_TYPE_HEADER.to_owned(), APPLICATION_JSON.to_owned())],
        body_json: String::new(),
    })
}

fn should_reconnect(command: &RemoteFeishuSdkLoopCommand, max_events: Option<usize>) -> bool {
    command.wait_ms.is_none() && max_events.is_none() && !command.stop_after_first_event
}

fn accumulate_loop_report(
    total: &mut RemoteFeishuOapiWsLoopReport,
    tick: &RemoteFeishuOapiWsLoopReport,
) {
    total.connected |= tick.connected;
    total.received_binary_frames += tick.received_binary_frames;
    total.received_data_frames += tick.received_data_frames;
    total.handled_events += tick.handled_events;
    total.skipped_data_frames += tick.skipped_data_frames;
    total.sent_pings += tick.sent_pings;
    total.received_pongs += tick.received_pongs;
    total.requested_stops += tick.requested_stops;
}

fn stop_response_for_appended_event(
    stop_after_first_event: bool,
    appended_events: usize,
) -> Option<Value> {
    if stop_after_first_event && appended_events > 0 {
        return Some(serde_json::json!({
            "axonControl": {
                "stop": true
            }
        }));
    }
    None
}
