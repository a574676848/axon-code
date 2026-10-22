use super::setup_interactive::InteractiveSetupInput;
use super::setup_interactive_config::AgentSetupConfig;
use super::setup_plan_entries::collect_explicit_entries;
use crate::SetupCommand;
use axon_config::AxonConfig;
use axon_config::setup::{EnvKeyChange, SetupPlanner, SetupPlannerOptions, SetupSource};
pub(crate) use axon_config::setup::{ExplicitEntry, SetupActions, SetupExplicitEntries};

pub(crate) struct SetupPlanResult {
    pub candidate_config: AxonConfig,
    pub env_changes: Vec<EnvKeyChange>,
    pub changed_summary: Vec<String>,
    pub effective_provider_name: String,
    pub effective_model_id: String,
    pub effective_api_key_env: String,
}

pub(crate) fn build_setup_plan(
    command: &SetupCommand,
    agent_cfg: &AgentSetupConfig,
    interactive_input: &InteractiveSetupInput,
    current_config: &AxonConfig,
) -> Result<SetupPlanResult, String> {
    let actions = SetupActions {
        reset_provider: command
            .reset_provider
            .clone()
            .or_else(|| interactive_input.reset_provider.clone()),
        reset_agent_models: command.reset_agent_models || interactive_input.reset_agent_models,
    };
    let entries = collect_explicit_entries(command, agent_cfg, interactive_input)?;
    if !entries.has_mutation(&actions) {
        let provider_name = interactive_input
            .selected_provider_name
            .clone()
            .or_else(|| {
                current_config
                    .providers
                    .first()
                    .map(|provider| provider.name.clone())
            })
            .unwrap_or_else(|| "openai".to_owned());
        let model_id = current_config
            .models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_else(|| "gpt-4o".to_owned());
        let api_key_env = current_config
            .providers
            .iter()
            .find(|provider| provider.name == provider_name)
            .and_then(|provider| provider.api_key_env.clone())
            .unwrap_or_else(|| "OPENAI_API_KEY".to_owned());
        return Ok(SetupPlanResult {
            candidate_config: current_config.clone(),
            env_changes: Vec::new(),
            changed_summary: Vec::new(),
            effective_provider_name: provider_name,
            effective_model_id: model_id,
            effective_api_key_env: api_key_env,
        });
    }
    let source = if interactive_input.interactive {
        SetupSource::Interactive
    } else if command.agent_config.is_some() {
        SetupSource::Agent
    } else {
        SetupSource::Cli
    };
    let intent = axon_config::setup::SetupIntent::from_explicit_entries(
        &entries,
        actions,
        source,
        current_config,
    );
    let plan = SetupPlanner
        .plan_explicit(
            current_config,
            &intent,
            &SetupPlannerOptions {
                entries: entries.clone(),
                quick: command.quick,
                selected_provider_name: interactive_input.selected_provider_name.clone(),
                selected_model_id: interactive_input.selected_model_id.clone(),
                selected_model_provider: interactive_input.selected_model_provider.clone(),
            },
        )
        .map_err(|error| format!("配置意图无效: {error}"))?;
    Ok(SetupPlanResult {
        candidate_config: plan.candidate_config,
        env_changes: plan.env_changes,
        changed_summary: plan.changed_summary,
        effective_provider_name: plan.effective_provider_name,
        effective_model_id: plan.effective_model_id,
        effective_api_key_env: plan.effective_api_key_env,
    })
}
