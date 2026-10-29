use crate::{
    AxonConfig, DEFAULT_AGENT_REASONING_EFFORT, DEFAULT_MODEL_CONTEXT_WINDOW,
    DEFAULT_MODEL_MAX_OUTPUT_TOKENS, ModelConfig, ProviderConfig, default_base_url_for_protocol,
    set_config_value,
};

use super::validator::{
    agent_model_slots_reference_missing, selected_provider, validate_env_reference,
};
use super::{EnvKeyChange, ExplicitEntry, SetupIntent, SetupPlannerOptions};

/// 纯函数 planner 的结果；不包含任何 secret 明文。
#[derive(Debug, Clone, PartialEq)]
pub struct SetupPlan {
    /// 应用显式意图后的候选配置。
    pub candidate_config: AxonConfig,
    /// 需要写入相邻 `.env` 的凭据引用变更。
    pub env_changes: Vec<EnvKeyChange>,
    /// 面向用户的简短变更说明。
    pub changed_summary: Vec<String>,
    /// 最终生效的 Provider 名称。
    pub effective_provider_name: String,
    /// 最终生效的模型 ID。
    pub effective_model_id: String,
    /// 最终生效的 Provider API key 环境变量名。
    pub effective_api_key_env: String,
}

/// Setup 规划器。
///
/// Planner 只接收当前配置和稳定的用户意图，不读取文件、环境变量、终端或网络。
#[derive(Debug, Default, Clone, Copy)]
pub struct SetupPlanner;

