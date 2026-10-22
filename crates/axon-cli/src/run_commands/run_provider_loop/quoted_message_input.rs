//! Provider 输入构建与引用消息瞬时投影。

use axon_agent_runtime::{
    AgentRunProviderTransportInput, agent_run_execution_error_from_axon_error,
    agent_run_provider_streaming_transport_input, insert_before_latest_user,
    project_quoted_message, provider_streaming_prompt_cache_plan,
};
use axon_config::{ModelConfig, ProviderConfig};

use super::super::RunRequest;
use super::super::run_execution::{RunExecutionError, RunToolReceipt};

pub(super) fn provider_streaming_transport_input(
    request: &RunRequest,
    channel_model: &ModelConfig,
    channel_provider: &ProviderConfig,
) -> Result<axon_agent_runtime::ProviderStreamingTransportInput, Box<RunExecutionError>> {
    let mut input = agent_run_provider_streaming_transport_input(AgentRunProviderTransportInput {
        client_channel: request.client_channel,
        model: channel_model,
        provider: channel_provider,
        messages: request.model_payload.messages.clone(),
        model_tools: request.model_payload.model_tools.clone(),
        context_section_count: request.model_payload.context_section_count,
        reasoning_effort: request.reasoning_effort.clone(),
        structured_output: request.structured_output.clone(),
    })?;
    let Some(quoted) = request.quoted_message.as_ref() else {
        return Ok(input);
    };

    let mut messages = input.messages.clone().into_messages();
    let note = project_quoted_message(
        quoted,
        &messages,
        input.protocol,
        &input.model_tools,
        input.supports_vision,
        input.context_window,
    )
    .into_runtime_note();
    insert_before_latest_user(&mut messages, note);
    input.messages = messages.into();
    input.prompt_cache_plan = provider_streaming_prompt_cache_plan(&input).map_err(|error| {
        Box::new(agent_run_execution_error_from_axon_error::<RunToolReceipt>(
            &error,
            &[],
        ))
    })?;
    Ok(input)
}
