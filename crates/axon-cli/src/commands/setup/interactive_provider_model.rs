use super::{
    setup_interactive::{ModelInteractiveInput, ProviderInteractiveInput},
    setup_interactive_canvas::{
        ESCAPE_CHOICE, menu_rows, read_single_column_menu_choice, render_setup_frame,
    },
    setup_interactive_prompts::{
        prompt_choice, prompt_i32, prompt_optional_choice, prompt_optional_secret, prompt_string,
        prompt_u32, prompt_u64, prompt_usize, prompt_yes_no,
    },
    setup_interactive_style::{SetupStyle, paint},
};
use crate::SetupCommand;
use axon_config::setup::{SETUP_SECRET_PRESENT, secret_status};
use axon_config::{
    AxonConfig, DEFAULT_CHANNEL_PRIORITY, DEFAULT_CHANNEL_WEIGHT, ModelConfig, ProviderConfig,
    REASONING_EFFORT_VALUES,
};
use std::path::Path;

struct ProviderFrame<'a> {
    name: &'a str,
    protocol: &'a str,
    base_url: &'a str,
    transport: &'a str,
    websocket_url: &'a str,
    key_env: &'a str,
    api_key: &'a str,
    enabled: bool,
    priority: i32,
    weight: u32,
}

struct ModelFrame<'a> {
    id: &'a str,
    name: &'a str,
    provider: &'a str,
    reasoning_effort: &'a str,
    context_window: u64,
    max_output_tokens: u64,
    supports_vision: bool,
    supports_embedding: bool,
    embedding_dimension: usize,
}

pub(super) fn edit_provider_input(
    command: &SetupCommand,
    config: &AxonConfig,
    env_path: &Path,
    input: &mut ProviderInteractiveInput,
) {
    loop {
        let provider_name = input.resolved_provider_name.clone();
        let current = current_provider_by_name(config, &provider_name);
        let protocol = input
            .protocol
            .as_deref()
            .unwrap_or_else(|| current.map_or("openai", |provider| provider.protocol.as_str()))
            .to_owned();
        let base_url = input
            .base_url
            .as_deref()
            .unwrap_or_else(|| {
                current.map_or(
                    "[内置默认] https://api.openai.com/v1 (不写 TOML)",
                    |provider| provider.base_url.as_str(),
                )
            })
            .to_owned();
        let transport = input
            .wire_transport
            .as_deref()
            .unwrap_or_else(|| current.map_or("http", |provider| provider.wire_transport.as_str()))
            .to_owned();
        let websocket_url = input
            .websocket_url
            .as_deref()
            .unwrap_or_else(|| {
                current
                    .and_then(|provider| provider.websocket_url.as_deref())
                    .unwrap_or("[未配置] (自动按 Base URL 推导)")
            })
            .to_owned();
        let key_env = input
            .api_key_env
            .as_deref()
            .unwrap_or_else(|| {
                current
                    .and_then(|provider| provider.api_key_env.as_deref())
                    .unwrap_or("OPENAI_API_KEY")
            })
            .to_owned();
        let api_key_display = api_key_display(input, env_path, &key_env);
        let provider_enabled = input
            .provider_enabled
            .unwrap_or_else(|| current.is_none_or(|provider| provider.enabled));
        let provider_priority = input.provider_priority.unwrap_or_else(|| {
            current.map_or(DEFAULT_CHANNEL_PRIORITY, |provider| provider.priority)
        });
        let provider_weight = input
            .provider_weight
            .unwrap_or_else(|| current.map_or(DEFAULT_CHANNEL_WEIGHT, |provider| provider.weight));
        let frame = ProviderFrame {
            name: &provider_name,
            protocol: &protocol,
            base_url: &base_url,
            transport: &transport,
            websocket_url: &websocket_url,
            key_env: &key_env,
            api_key: api_key_display,
            enabled: provider_enabled,
            priority: provider_priority,
            weight: provider_weight,
        };
        let choices = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "C"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| render_provider_frame(&frame, prompt, focus, selected),
            "选择操作",
            &choices,
            0,
            false,
        );
        match choice.to_ascii_lowercase().as_str() {
            "1" => update_provider_name(input, &provider_name),
            "2" => set_provider_enabled(input, provider_enabled),
            "3" => {
                let protocol = prompt_provider_protocol(command, current);
                if protocol != ESCAPE_CHOICE {
                    input.protocol = Some(protocol);
                    input.configure_provider = true;
                }
            }
            "4" => set_provider_priority(input, provider_priority),
            "5" => {
                input.base_url = Some(prompt_string(
                    "Base URL",
                    "上游 API 基础地址；留空回车使用协议默认值。",
                    current.map_or("https://api.openai.com/v1", |provider| {
                        provider.base_url.as_str()
                    }),
                ));
                input.configure_provider = true;
            }
            "6" => set_provider_weight(input, provider_weight),
            "7" => {
                let transport = prompt_choice(
                    "传输方式",
                    "选择 HTTP 或 WebSocket 传输。",
                    &["http", "websocket"],
                    &transport,
                );
                if transport != ESCAPE_CHOICE {
                    input.wire_transport = Some(transport);
                    input.configure_provider = true;
                }
            }
            "8" => update_websocket_url(input, current),
            "9" => {
                input.api_key_env = Some(prompt_string(
                    "Key 环境变量名",
                    "密钥环境变量名称。",
                    current
                        .and_then(|provider| provider.api_key_env.as_deref())
                        .unwrap_or(&key_env),
                ));
                input.configure_provider = true;
            }
            "10" => {
                if let Some(api_key) =
                    prompt_optional_secret("API Key 密钥", "输入后会在确认保存时写入 .env。 ")
                {
                    input.api_key = Some(api_key);
                    input.clear_api_key = false;
                    input.configure_provider = true;
                }
            }
            "c" | "clear" => clear_provider_field(input),
            "0" | "" | ESCAPE_CHOICE => break,
            _ => {}
        }
    }
}

