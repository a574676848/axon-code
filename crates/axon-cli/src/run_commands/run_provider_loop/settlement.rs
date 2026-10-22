use super::*;

pub(super) fn provider_wire_format(
    protocol: ProviderStreamProtocol,
) -> axon_agent_runtime::PromptCacheProviderWireFormat {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => {
            axon_agent_runtime::PromptCacheProviderWireFormat::OpenAiResponses
        }
        ProviderStreamProtocol::OpenAiCompatible => {
            axon_agent_runtime::PromptCacheProviderWireFormat::OpenAiCompatibleChat
        }
        ProviderStreamProtocol::AnthropicMessages => {
            axon_agent_runtime::PromptCacheProviderWireFormat::AnthropicMessages
        }
    }
}

pub(super) fn persist_settled_tool_receipts(
    request: &RunRequest,
    state: &PartialToolCallState<'_>,
    turn_id: u64,
    receipts: &[RunToolReceipt],
) {
    state
        .tool_receipts
        .borrow_mut()
        .extend(receipts.iter().cloned());
    persist_partial_tool_call_record(request, state, turn_id);
}

pub(super) fn partial_tool_call_events(
    turn_id: u64,
    tool_calls: &[axon_agent_runtime::ProviderToolCall],
) -> Vec<AgentTurnEvent> {
    tool_calls
        .iter()
        .enumerate()
        .map(|(dispatch_order, tool_call)| {
            AgentTurnEvent::new(
                axon_domain::TurnId(turn_id),
                AgentTurnEventKind::ToolExecutionStarted,
                format!("工具已持久化，等待执行：{}", tool_call.dispatch_name()),
            )
            .with_part_id(crate::tool_orchestrator::tool_execution_part_id(
                turn_id,
                &tool_call.id,
                dispatch_order,
            ))
            .with_tool_name(tool_call.dispatch_name())
            .with_provider_call_id(tool_call.id.clone())
            .with_status("running")
            .with_phase(TOOL_PHASE)
        })
        .collect()
}

pub(super) fn persist_partial_tool_call_record(
    request: &RunRequest,
    state: &PartialToolCallState<'_>,
    _turn_id: u64,
) {
    let input = super::super::run_session_store::RunSessionPartialInput {
        provider: state.provider_name.to_owned(),
        model: state.model_id.to_owned(),
        tool_calls: state.tool_calls.borrow().clone(),
        tool_receipts: state.tool_receipts.borrow().clone(),
        reasoning_messages: state.reasoning.borrow().clone(),
        turn_events: state.turn_events.borrow().clone(),
    };
    let record = super::super::run_session_store::RunSessionRecord::from_partial(request, &input);
    let title = super::super::run_thread_title::derive_thread_title(&record.prompt);
    if let Err(error) =
        super::super::commit_session_to_rollout(&request.config_path, &record, &title)
    {
        axon_logger::warning(&format!(
            "partial tool-call persistence skipped: session_id={}, error={error}",
            request.session_id
        ));
    }
}

pub(super) fn emit_provider_reasoning_reports(
    progress: Option<&RunProgressSink<'_>>,
    turn_reports: &[axon_agent_runtime::AgentTurnIterationReport],
) {
    let Some(sink) = progress else {
        return;
    };
    for turn_report in turn_reports {
        let Some(content) = turn_report
            .transport_report
            .runner_report
            .reasoning_content
            .as_deref()
            .map(str::trim)
            .filter(|content| !content.is_empty())
        else {
            continue;
        };
        sink.finish_reasoning(
            agent_run_reasoning_part_id(turn_report.turn_id.0),
            agent_run_provider_reasoning_id(turn_report.turn_id.0),
            content,
        );
    }
}

pub(super) fn interrupted_axon_error<T>() -> axon_domain::AxonResult<T> {
    Err(axon_domain::AxonError::runtime_failure(
        "agent_run_interrupted",
    ))
}

pub(super) fn response_from_turn_report(
    report: axon_agent_runtime::AgentTurnRuntimeReport<RunToolReceipt>,
    provider: &ProviderConfig,
    model: &ModelConfig,
) -> RunResponse {
    let turn_events = tool_lifecycle_turn_events(&report);
    agent_run_response_from_turn_report(AgentRunCompletedInput {
        report,
        provider: provider.name.clone(),
        model: model.id.clone(),
        turn_events,
    })
}

pub(super) fn interrupted_error(receipts: &[RunToolReceipt]) -> Box<RunExecutionError> {
    Box::new(agent_run_interrupted_error(receipts))
}
