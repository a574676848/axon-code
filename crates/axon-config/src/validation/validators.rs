use axon_domain::{AxonError, AxonResult};

use crate::{
    AgentConfig, AxonConfig, ChannelBalancerConfig, MemoryConfig, ModelConfig,
    PROVIDER_PROTOCOL_ANTHROPIC, PROVIDER_PROTOCOL_DASHSCOPE, PROVIDER_PROTOCOL_OPENAI,
    PROVIDER_PROTOCOL_OPENAI_CHAT, PROVIDER_PROTOCOL_OPENAI_MULTIMODAL_EMBEDDINGS,
    PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL, PROVIDER_WIRE_TRANSPORT_HTTP,
    PROVIDER_WIRE_TRANSPORT_WEBSOCKET, ProviderConfig, REASONING_EFFORT_VALUES,
    normalize_reasoning_effort,
};

pub(crate) fn validate_config(config: &AxonConfig) -> AxonResult<()> {
    validate_agent(&config.agent)?;
    validate_agent_model_slots(config)?;
    validate_channel_balancer(&config.channel_balancer)?;
    validate_remote_policy(config)?;
    validate_providers(&config.providers)?;
    validate_models(&config.providers, &config.models)?;
    validate_memory_embedding(&config.memory, &config.providers, &config.models)
}

fn validate_agent(agent: &AgentConfig) -> AxonResult<()> {
    validate_reasoning_effort(
        "agent.reasoning_effort",
        Some(agent.reasoning_effort.as_str()),
    )?;
    if agent.max_subagent_threads == 0 {
        return Err(AxonError::config_invalid(
            "agent.max_subagent_threads 必须大于 0",
        ));
    }
    if agent.max_subagent_active_turns == 0 {
        return Err(AxonError::config_invalid(
            "agent.max_subagent_active_turns 必须大于 0",
        ));
    }
    if agent.max_subagent_depth == 0 {
        return Err(AxonError::config_invalid(
            "agent.max_subagent_depth 必须大于 0",
        ));
    }
    if agent.idle_compaction_debounce_ms == 0 {
        return Err(AxonError::config_invalid(
            "agent.idle_compaction_debounce_ms 必须大于 0",
        ));
    }
    Ok(())
}

fn validate_agent_model_slots(config: &AxonConfig) -> AxonResult<()> {
    if config.models.is_empty() {
        return Ok(());
    }
    validate_agent_model_slot(config, "agent.default_model", &config.agent.default_model)?;
    validate_agent_model_slot(config, "agent.fast_model", &config.agent.fast_model)
}

fn validate_agent_model_slot(
    config: &AxonConfig,
    key: &'static str,
    value: &str,
) -> AxonResult<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AxonError::config_invalid(match key {
            "agent.default_model" => "agent.default_model 不能为空",
            "agent.fast_model" => "agent.fast_model 不能为空",
            _ => "agent 模型槽位不能为空",
        }));
    }
    if config
        .models
        .iter()
        .any(|model| model.name == value || model.id == value)
    {
        return Ok(());
    }
    Err(AxonError::config_invalid(match key {
        "agent.default_model" => "agent.default_model 必须引用已配置模型",
        "agent.fast_model" => "agent.fast_model 必须引用已配置模型",
        _ => "agent 模型槽位必须引用已配置模型",
    }))
}

fn validate_channel_balancer(balancer: &ChannelBalancerConfig) -> AxonResult<()> {
    if balancer.failure_threshold < 1 {
        return Err(AxonError::config_invalid(
            "channel_balancer.failure_threshold 必须大于等于 1",
        ));
    }
    if balancer.cooldown_ms == 0 {
        return Err(AxonError::config_invalid(
            "channel_balancer.cooldown_ms 必须大于 0",
        ));
    }
    if balancer.affinity_ttl_ms == 0 {
        return Err(AxonError::config_invalid(
            "channel_balancer.affinity_ttl_ms 必须大于 0",
        ));
    }
    Ok(())
}

