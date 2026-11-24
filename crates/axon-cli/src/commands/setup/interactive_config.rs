use std::collections::BTreeMap;

use axon_config::{
    AxonConfig, ConfigKeySpec, ConfigValueKind, ModelConfig, config_key_specs, get_config_value,
};
use serde::Deserialize;

use super::{
    setup_interactive_canvas::{
        ESCAPE_CHOICE, menu_rows, read_frame_choice, read_single_column_menu_choice,
        render_setup_frame,
    },
    setup_interactive_prompts::prompt_choice,
    setup_interactive_style::{SetupStyle, paint},
    setup_option_help::{input_hint, key_detail, option_detail},
};

const CHOICE_EMPTY: &str = "留空";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SetupClearField {
    WebsocketUrl,
    MaxOutputTokens,
    ReasoningEffort,
    ApiKey,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AgentSetupConfig {
    pub provider_name: Option<String>,
    pub protocol: Option<String>,
    pub base_url: Option<String>,
    pub wire_transport: Option<String>,
    pub websocket_url: Option<String>,
    pub provider_enabled: Option<bool>,
    pub provider_priority: Option<i32>,
    pub provider_weight: Option<u32>,
    pub model: Option<String>,
    pub model_name: Option<String>,
    #[serde(rename = "reasoningEffort")]
    pub agent_reasoning_effort: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_vision: Option<bool>,
    pub supports_embedding: Option<bool>,
    pub embedding_dimension: Option<usize>,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
    pub clear_fields: Option<Vec<SetupClearField>>,
    pub no_validate: Option<bool>,
    #[serde(default)]
    pub config_values: BTreeMap<String, String>,
}

impl AgentSetupConfig {
    pub(crate) fn is_clear_field(&self, name: &str) -> bool {
        let target = match name {
            "websocketUrl" => SetupClearField::WebsocketUrl,
            "maxOutputTokens" => SetupClearField::MaxOutputTokens,
            "reasoningEffort" => SetupClearField::ReasoningEffort,
            "apiKey" => SetupClearField::ApiKey,
            _ => return false,
        };
        self.clear_fields
            .as_ref()
            .is_some_and(|fields| fields.contains(&target))
    }
}

pub(crate) fn collect_module_config_values(
    config: &AxonConfig,
    module: &str,
    values: &mut BTreeMap<String, String>,
) {
    let specs = editable_setup_specs();
    let mut effective = current_config_values(config, &specs);
    effective.extend(values.clone());
    let module_specs = module_specs(&specs, module);
    loop {
        let choices = module_specs
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let value = effective
                    .get(spec.key)
                    .map(String::as_str)
                    .unwrap_or(spec.default_value);
                format!(
                    "[{}] {}: {}",
                    index + 1,
                    spec.key,
                    display_empty_value(value)
                )
            })
            .collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| {
                let mut lines = vec![
                    paint(
                        SetupStyle::Muted,
                        format!("主菜单  ›  {} 配置", module_display_name(module)),
                    ),
                    String::new(),
                ];
                lines.extend(menu_rows(&choices, focus, selected));
                lines.extend([
                    String::new(),
                    paint(SetupStyle::Muted, "方向键移动；回车编辑；Esc 返回上级菜单"),
                    paint(SetupStyle::Accent, prompt),
                ]);
                render_setup_frame(&paint(SetupStyle::Accent, "CONFIGURATION"), &lines);
            },
            "选择字段",
            &choices,
            0,
            false,
        );
        if is_back_choice(&choice) {
            break;
        }
        let Some(index) = choices.iter().position(|item| item == &choice) else {
            continue;
        };
        let Some(spec) = module_specs.get(index).copied() else {
            continue;
        };
        if !dependencies_enabled(spec, &effective) {
            continue;
        }
        if let Some(value) = prompt_config_value(config, spec, &effective) {
            effective.insert(spec.key.to_owned(), value.clone());
            values.insert(spec.key.to_owned(), value);
        }
    }
}

fn editable_setup_specs() -> Vec<&'static ConfigKeySpec> {
    config_key_specs()
        .iter()
        .filter(|spec| spec.editable && !spec.setup_module.is_empty())
        .collect()
}