impl SetupPlanner {
    /// 将最小用户意图转换为一个可校验的候选配置。
    pub fn plan(&self, current: &AxonConfig, intent: &SetupIntent) -> Result<SetupPlan, String> {
        let mut candidate = current.clone();
        let mut changed_summary = Vec::new();

        if let Some(provider_name) = intent.actions.reset_provider.as_deref() {
            candidate
                .models
                .retain(|model| model.provider != provider_name);
            candidate
                .providers
                .retain(|provider| provider.name != provider_name);
            if candidate.models.iter().all(|model| {
                model.name != candidate.agent.default_model
                    && model.id != candidate.agent.default_model
            }) {
                candidate.agent.default_model.clear();
            }
            changed_summary.push(format!("已移除供应商 {provider_name} 及其模型"));
        }

        if let ExplicitEntry::Set(model_ref) = &intent.default_model {
            let model = candidate
                .models
                .iter()
                .find(|model| model.id == *model_ref || model.name == *model_ref)
                .ok_or_else(|| format!("找不到模型“{model_ref}”，请先添加或启用该模型"))?;
            candidate.agent.default_model = model.name.clone();
            changed_summary.push(format!("主 Axon 将使用模型 {}", model.name));
        } else if matches!(intent.default_model, ExplicitEntry::Clear) {
            candidate.agent.default_model.clear();
            changed_summary.push("已清除主 Axon 默认模型".to_owned());
        }

        if let ExplicitEntry::Set(env_name) = &intent.credential_env {
            validate_env_reference(env_name)?;
            let provider_name =
                selected_provider(&candidate, &intent.default_model).map(str::to_owned);
            if let Some(provider) = candidate
                .providers
                .iter_mut()
                .find(|provider| Some(provider.name.as_str()) == provider_name.as_deref())
            {
                provider.api_key_env = Some(env_name.clone());
                changed_summary.push(format!(
                    "{} 的密钥改为读取环境变量 {env_name}",
                    provider.name
                ));
            }
        }

        Ok(SetupPlan {
            candidate_config: candidate,
            env_changes: Vec::new(),
            changed_summary,
            effective_provider_name: current
                .providers
                .first()
                .map(|provider| provider.name.clone())
                .unwrap_or_else(|| "openai".to_owned()),
            effective_model_id: current
                .models
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| "gpt-4o".to_owned()),
            effective_api_key_env: current
                .providers
                .first()
                .and_then(|provider| provider.api_key_env.clone())
                .unwrap_or_else(|| "OPENAI_API_KEY".to_owned()),
        })
    }

    /// 规划完整 setup 候选；所有文件、终端和环境访问仍留在 CLI bootstrap adapter。
    pub fn plan_explicit(
        &self,
        current: &AxonConfig,
        intent: &SetupIntent,
        options: &SetupPlannerOptions,
    ) -> Result<SetupPlan, String> {
        let entries = &options.entries;
        let actions = &intent.actions;
        let stable = self.plan(current, intent)?;
        let mut candidate = stable.candidate_config;
        let mut changed = stable.changed_summary;

        if let Some(provider_name) = actions.reset_provider.as_deref() {
            candidate
                .models
                .retain(|model| model.provider != provider_name);
            candidate
                .providers
                .retain(|provider| provider.name != provider_name);
            changed.push("reset_provider".to_owned());
            if agent_model_slots_reference_missing(&candidate) {
                if candidate.models.is_empty() {
                    candidate.agent.default_model.clear();
                    candidate.agent.fast_model.clear();
                } else if entries.model_id.as_set().is_none()
                    && entries.model_name.as_set().is_none()
                {
                    return Err(format!(
                        "重置 provider '{provider_name}' 后被引用的模型被删除，必须使用 --model 显式选择替代模型"
                    ));
                }
            }
        }

        if actions.reset_agent_models {
            candidate.agent.default_model.clear();
            candidate.agent.fast_model.clear();
            candidate.agent.reasoning_effort = DEFAULT_AGENT_REASONING_EFFORT.to_owned();
            changed.push("reset_agent_models".to_owned());
            if !candidate.models.is_empty()
                && entries.model_id.as_set().is_none()
                && entries.model_name.as_set().is_none()
            {
                return Err(
                    "重置 agent 模型槽位后由于存在可用模型，必须通过 --model 显式指定模型选择"
                        .to_owned(),
                );
            }
        }

        let target_provider = entries
            .provider_name
            .as_set()
            .cloned()
            .or_else(|| options.selected_provider_name.clone())
            .or_else(|| {
                candidate
                    .providers
                    .first()
                    .map(|provider| provider.name.clone())
            })
            .unwrap_or_else(|| "openai".to_owned());
        let provider_exists = candidate
            .providers
            .iter()
            .any(|provider| provider.name == target_provider);
        let provider_explicit = entries.provider_name.is_changed()
            || entries.protocol.is_changed()
            || entries.base_url.is_changed()
            || entries.wire_transport.is_changed()
            || entries.websocket_url.is_changed()
            || entries.provider_enabled.is_changed()
            || entries.provider_priority.is_changed()
            || entries.provider_weight.is_changed()
            || entries.api_key_env.is_changed();
        let update_provider =
            !(options.quick && provider_exists && actions.reset_provider.is_none())
                && provider_explicit;

        if update_provider {
            let protocol = entries
                .protocol
                .as_set()
                .cloned()
                .unwrap_or_else(|| "openai".to_owned());
            let base_url = entries.base_url.as_set().cloned().unwrap_or_else(|| {
                default_base_url_for_protocol(&protocol)
                    .unwrap_or("https://api.openai.com/v1")
                    .to_owned()
            });
            let transport = entries
                .wire_transport
                .as_set()
                .cloned()
                .unwrap_or_else(|| "http".to_owned());
            let key_env = entries
                .api_key_env
                .as_set()
                .cloned()
                .unwrap_or_else(|| "OPENAI_API_KEY".to_owned());
            let websocket = match &entries.websocket_url {
                ExplicitEntry::Set(value) => Some(value.clone()),
                ExplicitEntry::Clear => None,
                ExplicitEntry::Unchanged => None,
            };
            if let Some(provider) = candidate
                .providers
                .iter_mut()
                .find(|provider| provider.name == target_provider)
            {
                if entries.protocol.is_changed() {
                    provider.protocol = protocol;
                }
                if entries.base_url.is_changed() {
                    provider.base_url = base_url;
                }
                if entries.wire_transport.is_changed() {
                    provider.wire_transport = transport;
                }
                if entries.api_key_env.is_changed() {
                    provider.api_key_env = Some(key_env);
                }
                if entries.websocket_url.is_changed() {
                    provider.websocket_url = websocket;
                }
                if let ExplicitEntry::Set(value) = entries.provider_enabled {
                    provider.enabled = value;
                }
                if let ExplicitEntry::Set(value) = entries.provider_priority {
                    provider.priority = value;
                }
                if let ExplicitEntry::Set(value) = entries.provider_weight {
                    provider.weight = value;
                }
                changed.push("provider.updated".to_owned());
            } else {
                let mut provider = ProviderConfig::new(&target_provider, &protocol, &base_url);
                provider.wire_transport = transport;
                provider.api_key_env = Some(key_env);
                provider.websocket_url = websocket;
                if let ExplicitEntry::Set(value) = entries.provider_enabled {
                    provider.enabled = value;
                }
                if let ExplicitEntry::Set(value) = entries.provider_priority {
                    provider.priority = value;
                }
                if let ExplicitEntry::Set(value) = entries.provider_weight {
                    provider.weight = value;
                }
                candidate.providers.push(provider);
                changed.push("provider.added".to_owned());
            }
        } else if provider_exists {
            changed.push("provider.skipped".to_owned());
        }

        let source_model_index = options
            .selected_model_id
            .as_deref()
            .zip(options.selected_model_provider.as_deref())
            .and_then(|(id, provider)| {
                candidate
                    .models
                    .iter()
                    .position(|model| model.id == id && model.provider == provider)
            })
            .or_else(|| {
                entries.model_id.as_set().and_then(|id| {
                    candidate
                        .models
                        .iter()
                        .position(|model| model.id == *id && model.provider == target_provider)
                })
            });
        let model_exists = source_model_index.is_some();
        let model_explicit = entries.model_id.is_changed()
            || entries.model_name.is_changed()
            || entries.context_window.is_changed()
            || entries.max_output_tokens.is_changed()
            || entries.supports_vision.is_changed()
            || entries.supports_embedding.is_changed()
            || entries.embedding_dimension.is_changed()
            || entries.model_provider.is_changed();
        let update_model =
            !(options.quick && model_exists && actions.reset_provider.is_none()) && model_explicit;
        let model_provider = entries
            .model_provider
            .as_set()
            .cloned()
            .or_else(|| options.selected_model_provider.clone())
            .unwrap_or_else(|| target_provider.clone());
        let model_id = entries
            .model_id
            .as_set()
            .cloned()
            .or_else(|| {
                source_model_index
                    .and_then(|index| candidate.models.get(index))
                    .map(|model| model.id.clone())
            })
            .unwrap_or_else(|| "gpt-4o".to_owned());
        let model_name = entries
            .model_name
            .as_set()
            .cloned()
            .or_else(|| {
                source_model_index
                    .and_then(|index| candidate.models.get(index))
                    .map(|model| model.name.clone())
            })
            .unwrap_or_else(|| model_id.clone());

        if update_model {
            let max_tokens = match &entries.max_output_tokens {
                ExplicitEntry::Set(value) => Some(*value),
                ExplicitEntry::Clear => None,
                ExplicitEntry::Unchanged => Some(DEFAULT_MODEL_MAX_OUTPUT_TOKENS),
            };
            let context_window = match &entries.context_window {
                ExplicitEntry::Set(value) => *value,
                _ => DEFAULT_MODEL_CONTEXT_WINDOW,
            };
            let vision = entries.supports_vision.as_set().copied().unwrap_or(true);
            let embedding = entries
                .supports_embedding
                .as_set()
                .copied()
                .unwrap_or(false);
            let dimension = entries.embedding_dimension.as_set().copied().unwrap_or(0);
            if let Some(index) = source_model_index {
                let model = &mut candidate.models[index];
                if entries.model_id.is_changed() {
                    model.id = model_id.clone();
                }
                if entries.model_name.is_changed() {
                    model.name = model_name.clone();
                }
                if entries.model_provider.is_changed() {
                    model.provider = model_provider.clone();
                }
                if entries.context_window.is_changed() {
                    model.context_window = context_window;
                }
                if entries.max_output_tokens.is_changed() {
                    model.max_output_tokens = max_tokens;
                }
                if entries.supports_vision.is_changed() {
                    model.supports_vision = vision;
                }
                if entries.supports_embedding.is_changed() {
                    model.supports_embedding = embedding;
                }
                if entries.embedding_dimension.is_changed() {
                    model.embedding_dimension = dimension;
                }
                changed.push("model.updated".to_owned());
            } else {
                candidate.models.push(ModelConfig {
                    name: model_name.clone(),
                    id: model_id.clone(),
                    provider: model_provider.clone(),
                    context_window,
                    max_output_tokens: max_tokens,
                    supports_vision: vision,
                    supports_embedding: embedding,
                    embedding_dimension: dimension,
                    requires_reasoning_content_on_assistant_messages: false,
                });
                changed.push("model.added".to_owned());
            }
            if entries.model_id.is_changed()
                || entries.model_name.is_changed()
                || entries.model_provider.is_changed()
                || !model_exists
            {
                candidate.agent.default_model = model_name.clone();
                candidate.agent.fast_model = model_name.clone();
                changed.push("agent.default_model".to_owned());
                changed.push("agent.fast_model".to_owned());
            }
        } else if model_exists {
            changed.push("model.skipped".to_owned());
        }

        match &entries.agent_reasoning_effort {
            ExplicitEntry::Set(value) => {
                candidate.agent.reasoning_effort = value.clone();
                changed.push("agent.reasoning_effort".to_owned());
            }
            ExplicitEntry::Clear => {
                candidate.agent.reasoning_effort = DEFAULT_AGENT_REASONING_EFFORT.to_owned();
                changed.push("agent.reasoning_effort".to_owned());
            }
            ExplicitEntry::Unchanged => {}
        }
        let config_value_entries = entries
            .config_values
            .iter()
            .filter(|(key, _)| key.as_str() != "memory.embedding.enabled")
            .chain(
                entries
                    .config_values
                    .iter()
                    .filter(|(key, _)| key.as_str() == "memory.embedding.enabled"),
            );
        for (key, entry) in config_value_entries {
            if let ExplicitEntry::Set(value) = entry {
                set_config_value(&mut candidate, key, value)
                    .map_err(|error| format!("写入 configValue '{key}' 失败: {error:?}"))?;
                changed.push(format!("configValue.{key}"));
            }
        }
        let mut env_changes = Vec::new();
        match &entries.api_key {
            ExplicitEntry::Set(secret) => {
                let key = candidate
                    .providers
                    .iter()
                    .find(|provider| provider.name == target_provider)
                    .and_then(|provider| provider.api_key_env.clone())
                    .unwrap_or_else(|| "OPENAI_API_KEY".to_owned());
                env_changes.push(EnvKeyChange::Set(key, secret.clone()));
                changed.push("secret.updated".to_owned());
            }
            ExplicitEntry::Clear => {
                let key = candidate
                    .providers
                    .iter()
                    .find(|provider| provider.name == target_provider)
                    .and_then(|provider| provider.api_key_env.clone())
                    .unwrap_or_else(|| "OPENAI_API_KEY".to_owned());
                env_changes.push(EnvKeyChange::Clear(key));
                changed.push("secret.cleared".to_owned());
            }
            ExplicitEntry::Unchanged => {}
        }
        let effective_key = candidate
            .providers
            .iter()
            .find(|provider| provider.name == target_provider)
            .and_then(|provider| provider.api_key_env.clone())
            .unwrap_or_else(|| "OPENAI_API_KEY".to_owned());
        Ok(SetupPlan {
            candidate_config: candidate,
            env_changes,
            changed_summary: changed,
            effective_provider_name: target_provider,
            effective_model_id: model_id,
            effective_api_key_env: effective_key,
        })
    }
}