fn validate_remote_policy(config: &AxonConfig) -> AxonResult<()> {
    let token_env = config.remote.gateway.api_token_env.trim();
    if token_env.is_empty()
        || !token_env
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(AxonError::config_invalid(
            "remote.gateway.api_token_env 只能包含字母、数字和下划线",
        ));
    }
    for (key, value) in [
        ("remote.gateway.http_bind", &config.remote.gateway.http_bind),
        (
            "remote.gateway.websocket_bind",
            &config.remote.gateway.websocket_bind,
        ),
    ] {
        if value.trim().parse::<std::net::SocketAddr>().is_err() {
            return Err(AxonError::config_invalid(format!(
                "{key} 必须为有效的 IP:端口地址",
            )));
        }
    }
    if !config
        .remote
        .feishu_websocket
        .api_base_url
        .trim()
        .is_empty()
        && !websocket_url_allowed(&config.remote.feishu_websocket.api_base_url)
    {
        return Err(AxonError::config_invalid(
            "remote.feishu_websocket.api_base_url 必须为 http 或 https URL",
        ));
    }
    if config
        .remote
        .feishu_websocket
        .allowed_sender_open_ids
        .iter()
        .any(|open_id| open_id.trim().is_empty())
    {
        return Err(AxonError::config_invalid(
            "remote.feishu_websocket.allowed_sender_open_ids 不能为空字符串",
        ));
    }
    if config
        .remote
        .feishu_websocket
        .blocked_sender_open_ids
        .iter()
        .any(|open_id| open_id.trim().is_empty())
    {
        return Err(AxonError::config_invalid(
            "remote.feishu_websocket.blocked_sender_open_ids 不能为空字符串",
        ));
    }
    for policy in &config.remote.feishu_websocket.chat_policies {
        if policy.chat_id.trim().is_empty() {
            return Err(AxonError::config_invalid(
                "remote.feishu_websocket.chat_policies.chat_id 不能为空",
            ));
        }
        if policy
            .allowed_sender_open_ids
            .iter()
            .any(|open_id| open_id.trim().is_empty())
            || policy
                .blocked_sender_open_ids
                .iter()
                .any(|open_id| open_id.trim().is_empty())
        {
            return Err(AxonError::config_invalid(
                "remote.feishu_websocket.chat_policies open_id 不能为空字符串",
            ));
        }
    }
    Ok(())
}

fn validate_providers(providers: &[ProviderConfig]) -> AxonResult<()> {
    for (index, provider) in providers.iter().enumerate() {
        if provider.name.trim().is_empty() {
            return Err(AxonError::config_invalid("provider.name 不能为空"));
        }
        if !provider_protocol_supported(&provider.protocol) {
            return Err(AxonError::config_invalid(
                "provider.protocol 必须为 openai、openai-chatcompletions、anthropic、dashscope、openai-multimodal-embeddings 或 volcengine-ark-multimodal",
            ));
        }
        if !provider.base_url.starts_with("http://") && !provider.base_url.starts_with("https://") {
            return Err(AxonError::config_invalid(
                "provider.base_url 必须为 http 或 https URL",
            ));
        }
        if !provider_wire_transport_supported(&provider.wire_transport) {
            return Err(AxonError::config_invalid(
                "provider.wire_transport 必须为 http 或 websocket",
            ));
        }
        if let Some(websocket_url) = provider.websocket_url.as_deref()
            && !websocket_url_allowed(websocket_url)
        {
            return Err(AxonError::config_invalid(
                "provider.websocket_url 必须为 ws、wss、http 或 https URL",
            ));
        }
        if providers[..index]
            .iter()
            .any(|existing| existing.name == provider.name)
        {
            return Err(AxonError::config_invalid("provider.name 不能重复"));
        }
    }
    Ok(())
}

fn validate_models(providers: &[ProviderConfig], models: &[ModelConfig]) -> AxonResult<()> {
    for (index, model) in models.iter().enumerate() {
        // model.name 为空时自动 fallback 到 model.id，无需强制非空。
        if model.id.trim().is_empty() {
            return Err(AxonError::config_invalid("model.id 不能为空"));
        }
        if model.provider.trim().is_empty() {
            return Err(AxonError::config_invalid("model.provider 不能为空"));
        }
        let provider = providers
            .iter()
            .find(|provider| provider.name == model.provider)
            .ok_or(AxonError::config_invalid(
                "model.provider 必须引用已配置 provider",
            ))?;
        validate_model_provider_protocol(model, provider)?;
        if model.context_window == 0 && !model.supports_embedding {
            return Err(AxonError::config_invalid("model.context_window 必须大于 0"));
        }
        if model.max_output_tokens.is_some_and(|value| value == 0) {
            return Err(AxonError::config_invalid(
                "model.max_output_tokens 必须大于 0",
            ));
        }
        if model.supports_embedding && model.embedding_dimension == 0 {
            return Err(AxonError::config_invalid(
                "model.embedding_dimension 必须大于 0",
            ));
        }
        if !model.supports_embedding && model.embedding_dimension != 0 {
            return Err(AxonError::config_invalid(
                "model.embedding_dimension 只能用于 embedding 模型",
            ));
        }
        if models[..index].iter().any(|existing| {
            existing.provider == model.provider
                && existing.id == model.id
                && model_routing_key(existing) == model_routing_key(model)
        }) {
            return Err(AxonError::config_invalid(
                "同一 provider 下 model.name 与 model.id 组合不能重复",
            ));
        }
    }
    Ok(())
}

