//! 固定主模型选择辅助。
//!
//! 主 Agent 模型不再按 prompt、tier 或关键词智能路由；本模块只负责把
//! `[agent].default_model` 映射到配置目录，并为 TUI 保留稳定 profile id。

use axon_config::{AxonConfig, ModelConfig};
use axon_domain::ModelProfileId;

/// `axon run`/composer 共享的模型选择结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunModelSelection {
    /// 选中的 config.models 索引。
    pub config_index: usize,
    /// 选中的 runtime 模型画像 ID。
    pub profile_id: ModelProfileId,
}

/// 按 agent model slot 取候选模型配置索引。
#[must_use]
pub fn agent_model_slot_candidates<'a>(
    config: &'a AxonConfig,
    slot: &str,
) -> Vec<(usize, &'a ModelConfig)> {
    let mut candidates = config
        .models
        .iter()
        .enumerate()
        .filter(|(_, model)| model.name == slot && config.model_is_enabled(model))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        candidates = config
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| model.id == slot && config.model_is_enabled(model))
            .collect();
    }
    candidates
}

/// 按 agent.default_model 取主 Agent 候选模型。
#[must_use]
pub fn default_run_model_candidates(config: &AxonConfig) -> Vec<(usize, &ModelConfig)> {
    agent_model_slot_candidates(config, config.agent.default_model.as_str())
}

/// 选择 run/composer 默认模型，返回可映射回 config 的选择。
pub fn select_default_run_model_from_config(
    config: &AxonConfig,
) -> Result<RunModelSelection, String> {
    let candidates = default_run_model_candidates(config);
    let (config_index, _) = candidates.first().ok_or_else(|| {
        format!(
            "agent.default_model 必须引用已配置模型名：{}",
            config.agent.default_model
        )
    })?;
    Ok(RunModelSelection {
        config_index: *config_index,
        profile_id: model_profile_id_for_candidate_index(0),
    })
}

/// 根据 provider + model 字符串解析 run/composer 使用的 ModelProfileId。
#[must_use]
pub fn resolve_run_model_profile_id(
    config: &AxonConfig,
    provider: &str,
    model: &str,
) -> Option<ModelProfileId> {
    config
        .models
        .iter()
        .enumerate()
        .find(|(_, mc)| mc.provider == provider && config.model_is_enabled(mc) && mc.name == model)
        .or_else(|| {
            config.models.iter().enumerate().find(|(_, mc)| {
                mc.provider == provider && config.model_is_enabled(mc) && mc.id == model
            })
        })
        .map(|(config_index, _)| model_profile_id_for_candidate_index(config_index))
}

fn model_profile_id_for_candidate_index(index: usize) -> ModelProfileId {
    ModelProfileId((index + 1) as u64)
}

#[cfg(test)]
mod tests {
    use axon_config::{AxonConfig, ModelConfig, ProviderConfig};

    use super::*;

    #[test]
    fn select_default_run_model_uses_agent_default_model() {
        let mut config = AxonConfig {
            providers: vec![provider("p-a"), provider("p-b")],
            models: vec![model("tiny", "p-a"), model("balanced", "p-b")],
            ..AxonConfig::default()
        };
        config.agent.default_model = "balanced".to_owned();
        config.agent.fast_model = "tiny".to_owned();

        let selection = select_default_run_model_from_config(&config).unwrap();

        assert_eq!(selection.config_index, 1);
        assert_eq!(selection.profile_id, ModelProfileId(1));
    }

    #[test]
    fn resolve_run_model_profile_id_uses_config_order() {
        let config = AxonConfig {
            providers: vec![provider("p-a"), provider("p-b")],
            models: vec![model("tiny", "p-a"), model("balanced", "p-b")],
            ..AxonConfig::default()
        };

        assert_eq!(
            resolve_run_model_profile_id(&config, "p-b", "balanced"),
            Some(ModelProfileId(2))
        );
        assert_eq!(
            resolve_run_model_profile_id(&config, "p-c", "balanced"),
            None
        );
    }

    #[test]
    fn resolve_run_model_profile_id_prefers_alias_for_duplicate_provider_model_id() {
        let config = AxonConfig {
            providers: vec![provider("p-a")],
            models: vec![
                ModelConfig {
                    name: "alias-a".to_owned(),
                    id: "upstream".to_owned(),
                    provider: "p-a".to_owned(),
                    context_window: 128_000,
                    ..ModelConfig::default()
                },
                ModelConfig {
                    name: "alias-b".to_owned(),
                    id: "upstream".to_owned(),
                    provider: "p-a".to_owned(),
                    context_window: 128_000,
                    ..ModelConfig::default()
                },
            ],
            ..AxonConfig::default()
        };

        assert_eq!(
            resolve_run_model_profile_id(&config, "p-a", "alias-b"),
            Some(ModelProfileId(2))
        );
    }

    #[test]
    fn agent_model_slot_candidates_prefers_alias_over_duplicate_id() {
        let config = AxonConfig {
            providers: vec![provider("p-a")],
            models: vec![
                ModelConfig {
                    name: "alias-a".to_owned(),
                    id: "alias-b".to_owned(),
                    provider: "p-a".to_owned(),
                    context_window: 128_000,
                    ..ModelConfig::default()
                },
                ModelConfig {
                    name: "alias-b".to_owned(),
                    id: "shared".to_owned(),
                    provider: "p-a".to_owned(),
                    context_window: 128_000,
                    ..ModelConfig::default()
                },
            ],
            ..AxonConfig::default()
        };

        let candidates = agent_model_slot_candidates(&config, "alias-b");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].1.name, "alias-b");
    }

    #[test]
    fn disabled_provider_models_are_not_default_or_profile_candidates() {
        let mut disabled = provider("p-disabled");
        disabled.enabled = false;
        let mut config = AxonConfig {
            providers: vec![disabled, provider("p-enabled")],
            models: vec![
                model("balanced", "p-disabled"),
                model("balanced", "p-enabled"),
            ],
            ..AxonConfig::default()
        };
        config.agent.default_model = "balanced".to_owned();

        let selection = select_default_run_model_from_config(&config).unwrap();

        assert_eq!(selection.config_index, 1);
        assert_eq!(
            resolve_run_model_profile_id(&config, "p-disabled", "balanced"),
            None
        );
    }

    fn provider(name: &str) -> ProviderConfig {
        ProviderConfig::new(name, "openai", "https://provider.invalid/v1")
    }

    fn model(id: &str, provider: &str) -> ModelConfig {
        ModelConfig {
            name: id.to_owned(),
            id: id.to_owned(),
            provider: provider.to_owned(),
            context_window: 128_000,
            ..ModelConfig::default()
        }
    }
}
