//! Provider turn 的实时观察、文本批处理与权威收口投影。
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc::sync_channel};

use axon_agent_runtime::RunCancelToken;
use axon_protocol::wire::{WireEventEnvelope, WireEventKind};
use axon_session_runtime::{
    ProviderTurnRequest, SubagentSessionSinkFactory, TurnEventSink, TurnPerformanceRecorder,
    TurnPerformanceStage,
};

use super::run_progress::RunProgressWireEvent;
use super::run_steer::RunSteerInbox;
use super::run_stream_event_batch::{channel_capacity, run_event_batch_worker};
use super::{CliRunResult, RunAgentIdentity, RunCommand, RunProgressOptions, RunStartupSnapshot};

const LIVE_PROJECTION_FAILURE_EXIT_CODE: u8 = 1;
const LIVE_PROJECTION_FAILURE_PREFIX: &str = "Daemon 实时事件持久化失败";
const LIVE_PROJECTION_WORKER_PANIC: &str = "Daemon 实时投影批处理 worker 异常退出";

/// 执行 Provider turn，并把实时事件交给有界批处理 worker。
pub(super) fn run_with_progress(
    config_path: &Path,
    request: &ProviderTurnRequest,
    command: &RunCommand,
    startup_snapshot: Option<Arc<RunStartupSnapshot>>,
    sink: Option<&dyn TurnEventSink>,
    steer_inbox: RunSteerInbox,
    agent_identity: RunAgentIdentity,
    cancel_token: RunCancelToken,
    user_question_port: Option<std::sync::Arc<dyn axon_agent_runtime::UserQuestionPort>>,
) -> CliRunResult {
    let performance = sink.and_then(TurnEventSink::performance_recorder);
    let observed_performance = performance.clone();
    if let Some(sink) = sink {
        let (sender, receiver) = sync_channel(channel_capacity());
        let (run_result, projection_result) = std::thread::scope(|scope| {
            let worker = scope.spawn(|| run_event_batch_worker(receiver, sink));
            let run_result;
            {
                let mut observe = |event: RunProgressWireEvent| {
                    if let Some(performance) = &observed_performance
                        && first_visible_wire_event(&event.event.kind)
                    {
                        performance.record(TurnPerformanceStage::ProviderFirstDeltaParsed);
                    }
                    if let Err(error) = sender.send(event.event.clone()) {
                        axon_logger::warning(&format!("Daemon 实时投影批处理通道已断开: {error}"));
                    }
                };
                let bus_sender = sender.clone();
                // 子代理进度总线闭包持有的 sender 克隆会随后台作业的终端保留窗（5 分钟）
                // 存活，令事件 worker 的 Disconnected 迟迟不到、runner 被迫等满保留窗。
                // 改为可摘取发送端：父 run 收口时立即摘除，尾随进度事件显式丢弃并留痕。
                let bus_sender_cell: Arc<
                    Mutex<Option<std::sync::mpsc::SyncSender<WireEventEnvelope>>>,
                > = Arc::new(Mutex::new(Some(bus_sender)));
                let bus_cell_for_closure = Arc::clone(&bus_sender_cell);
                let subagent_event_bus: Option<super::run_progress::RunProgressEventBus> =
                    Some(Arc::new(move |event| {
                        let sender = bus_cell_for_closure
                            .lock()
                            .expect("子代理进度总线锁")
                            .clone();
                        match sender {
                            Some(sender) => sender
                                .send(event.event.clone())
                                .map_err(|error| error.to_string()),
                            None => Err("父 run 已收口，尾随子代理进度事件已丢弃".to_owned()),
                        }
                    }));
                // 问答事件与子代理进度共用同一条实时通道：进程级问答宿主按车道身份把
                // 事件投进这里，run 收口时与子代理总线一起摘除发送端。
                let question_registration = question_event_registration(request, &sender);
                run_result = super::run_run_with_progress(
                    command,
                    Some(config_path),
                    Some(Path::new(&request.cwd)),
                    provider_progress_options(
                        request,
                        &mut observe,
                        subagent_event_bus,
                        sink.subagent_session_sinks(),
                        startup_snapshot,
                        steer_inbox,
                        agent_identity,
                        performance,
                        cancel_token.clone(),
                        user_question_port.clone(),
                    ),
                );
                // 父 run 已收口：摘除总线发送端，事件通道随即断开，worker 排空后立即退出。
                bus_sender_cell.lock().expect("子代理进度总线锁").take();
                drop(question_registration);
            }
            drop(sender);
            let projection_result = worker
                .join()
                .unwrap_or_else(|_| Err(LIVE_PROJECTION_WORKER_PANIC.to_owned()));
            axon_logger::debug(&format!(
                "事件批处理 worker join 完成: session={}",
                request.session_id
            ));
            (run_result, projection_result)
        });
        apply_projection_result(run_result, projection_result)
    } else {
        let mut observe = |_event: RunProgressWireEvent| {};
        super::run_run_with_progress(
            command,
            Some(config_path),
            Some(Path::new(&request.cwd)),
            provider_progress_options(
                request,
                &mut observe,
                None,
                sink.and_then(TurnEventSink::subagent_session_sinks),
                startup_snapshot,
                steer_inbox,
                agent_identity,
                performance,
                cancel_token,
                user_question_port.clone(),
            ),
        )
    }
}