pub(super) fn edit_model_input(
    command: &SetupCommand,
    config: &AxonConfig,
    staged_provider_name: &str,
    input: &mut ModelInteractiveInput,
) {
    loop {
        let selected_provider_name = input
            .selected_model_provider
            .as_deref()
            .unwrap_or(staged_provider_name);
        let current = current_model(
            config,
            selected_provider_name,
            input.selected_model_id.as_deref(),
            command,
        );
        let model_id = input
            .model
            .as_deref()
            .unwrap_or_else(|| current.map_or("gpt-4o", |model| model.id.as_str()))
            .to_owned();
        let model_name = input
            .model_name
            .as_deref()
            .unwrap_or_else(|| current.map_or(model_id.as_str(), |model| model.name.as_str()))
            .to_owned();
        let model_provider = input
            .model_provider
            .as_deref()
            .unwrap_or(selected_provider_name)
            .to_owned();
        let reasoning_effort = input
            .agent_reasoning_effort
            .as_deref()
            .unwrap_or(config.agent.reasoning_effort.as_str())
            .to_owned();
        let context_window = input
            .context_window
            .unwrap_or_else(|| current.map_or(128_000, |model| model.context_window));
        let max_output_tokens = input.max_output_tokens.unwrap_or_else(|| {
            current
                .and_then(|model| model.max_output_tokens)
                .unwrap_or(128_000)
        });
        let supports_vision = input
            .supports_vision
            .unwrap_or_else(|| current.is_none_or(|model| model.supports_vision));
        let supports_embedding = input
            .supports_embedding
            .unwrap_or_else(|| current.is_some_and(|model| model.supports_embedding));
        let embedding_dimension = input
            .embedding_dimension
            .unwrap_or_else(|| current.map_or(0, |model| model.embedding_dimension));
        let frame = ModelFrame {
            id: &model_id,
            name: &model_name,
            provider: &model_provider,
            reasoning_effort: &reasoning_effort,
            context_window,
            max_output_tokens,
            supports_vision,
            supports_embedding,
            embedding_dimension,
        };
        let choices = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "C"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| render_model_frame(&frame, prompt, focus, selected),
            "选择操作",
            &choices,
            0,
            false,
        );
        match choice.to_ascii_lowercase().as_str() {
            "1" => set_model_id(input, &model_id),
            "2" => set_model_name(input, &model_name),
            "3" => set_model_provider(config, staged_provider_name, input, &model_provider),
            "4" => set_reasoning_effort(input, &reasoning_effort),
            "5" => set_context_window(input, context_window),
            "6" => set_max_output_tokens(input, max_output_tokens),
            "7" => set_supports_vision(input, supports_vision),
            "8" => set_supports_embedding(input, supports_embedding),
            "9" => set_embedding_dimension(input, embedding_dimension),
            "c" | "clear" => clear_model_field(input),
            "0" | "" | ESCAPE_CHOICE => break,
            _ => {}
        }
    }
}

