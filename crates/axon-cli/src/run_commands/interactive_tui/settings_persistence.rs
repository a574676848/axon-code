//! Agent TUI 设置持久化到当前项目配置。

use std::path::Path;

use axon_config::{AxonConfig, ConfigLoader};
use axon_tui_runtime::AgentTuiSettingsChange;

/// 将 TUI 中最后选择的模型运行设置写回配置文件。
pub(super) fn persist_tui_settings_change(
    config_path: &Path,
    change: &AgentTuiSettingsChange,
) -> Result<(), String> {
    let mut loaded = ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取配置失败：{error:?}"))?;
    apply_tui_settings_change(&mut loaded.config, change);
    ConfigLoader::save_to_path(config_path, &loaded.config)
        .map_err(|error| format!("写入配置失败：{error:?}"))
}

/// 在 TUI 提交进入 Daemon 前同步必要的模型设置，避免配置写回回调尚未执行时使用旧模型。
pub(super) fn persist_tui_settings_change_if_needed(
    config_path: &Path,
    change: &AgentTuiSettingsChange,
) -> Result<(), String> {
    let loaded = ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取配置失败：{error:?}"))?;
    if !tui_settings_change_requires_persist(&loaded.config, change) {
        return Ok(());
    }
    persist_tui_settings_change(config_path, change)
}

fn tui_settings_change_requires_persist(
    config: &AxonConfig,
    change: &AgentTuiSettingsChange,
) -> bool {
    let model_changed = change
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|model| config.agent.default_model != model);
    let reasoning_changed = change
        .reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|effort| config.agent.reasoning_effort != effort);
    model_changed || reasoning_changed || !change.skill_states.is_empty()
}

fn apply_tui_settings_change(config: &mut AxonConfig, change: &AgentTuiSettingsChange) {
    if let Some(model_name) = change
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && let Some(index) = config
            .models
            .iter()
            .position(|model| config.model_is_enabled(model) && model.name == model_name)
            .or_else(|| {
                config
                    .models
                    .iter()
                    .position(|model| config.model_is_enabled(model) && model.id == model_name)
            })
    {
        config.agent.default_model = config.models[index].name.clone();
        if let Some(reasoning_effort) =
            normalized_reasoning_effort(change.reasoning_effort.as_deref())
        {
            config.agent.reasoning_effort = reasoning_effort;
        }
    }
    for skill in &change.skill_states {
        if skill.enabled {
            config.skills.disabled.remove(&skill.name);
        } else {
            config.skills.disabled.insert(skill.name.clone());
        }
    }
}

fn normalized_reasoning_effort(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use axon_config::{AxonConfig, ModelConfig, ProviderConfig};
    use axon_tui_runtime::AgentTuiSettingsChange;

    use super::apply_tui_settings_change;

    #[test]
    fn model_change_updates_default_model_and_agent_reasoning_effort() {
        let mut config = fixture_config();
        let change = AgentTuiSettingsChange {
            model: Some("upstream-fast".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            skill_states: Vec::new(),
        };

        apply_tui_settings_change(&mut config, &change);

        assert_eq!(config.agent.default_model, "fast");
        assert_eq!(config.agent.reasoning_effort, "high");
    }

    #[test]
    fn model_change_prefers_alias_name_over_duplicate_model_id() {
        let mut config = fixture_config();
        config
            .models
            .insert(0, model("alias-a", "alias-b", "local"));
        config
            .models
            .push(model("alias-b", "shared-upstream-id", "local"));
        let change = AgentTuiSettingsChange {
            model: Some("alias-b".to_owned()),
            reasoning_effort: None,
            skill_states: Vec::new(),
        };

        apply_tui_settings_change(&mut config, &change);

        assert_eq!(config.agent.default_model, "alias-b");
        assert!(config.models.iter().any(|model| {
            model.name == config.agent.default_model && model.id == "shared-upstream-id"
        }));
    }

    #[test]
    fn model_name_change_reuses_channel_balancer_route_key() {
        let mut config = fixture_config();
        config.agent.default_model = "balanced".to_owned();
        let change = AgentTuiSettingsChange {
            model: Some("balanced".to_owned()),
            reasoning_effort: Some("xhigh".to_owned()),
            skill_states: Vec::new(),
        };

        apply_tui_settings_change(&mut config, &change);

        assert_eq!(config.agent.default_model, "balanced");
        assert_eq!(config.agent.reasoning_effort, "xhigh");
    }

    #[test]
    fn model_change_ignores_disabled_provider_model() {
        let mut config = fixture_config();
        config.agent.default_model = "balanced".to_owned();
        let previous_reasoning_effort = config.agent.reasoning_effort.clone();
        config
            .providers
            .iter_mut()
            .find(|provider| provider.name == "remote")
            .expect("测试 Provider 应存在")
            .enabled = false;
        let change = AgentTuiSettingsChange {
            model: Some("upstream-fast".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            skill_states: Vec::new(),
        };

        apply_tui_settings_change(&mut config, &change);

        assert_eq!(config.agent.default_model, "balanced");
        assert_eq!(config.agent.reasoning_effort, previous_reasoning_effort);
    }

    #[test]
    fn skill_state_change_updates_disabled_skills() {
        let mut config = fixture_config();
        config.skills.disabled.insert("imagegen".to_owned());
        let change = AgentTuiSettingsChange {
            model: None,
            reasoning_effort: None,
            skill_states: vec![
                axon_tui_runtime::AgentTuiSkillStateChange {
                    name: "review".to_owned(),
                    enabled: false,
                },
                axon_tui_runtime::AgentTuiSkillStateChange {
                    name: "imagegen".to_owned(),
                    enabled: true,
                },
            ],
        };

        apply_tui_settings_change(&mut config, &change);

        assert!(config.skills.disabled.contains("review"));
        assert!(!config.skills.disabled.contains("imagegen"));
    }

    fn fixture_config() -> AxonConfig {
        let mut config = AxonConfig::default();
        config.providers.push(ProviderConfig::new(
            "local",
            "openai",
            "http://local.invalid",
        ));
        config.providers.push(ProviderConfig::new(
            "remote",
            "openai",
            "http://remote.invalid",
        ));
        config
            .models
            .push(model("balanced", "local-balanced", "local"));
        config
            .models
            .push(model("balanced", "remote-balanced", "remote"));
        config.models.push(model("fast", "upstream-fast", "remote"));
        config
    }

    fn model(name: &str, id: &str, provider: &str) -> ModelConfig {
        ModelConfig {
            name: name.to_owned(),
            id: id.to_owned(),
            provider: provider.to_owned(),
            ..ModelConfig::default()
        }
    }
}
