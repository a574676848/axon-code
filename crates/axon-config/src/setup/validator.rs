use crate::AxonConfig;

use super::ExplicitEntry;

pub(super) fn validate_env_reference(value: &str) -> Result<(), String> {
    let valid = !value.trim().is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    valid
        .then_some(())
        .ok_or_else(|| "密钥环境变量名只能包含字母、数字和下划线".to_owned())
}

pub(super) fn selected_provider<'a>(
    config: &'a AxonConfig,
    model_ref: &ExplicitEntry<String>,
) -> Option<&'a str> {
    if let ExplicitEntry::Set(model_ref) = model_ref
        && let Some(model) = config
            .models
            .iter()
            .find(|model| model.id == *model_ref || model.name == *model_ref)
    {
        return Some(model.provider.as_str());
    }
    config
        .providers
        .first()
        .map(|provider| provider.name.as_str())
}

pub(super) fn agent_model_slots_reference_missing(config: &AxonConfig) -> bool {
    [&config.agent.default_model, &config.agent.fast_model]
        .into_iter()
        .any(|slot| {
            !slot.is_empty()
                && !config
                    .models
                    .iter()
                    .any(|model| model.name == *slot || model.id == *slot)
        })
}