fn render_provider_frame(
    frame: &ProviderFrame<'_>,
    prompt: &str,
    focus: usize,
    selected: Option<usize>,
) {
    let fields = menu_rows(
        &[
            format!("[1] Provider 名称    {}", frame.name),
            format!(
                "[2] 启用状态         {}",
                if frame.enabled {
                    "已启用"
                } else {
                    "已禁用"
                }
            ),
            format!("[3] API 协议         {}", frame.protocol),
            format!("[4] 路由优先级       {}", frame.priority),
            format!("[5] Base URL         {}", frame.base_url),
            format!("[6] 路由权重         {}", frame.weight),
            format!("[7] 传输方式         {}", frame.transport),
            format!("[8] WebSocket URL    {}", frame.websocket_url),
            format!("[9] Key 环境变量名   {}", frame.key_env),
            format!("[10] API Key         {}", frame.api_key),
            "[C] 清除可选项".to_owned(),
        ],
        focus,
        selected,
    );
    let mut lines = vec![
        paint(
            SetupStyle::Muted,
            format!("主菜单  ›  Providers  ›  [{}]", frame.name),
        ),
        String::new(),
    ];
    lines.extend(fields);
    lines.extend([
        String::new(),
        paint(
            SetupStyle::Muted,
            "方向键移动；回车确认；Esc 返回供应商目录",
        ),
        paint(SetupStyle::Accent, prompt),
    ]);
    render_setup_frame(&paint(SetupStyle::Accent, "PROVIDER CONFIGURATION"), &lines);
}

fn render_model_frame(frame: &ModelFrame<'_>, prompt: &str, focus: usize, selected: Option<usize>) {
    let context_display = if frame.context_window == 128_000 {
        "[内置默认] 128000 (不写 TOML)".to_owned()
    } else {
        frame.context_window.to_string()
    };
    let fields = menu_rows(
        &[
            format!("[1] 模型 ID          {}", frame.id),
            format!("[2] 模型别名         {}", frame.name),
            format!("[3] Provider          {}", frame.provider),
            format!("[4] 主 Agent 推理强度 {}", frame.reasoning_effort),
            format!("[5] 上下文窗口        {context_display}"),
            format!("[6] 最大输出 Token    {}", frame.max_output_tokens),
            format!("[7] 是否支持视觉      {}", frame.supports_vision),
            format!("[8] 是否为 Embedding  {}", frame.supports_embedding),
            format!("[9] Embedding 维度    {}", frame.embedding_dimension),
            "[C] 清除可选项".to_owned(),
        ],
        focus,
        selected,
    );
    let mut lines = vec![
        paint(
            SetupStyle::Muted,
            format!("主菜单  ›  Models  ›  [{}]", frame.id),
        ),
        String::new(),
    ];
    lines.extend(fields);
    lines.extend([
        String::new(),
        paint(SetupStyle::Muted, "方向键移动；回车确认；Esc 返回模型目录"),
        paint(SetupStyle::Accent, prompt),
    ]);
    render_setup_frame(&paint(SetupStyle::Accent, "MODELS CONFIGURATION"), &lines);
}