fn model_routing_key(model: &ModelConfig) -> &str {
    if model.name.trim().is_empty() {
        &model.id
    } else {
        &model.name
    }
}

fn validate_reasoning_effort(key: &'static str, effort: Option<&str>) -> AxonResult<()> {
    let Some(effort) = effort else {
        return Ok(());
    };
    let Some(effort) = normalize_reasoning_effort(effort) else {
        return Err(AxonError::config_invalid(match key {
            "agent.reasoning_effort" => "agent.reasoning_effort 不能为空字符串",
            _ => "reasoning_effort 不能为空字符串",
        }));
    };
    if REASONING_EFFORT_VALUES.contains(&effort) {
        return Ok(());
    }
    Err(AxonError::config_invalid(match key {
        "agent.reasoning_effort" => {
            "agent.reasoning_effort 必须为 none、minimal、low、medium、high、xhigh 或 max"
        }
        _ => "reasoning_effort 必须为 none、minimal、low、medium、high、xhigh 或 max",
    }))
}

fn validate_model_provider_protocol(
    model: &ModelConfig,
    provider: &ProviderConfig,
) -> AxonResult<()> {
    if model.supports_embedding && !embedding_provider_protocol_supported(&provider.protocol) {
        return Err(AxonError::config_invalid(
            "embedding 模型必须使用 embedding-capable provider 协议",
        ));
    }
    if !model.supports_embedding && !chat_provider_protocol_supported(&provider.protocol) {
        return Err(AxonError::config_invalid(
            "非 embedding 模型必须使用 chat-capable provider 协议",
        ));
    }
    Ok(())
}

fn validate_memory_embedding(
    memory: &MemoryConfig,
    providers: &[ProviderConfig],
    models: &[ModelConfig],
) -> AxonResult<()> {
    if !memory.embedding.enabled {
        return Ok(());
    }
    if memory.embedding.model.trim().is_empty() {
        return Err(AxonError::config_invalid("memory.embedding.model 不能为空"));
    }
    let model = models
        .iter()
        .find(|model| {
            model.supports_embedding
                && (model.name == memory.embedding.model || model.id == memory.embedding.model)
        })
        .ok_or(AxonError::config_invalid(
            "memory.embedding.model 必须引用已配置 embedding 模型",
        ))?;
    let provider = providers
        .iter()
        .find(|provider| provider.name == model.provider)
        .ok_or(AxonError::config_invalid(
            "memory.embedding.model 的 provider 必须存在于 provider catalog",
        ))?;
    if !embedding_provider_protocol_supported(&provider.protocol) {
        return Err(AxonError::config_invalid(
            "memory.embedding.model 的 provider 必须使用 openai、openai-chatcompletions、dashscope、openai-multimodal-embeddings 或 volcengine-ark-multimodal 兼容协议",
        ));
    }
    Ok(())
}

fn provider_protocol_supported(protocol: &str) -> bool {
    matches!(
        protocol,
        PROVIDER_PROTOCOL_OPENAI
            | PROVIDER_PROTOCOL_OPENAI_CHAT
            | PROVIDER_PROTOCOL_ANTHROPIC
            | PROVIDER_PROTOCOL_DASHSCOPE
            | PROVIDER_PROTOCOL_OPENAI_MULTIMODAL_EMBEDDINGS
            | PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL
    )
}

fn embedding_provider_protocol_supported(protocol: &str) -> bool {
    matches!(
        protocol,
        PROVIDER_PROTOCOL_OPENAI
            | PROVIDER_PROTOCOL_OPENAI_CHAT
            | PROVIDER_PROTOCOL_DASHSCOPE
            | PROVIDER_PROTOCOL_OPENAI_MULTIMODAL_EMBEDDINGS
            | PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL
    )
}

fn chat_provider_protocol_supported(protocol: &str) -> bool {
    matches!(
        protocol,
        PROVIDER_PROTOCOL_OPENAI
            | PROVIDER_PROTOCOL_OPENAI_CHAT
            | PROVIDER_PROTOCOL_ANTHROPIC
            | PROVIDER_PROTOCOL_DASHSCOPE
    )
}

fn provider_wire_transport_supported(transport: &str) -> bool {
    matches!(
        transport,
        PROVIDER_WIRE_TRANSPORT_HTTP | PROVIDER_WIRE_TRANSPORT_WEBSOCKET
    )
}

fn websocket_url_allowed(websocket_url: &str) -> bool {
    websocket_url.starts_with("ws://")
        || websocket_url.starts_with("wss://")
        || websocket_url.starts_with("http://")
        || websocket_url.starts_with("https://")
}