fn apply_projection_result(
    mut run_result: CliRunResult,
    projection_result: Result<(), String>,
) -> CliRunResult {
    let Err(error) = projection_result else {
        return run_result;
    };
    let detail = format!("{LIVE_PROJECTION_FAILURE_PREFIX}: {error}");
    axon_logger::warning(&detail);
    if !run_result.stderr.is_empty() {
        run_result.stderr.push('\n');
    }
    run_result.stderr.push_str(&detail);
    if run_result.exit_code == 0 {
        run_result.exit_code = LIVE_PROJECTION_FAILURE_EXIT_CODE;
    }
    run_result
}

fn provider_progress_options<'a>(
    request: &ProviderTurnRequest,
    progress: &'a mut dyn FnMut(RunProgressWireEvent),
    subagent_event_bus: Option<super::run_progress::RunProgressEventBus>,
    subagent_session_sinks: Option<Arc<dyn SubagentSessionSinkFactory>>,
    startup_snapshot: Option<Arc<RunStartupSnapshot>>,
    steer_inbox: RunSteerInbox,
    agent_identity: RunAgentIdentity,
    performance: Option<Arc<dyn TurnPerformanceRecorder>>,
    cancel_token: RunCancelToken,
    user_question_port: Option<std::sync::Arc<dyn axon_agent_runtime::UserQuestionPort>>,
) -> RunProgressOptions<'a> {
    let client_channel = request
        .transport_context
        .as_ref()
        .and_then(|value| value.get("channel").and_then(|value| value.as_str()))
        .map(|value| axon_client_identity::ClientChannel::parse(Some(value)))
        .unwrap_or(axon_client_identity::ClientChannel::Feishu);
    RunProgressOptions {
        client_channel,
        progress: Some(progress),
        subagent_event_bus,
        subagent_session_sinks,
        startup_snapshot,
        steer_inbox: Some(steer_inbox),
        quoted_message: request.quoted_message.clone(),
        transport_context: request.transport_context.clone(),
        external_stop_control: true,
        agent_identity,
        performance,
        // 网关中断与文件式 stop 控制共用同一个取消令牌：
        // 两者都只是把它置位，收口路径完全一致。
        cancel_token: Some(cancel_token),
        user_question_port,
    }
}

pub(super) fn first_visible_wire_event(kind: &WireEventKind) -> bool {
    match kind {
        WireEventKind::AssistantMessage(message) => message.delta && !message.content.is_empty(),
        WireEventKind::ReasoningDelta(reasoning) => !reasoning.delta.is_empty(),
        WireEventKind::ToolCall(call) => {
            !call.tool_name.is_empty()
                || call
                    .arguments_summary
                    .as_deref()
                    .is_some_and(|summary| !summary.is_empty())
        }
        _ => false,
    }
}

/// 把本轮车道登记进问答事件路由，返回 RAII 注销句柄。
///
/// 进程级问答宿主按车道身份把事件投进这条通道；run 收口即摘除，尾随事件显式丢弃。
fn question_event_registration(
    request: &ProviderTurnRequest,
    sender: &std::sync::mpsc::SyncSender<WireEventEnvelope>,
) -> super::run_question_projection::QuestionEventRegistration {
    let router = super::run_question_projection::global_question_event_router();
    let lane_id = super::run_question_projection::run_lane_id(
        request.transport_context.as_ref(),
        &request.session_id,
    );
    let sender_cell = Arc::new(Mutex::new(Some(sender.clone())));
    let bus: super::run_question_projection::QuestionEventBus = Arc::new(move |event| {
        let sender = sender_cell.lock().expect("问答事件通道锁").clone();
        match sender {
            Some(sender) => sender.send(event.event).map_err(|error| error.to_string()),
            None => Err("本轮 run 已收口，尾随问答事件已丢弃".to_owned()),
        }
    });
    router.register(&lane_id, bus)
}