fn api_key_display(
    input: &ProviderInteractiveInput,
    env_path: &Path,
    key_env: &str,
) -> &'static str {
    if input.api_key.is_some() {
        "•••••••••••••••••••• [新输入]"
    } else if secret_status(env_path, key_env) == SETUP_SECRET_PRESENT {
        "•••••••••••••••••••• [已就绪]"
    } else {
        "[缺失]"
    }
}

fn update_provider_name(input: &mut ProviderInteractiveInput, provider_name: &str) {
    let name = prompt_string(
        "Provider 名称",
        "上游模型服务的逻辑标识名称。",
        provider_name,
    );
    if name != provider_name {
        input.provider_name = Some(name.clone());
        input.resolved_provider_name = name;
        input.configure_provider = true;
    }
}

fn update_websocket_url(input: &mut ProviderInteractiveInput, current: Option<&ProviderConfig>) {
    let value = prompt_string(
        "WebSocket URL",
        "留空回车由运行时从 Base URL 自动推导。",
        current
            .and_then(|provider| provider.websocket_url.as_deref())
            .unwrap_or(""),
    );
    input.websocket_url = (!value.is_empty()).then_some(value);
    input.clear_websocket_url = input.websocket_url.is_none();
    input.configure_provider = true;
}

fn clear_provider_field(input: &mut ProviderInteractiveInput) {
    match prompt_choice(
        "清除 Provider 字段",
        "选择要清除的可选项。",
        &["WebSocket URL", "API Key"],
        "WebSocket URL",
    )
    .as_str()
    {
        "WebSocket URL" => {
            input.websocket_url = None;
            input.clear_websocket_url = true;
            input.configure_provider = true;
        }
        "API Key" => {
            input.api_key = None;
            input.clear_api_key = true;
        }
        _ => {}
    }
}

fn set_model_id(input: &mut ModelInteractiveInput, model_id: &str) {
    input.model = Some(prompt_string(
        "模型 ID",
        "实际发送给上游 Provider 的模型标识符。",
        model_id,
    ));
    input.configure_model = true;
}

fn set_model_name(input: &mut ModelInteractiveInput, model_name: &str) {
    input.model_name = Some(prompt_string(
        "模型别名",
        "本地路由引用的模型别名。",
        model_name,
    ));
    input.configure_model = true;
}

fn set_model_provider(
    config: &AxonConfig,
    staged_provider_name: &str,
    input: &mut ModelInteractiveInput,
    current_provider: &str,
) {
    let mut providers = config
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect::<Vec<_>>();
    if !staged_provider_name.is_empty()
        && !providers.iter().any(|name| name == staged_provider_name)
    {
        providers.push(staged_provider_name.to_owned());
    }
    if providers.is_empty() {
        return;
    }
    let provider_refs = providers.iter().map(String::as_str).collect::<Vec<_>>();
    let selected = prompt_choice(
        "模型 Provider",
        "选择该模型所属的 Provider；包含当前配置和暂存的新 Provider。",
        &provider_refs,
        current_provider,
    );
    if selected != ESCAPE_CHOICE {
        input.model_provider = Some(selected);
        input.configure_model = true;
    }
}

fn set_reasoning_effort(input: &mut ModelInteractiveInput, value: &str) {
    if let Some(reasoning_effort) = prompt_optional_choice(
        "主 Agent 推理强度",
        "仅作用于 default_model。",
        REASONING_EFFORT_VALUES,
        Some(value),
    ) {
        input.agent_reasoning_effort = Some(reasoning_effort);
        input.clear_reasoning_effort = false;
        input.configure_model = true;
    }
}

