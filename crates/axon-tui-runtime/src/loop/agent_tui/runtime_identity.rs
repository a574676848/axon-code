//! Agent TUI 运行身份解析。

use super::{AgentTuiConfigStatus, AgentTuiModelCatalogItem, AgentTuiRunOverrides};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectiveModelIdentity {
    pub(crate) provider: String,
    pub(crate) model: String,
}

pub(crate) fn provider_model_route(provider: &str, model: &str) -> String {
    let provider = provider.trim();
    let model = model.trim();
    match (provider.is_empty(), model.is_empty()) {
        (false, false) if model.starts_with(&format!("{provider}/")) => model.to_owned(),
        (false, false) => format!("{provider}/{model}"),
        (true, false) => model.to_owned(),
        (false, true) => provider.to_owned(),
        (true, true) => String::new(),
    }
}

pub(crate) fn effective_model(
    config_status: &AgentTuiConfigStatus,
    overrides: &AgentTuiRunOverrides,
) -> String {
    overrides
        .model
        .clone()
        .unwrap_or_else(|| config_status.model.clone())
}

pub(crate) fn display_model(
    config_status: &AgentTuiConfigStatus,
    overrides: &AgentTuiRunOverrides,
) -> String {
    model_name_only(&effective_model(config_status, overrides))
}

pub(crate) fn effective_model_identity(
    config_status: &AgentTuiConfigStatus,
    overrides: &AgentTuiRunOverrides,
    models: &[AgentTuiModelCatalogItem],
) -> EffectiveModelIdentity {
    let configured_model = effective_model(config_status, overrides);
    let display_model = model_name_only(&configured_model);
    let selected_model = models
        .iter()
        .find(|candidate| candidate.label == display_model)
        .or_else(|| {
            models
                .iter()
                .find(|candidate| candidate.matches(&configured_model))
        });
    EffectiveModelIdentity {
        provider: selected_model
            .map(|candidate| candidate.provider.clone())
            .unwrap_or_else(|| config_status.provider.clone()),
        model: selected_model
            .map(|candidate| candidate.label.clone())
            .unwrap_or(display_model),
    }
}

pub(crate) fn model_name_only(value: &str) -> String {
    value
        .rsplit('/')
        .next()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(value.trim())
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_catalog_model_controls_provider_and_label() {
        let config_status = AgentTuiConfigStatus::new("sm-wenwen", "gpt-5.6-sol", "已认证");
        let overrides = AgentTuiRunOverrides {
            model: Some("gemini-3.6-flash-high".to_owned()),
            ..AgentTuiRunOverrides::default()
        };
        let models = vec![AgentTuiModelCatalogItem::new(
            "gemini-3.6-flash-high",
            "gemini-3.6-flash-high",
            "cpa",
        )];

        let identity = effective_model_identity(&config_status, &overrides, &models);

        assert_eq!(identity.provider, "cpa");
        assert_eq!(identity.model, "gemini-3.6-flash-high");
    }

    #[test]
    fn selected_alias_wins_when_selector_matches_another_alias() {
        let config_status = AgentTuiConfigStatus::new("provider", "alias-b", "已认证");
        let models = vec![
            AgentTuiModelCatalogItem::new("alias-a", "shared", "provider"),
            AgentTuiModelCatalogItem::new("alias-b", "shared", "provider"),
        ];

        let identity =
            effective_model_identity(&config_status, &AgentTuiRunOverrides::default(), &models);

        assert_eq!(identity.model, "alias-b");
    }
}