fn module_specs(specs: &[&'static ConfigKeySpec], module: &str) -> Vec<&'static ConfigKeySpec> {
    specs
        .iter()
        .copied()
        .filter(|spec| spec.setup_module == module)
        .collect()
}

fn current_config_values(
    config: &AxonConfig,
    specs: &[&'static ConfigKeySpec],
) -> BTreeMap<String, String> {
    specs
        .iter()
        .filter_map(|spec| {
            get_config_value(config, spec.key).map(|value| (spec.key.to_owned(), value.to_string()))
        })
        .collect()
}

fn dependencies_enabled(spec: &ConfigKeySpec, effective: &BTreeMap<String, String>) -> bool {
    spec.depends_on.iter().all(|dependency| {
        effective.get(*dependency).is_some_and(|value| {
            let value = value.trim();
            value == "true" || (!value.is_empty() && value != "false" && value != "0")
        })
    })
}

fn prompt_config_value(
    config: &AxonConfig,
    spec: &ConfigKeySpec,
    effective: &BTreeMap<String, String>,
) -> Option<String> {
    let current = effective
        .get(spec.key)
        .cloned()
        .unwrap_or_else(|| default_prompt_value(spec));
    if requires_model_selection(spec) {
        return prompt_required_model_value(config, spec, &current);
    }
    let description = config_prompt_description(spec, &current);
    let options = config_input_options(spec);
    let prompt = config_input_prompt(spec, &current);
    let value = if options.is_empty() {
        read_frame_choice(
            |input| {
                let mut lines = vec![
                    paint(SetupStyle::Muted, "主菜单  ›  配置项编辑"),
                    String::new(),
                    paint(SetupStyle::Accent, spec.key),
                ];
                lines.extend(description.lines().map(str::to_owned));
                lines.extend([String::new(), paint(SetupStyle::Accent, input)]);
                render_setup_frame(&paint(SetupStyle::Accent, "CONFIGURATION"), &lines);
            },
            &prompt,
        )
    } else {
        let refs = options.iter().map(String::as_str).collect::<Vec<_>>();
        let default_option = options
            .iter()
            .find(|option| selectable_option_value(option) == current)
            .map(String::as_str)
            .unwrap_or(&current);
        prompt_choice(
            spec.key,
            &format!("{description}\n\n方向键移动；空格选中；回车确认；Esc 返回。"),
            &refs,
            default_option,
        )
    };
    let value = normalize_config_input(spec, &current, &options, &value);
    value_changed(&value, &current).then_some(value)
}

fn prompt_required_model_value(
    config: &AxonConfig,
    spec: &ConfigKeySpec,
    current: &str,
) -> Option<String> {
    let models = selectable_models(config, spec);
    if models.is_empty() {
        let _ = read_frame_choice(
            |input| {
                let lines = vec![
                    paint(SetupStyle::Accent, spec.key),
                    String::new(),
                    paint(
                        SetupStyle::Muted,
                        "当前没有可用的模型；请先在 Models 中新增或配置模型。",
                    ),
                    paint(SetupStyle::Muted, "按 Esc 返回。"),
                    String::new(),
                    paint(SetupStyle::Accent, input),
                ];
                render_setup_frame(&paint(SetupStyle::Accent, "MODEL SELECTION"), &lines);
            },
            "返回",
        );
        return None;
    }

    let values = models
        .iter()
        .map(|model| model_selection_value(spec, model))
        .collect::<Vec<_>>();
    let labels = model_choice_labels(&models);
    let initial_focus = values
        .iter()
        .position(|model_id| model_id == current)
        .unwrap_or(0);
    let selected = read_single_column_menu_choice(
        |_prompt, focus, selected| {
            let mut lines = vec![
                paint(
                    SetupStyle::Muted,
                    format!("主菜单  ›  {} 配置", module_display_name(spec.setup_module)),
                ),
                String::new(),
                paint(SetupStyle::Accent, spec.key),
                paint(SetupStyle::Muted, "从已配置模型中选择"),
                paint(
                    SetupStyle::Muted,
                    format!("当前：{}", display_empty_value(current)),
                ),
                String::new(),
            ];
            lines.extend(menu_rows(&labels, focus, selected));
            lines.extend([
                String::new(),
                paint(
                    SetupStyle::Muted,
                    "方向键移动；空格选中；回车确认；Esc 返回",
                ),
            ]);
            render_setup_frame(&paint(SetupStyle::Accent, "MODEL SELECTION"), &lines);
        },
        "选择模型",
        &values,
        initial_focus,
        true,
    );
    (selected != ESCAPE_CHOICE)
        .then_some(selected)
        .filter(|selected| value_changed(selected, current))
}

fn selectable_models<'a>(config: &'a AxonConfig, spec: &ConfigKeySpec) -> Vec<&'a ModelConfig> {
    match spec.key {
        "agent.default_model" | "agent.fast_model" => config
            .models
            .iter()
            .filter(|model| !model.supports_embedding)
            .collect(),
        "memory.embedding.model" => config
            .models
            .iter()
            .filter(|model| model.supports_embedding)
            .collect(),
        _ => Vec::new(),
    }
}

fn model_choice_labels(models: &[&ModelConfig]) -> Vec<String> {
    models
        .iter()
        .enumerate()
        .map(|(index, model)| format!("[{}] {} / {}", index + 1, model.id, model.provider))
        .collect()
}

fn requires_model_selection(spec: &ConfigKeySpec) -> bool {
    matches!(
        spec.key,
        "agent.default_model" | "agent.fast_model" | "memory.embedding.model"
    )
}

fn model_selection_value(spec: &ConfigKeySpec, model: &ModelConfig) -> String {
    match spec.key {
        "agent.default_model" | "agent.fast_model" => model.name.clone(),
        _ => model.id.clone(),
    }
}

fn config_input_options(spec: &ConfigKeySpec) -> Vec<String> {
    match spec.kind {
        ConfigValueKind::Bool => ["true", "false"].as_slice(),
        ConfigValueKind::String if !spec.options.is_empty() => spec.options,
        _ => &[],
    }
    .iter()
    .enumerate()
    .map(|(index, option)| {
        option_detail(spec.key, option).map_or_else(
            || format!("[{}] {option}", index + 1),
            |detail| format!("[{}] {option}：{detail}", index + 1),
        )
    })
    .collect()
}

fn config_input_prompt(spec: &ConfigKeySpec, current: &str) -> String {
    let clear_hint = match spec.kind {
        ConfigValueKind::String | ConfigValueKind::StringList => {
            format!("，输入 {CHOICE_EMPTY} 清空")
        }
        _ => String::new(),
    };
    format!(
        "{} [{}]（回车保留{}） ❯",
        spec.key,
        display_empty_value(current),
        clear_hint
    )
}

fn normalize_config_input(
    spec: &ConfigKeySpec,
    current: &str,
    options: &[String],
    input: &str,
) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return current.to_owned();
    }
    if trimmed == ESCAPE_CHOICE {
        return current.to_owned();
    }
    if matches!(
        spec.kind,
        ConfigValueKind::String | ConfigValueKind::StringList
    ) && trimmed == CHOICE_EMPTY
    {
        return String::new();
    }
    let is_selectable = matches!(spec.kind, ConfigValueKind::Bool)
        || (matches!(spec.kind, ConfigValueKind::String) && !spec.options.is_empty());
    if is_selectable {
        if let Some(index) = trimmed
            .strip_prefix('[')
            .and_then(|value| value.split_once(']'))
            .and_then(|(index, _)| index.parse::<usize>().ok())
            .and_then(|value| value.checked_sub(1))
            .and_then(|index| options.get(index))
        {
            return selectable_option_value(index);
        }
        if let Some(option) = options.iter().find(|option| option.trim() == trimmed) {
            return selectable_option_value(option);
        }
        if let Some(option) = options
            .iter()
            .find(|option| selectable_option_value(option) == trimmed)
        {
            return selectable_option_value(option);
        }
        if let Some(index) = trimmed
            .parse::<usize>()
            .ok()
            .and_then(|value| value.checked_sub(1))
            .and_then(|index| options.get(index))
        {
            return selectable_option_value(index);
        }
        if matches!(spec.kind, ConfigValueKind::Bool) {
            return current.to_owned();
        }
    }
    trimmed.to_owned()
}