fn set_context_window(input: &mut ModelInteractiveInput, value: u64) {
    input.context_window = Some(prompt_u64(
        "上下文窗口",
        "模型支持的最大上下文 Token 数。",
        value,
    ));
    input.configure_model = true;
}

fn set_max_output_tokens(input: &mut ModelInteractiveInput, value: u64) {
    input.max_output_tokens = Some(prompt_u64(
        "最大输出 Token",
        "模型单次最大输出 Token 数。",
        value,
    ));
    input.clear_max_output_tokens = false;
    input.configure_model = true;
}

fn set_supports_vision(input: &mut ModelInteractiveInput, value: bool) {
    input.supports_vision = Some(prompt_yes_no("是否支持视觉输入", value));
    input.configure_model = true;
}

fn set_supports_embedding(input: &mut ModelInteractiveInput, value: bool) {
    input.supports_embedding = Some(prompt_yes_no("是否为 Embedding 模型", value));
    input.configure_model = true;
}

fn set_embedding_dimension(input: &mut ModelInteractiveInput, value: usize) {
    input.embedding_dimension = Some(prompt_usize(
        "Embedding 维度",
        "向量维度，例如 1536。",
        value,
    ));
    input.configure_model = true;
}

fn clear_model_field(input: &mut ModelInteractiveInput) {
    match prompt_choice(
        "清除模型字段",
        "选择要恢复默认值的可选项。",
        &["推理强度", "最大输出 Token"],
        "推理强度",
    )
    .as_str()
    {
        "推理强度" => {
            input.agent_reasoning_effort = None;
            input.clear_reasoning_effort = true;
        }
        "最大输出 Token" => {
            input.max_output_tokens = None;
            input.clear_max_output_tokens = true;
        }
        _ => {}
    }
}

fn prompt_provider_protocol(_command: &SetupCommand, current: Option<&ProviderConfig>) -> String {
    prompt_choice(
        "Provider 协议",
        "选择上游 API 协议。",
        &[
            "openai",
            "openai-chatcompletions",
            "anthropic",
            "dashscope",
            "volcengine-ark-multimodal",
        ],
        current.map_or_else(|| "openai", |provider| provider.protocol.as_str()),
    )
}

fn current_provider_by_name<'a>(
    config: &'a AxonConfig,
    provider_name: &str,
) -> Option<&'a ProviderConfig> {
    config
        .providers
        .iter()
        .find(|provider| provider.name == provider_name)
}

fn current_model<'a>(
    config: &'a AxonConfig,
    provider_name: &str,
    selected_model_id: Option<&str>,
    command: &SetupCommand,
) -> Option<&'a ModelConfig> {
    selected_model_id
        .and_then(|model_id| current_model_by_id(config, provider_name, model_id))
        .or_else(|| {
            command
                .model
                .as_deref()
                .and_then(|model_id| current_model_by_id(config, provider_name, model_id))
        })
}

fn current_model_by_id<'a>(
    config: &'a AxonConfig,
    provider_name: &str,
    model_id: &str,
) -> Option<&'a ModelConfig> {
    config
        .models
        .iter()
        .find(|model| model.provider == provider_name && model.id == model_id)
}

fn set_provider_enabled(input: &mut ProviderInteractiveInput, value: bool) {
    input.provider_enabled = Some(prompt_yes_no("是否启用 Provider", value));
    input.configure_provider = true;
}

fn set_provider_priority(input: &mut ProviderInteractiveInput, value: i32) {
    input.provider_priority = Some(prompt_i32(
        "路由优先级",
        "数值越大越优先；同优先级 Provider 按权重进行均衡。",
        value,
    ));
    input.configure_provider = true;
}

fn set_provider_weight(input: &mut ProviderInteractiveInput, value: u32) {
    input.provider_weight = Some(prompt_u32(
        "路由权重",
        "同优先级 Provider 的相对路由配额。",
        value,
    ));
    input.configure_provider = true;
}