fn selectable_option_value(option: &str) -> String {
    option
        .split_once(']')
        .map(|(_, option)| option)
        .unwrap_or(option)
        .split('：')
        .next()
        .unwrap_or(option)
        .trim()
        .to_owned()
}

fn module_display_name(module: &str) -> &str {
    match module {
        "agent" => "Agent",
        "remote" => "Remote",
        "memory" => "记忆",
        "channel_balancer" => "通道均衡",
        _ => module,
    }
}

fn is_back_choice(value: &str) -> bool {
    value.trim().is_empty() || value == ESCAPE_CHOICE
}

fn config_prompt_description(spec: &ConfigKeySpec, current: &str) -> String {
    let mut lines = vec![
        format!("用途：{}", spec.description),
        key_detail(spec.key)
            .map(|detail| format!("说明：{detail}"))
            .unwrap_or_default(),
        format!(
            "输入格式：{}",
            input_hint(spec.key, spec.kind, !spec.options.is_empty())
        ),
        String::new(),
        format!("默认值：{}", spec.default_value),
        format!("当前值：{}", display_empty_value(current)),
    ];
    if !spec.depends_on.is_empty() {
        lines.push(String::new());
        lines.push(format!("联动依赖：{}", spec.depends_on.join(", ")));
    }
    lines.join("\n")
}

fn default_prompt_value(spec: &ConfigKeySpec) -> String {
    match spec.default_value {
        "空字符串" | "[]" => String::new(),
        value => value.to_owned(),
    }
}

fn display_empty_value(value: &str) -> &str {
    if value.is_empty() {
        "空字符串"
    } else {
        value
    }
}

fn value_changed(value: &str, current: &str) -> bool {
    value != current
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_config::{DEFAULT_MODEL_CONTEXT_WINDOW, DEFAULT_MODEL_MAX_OUTPUT_TOKENS, ModelConfig};

    fn model(id: &str, embedding: bool) -> ModelConfig {
        ModelConfig {
            name: id.to_owned(),
            id: id.to_owned(),
            provider: "provider".to_owned(),
            context_window: DEFAULT_MODEL_CONTEXT_WINDOW,
            max_output_tokens: Some(DEFAULT_MODEL_MAX_OUTPUT_TOKENS),
            supports_vision: true,
            supports_embedding: embedding,
            embedding_dimension: usize::from(embedding),
            requires_reasoning_content_on_assistant_messages: false,
        }
    }

    #[test]
    fn memory_embedding_model_only_lists_embedding_models() {
        let config = AxonConfig {
            models: vec![model("chat-model", false), model("embedding-model", true)],
            ..AxonConfig::default()
        };
        let spec = config_key_specs()
            .iter()
            .find(|spec| spec.key == "memory.embedding.model")
            .expect("memory.embedding.model 应存在");

        assert!(requires_model_selection(spec));
        assert_eq!(
            selectable_models(&config, spec)
                .iter()
                .map(|model| model.id.clone())
                .collect::<Vec<_>>(),
            vec!["embedding-model".to_owned()]
        );
    }

    #[test]
    fn agent_model_slots_write_model_name_while_displaying_model_id() {
        let mut configured_model = model("provider-model-id", false);
        configured_model.name = "agent-routing-name".to_owned();
        let config = AxonConfig {
            models: vec![configured_model],
            ..AxonConfig::default()
        };
        let spec = config_key_specs()
            .iter()
            .find(|spec| spec.key == "agent.default_model")
            .expect("agent.default_model 应存在");

        let models = selectable_models(&config, spec);
        assert!(requires_model_selection(spec));
        assert_eq!(
            model_choice_labels(&models),
            vec!["[1] provider-model-id / provider".to_owned()]
        );
        assert_eq!(model_selection_value(spec, models[0]), "agent-routing-name");
    }

    #[test]
    fn normalize_boolean_config_selection_returns_canonical_values() {
        let spec = config_key_specs()
            .iter()
            .find(|spec| spec.key == "channel_balancer.telemetry_persistence_enabled")
            .expect("遥测持久化开关配置应存在");
        let options = config_input_options(spec);

        assert_eq!(
            normalize_config_input(
                spec,
                "false",
                &options,
                "[1] true：开启后将渠道性能与缓存遥测追加保存到 data/channel-metrics。"
            ),
            "true"
        );
        assert_eq!(
            normalize_config_input(
                spec,
                "true",
                &options,
                "[2] false：开启后将渠道性能与缓存遥测追加保存到 data/channel-metrics。"
            ),
            "false"
        );
        assert_eq!(normalize_config_input(spec, "false", &options, "1"), "true");
        assert_eq!(normalize_config_input(spec, "true", &options, "2"), "false");
        assert_eq!(normalize_config_input(spec, "false", &options, ""), "false");
        assert_eq!(
            normalize_config_input(spec, "false", &options, "false"),
            "false"
        );
        assert_eq!(
            normalize_config_input(
                spec,
                "false",
                &options,
                "configValue 'channel_balancer.telemetry_persistence_enabled'"
            ),
            "false"
        );
        assert!(
            options
                .iter()
                .find(|option| selectable_option_value(option) == "false")
                .is_some_and(|option| option.starts_with("[2] false"))
        );
    }
}
