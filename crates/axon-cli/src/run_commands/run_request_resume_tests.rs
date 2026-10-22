use axon_config::{AxonConfig, ModelConfig, ProviderConfig};

use super::super::{
    run_model_routing::select_run_model, run_session_store::RunSessionRecord,
    run_thread_store::PreparedRunSession,
};
use super::*;

#[test]
fn resume_missing_historical_model_uses_current_config_default() {
    let config = current_config();
    let previous = historical_session();
    let command = RunCommand {
        prompt: "继续处理".to_owned(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: true,
        session: Some(previous.record.session_id.clone()),
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    };

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();
    assert_eq!(effective.model, None);

    let selected = select_run_model(&effective, &config).unwrap();
    assert_eq!(selected.name, config.agent.default_model);
    assert_eq!(selected.provider, "current-provider");
}

#[test]
fn resume_prefers_current_config_model_over_historical_model() {
    let mut config = current_config();
    config.providers.push(ProviderConfig::new(
        "historical-provider",
        "openai-chatcompletions",
        "https://historical.invalid/v1",
    ));
    config.models.push(ModelConfig {
        name: "historical-model".to_owned(),
        id: "historical-model-id".to_owned(),
        provider: "historical-provider".to_owned(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    let previous = historical_session_with_model("historical-provider", "historical-model-id");
    let command = RunCommand {
        prompt: "继续处理".to_owned(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: true,
        session: Some(previous.record.session_id.clone()),
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    };

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();

    assert_eq!(effective.model, None);
    assert_eq!(
        select_run_model(&effective, &config).unwrap().name,
        "current-model"
    );
}

#[test]
fn resumed_run_uses_explicit_workspace_instead_of_recorded_workspace() {
    let first = axon_test_support::test_temp_dir();
    let second = axon_test_support::test_temp_dir();

    let resolved =
        super::resolve_effective_run_cwd(Some(second.path()), Some(first.path())).unwrap();

    assert_eq!(resolved, second.path().canonicalize().unwrap());
}

#[test]
fn resumed_run_reuses_recorded_workspace_without_explicit_override() {
    let workspace = axon_test_support::test_temp_dir();
    let canonical = workspace.path().canonicalize().unwrap();

    let resolved = super::resolve_effective_run_cwd(None, Some(&canonical)).unwrap();

    assert_eq!(resolved, canonical);
}

fn current_config() -> AxonConfig {
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "current-provider",
        "openai-chatcompletions",
        "https://provider.invalid/v1",
    ));
    config.models.push(ModelConfig {
        name: "current-model".to_owned(),
        id: "current-model-id".to_owned(),
        provider: "current-provider".to_owned(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    config.agent.default_model = "current-model".to_owned();
    config
}

fn historical_session() -> PreparedRunSession {
    historical_session_with_model("removed-provider", "removed-model")
}

fn historical_session_with_model(provider: &str, model: &str) -> PreparedRunSession {
    let record = RunSessionRecord {
        session_id: "historical-session".to_owned(),
        parent_session_id: None,
        created_at_ms: 1,
        startup_timeline: super::super::run_startup_snapshot::test_startup_timeline(),
        cwd: None,
        provider: provider.to_owned(),
        model: model.to_owned(),
        selected_skill_name: None,
        repo_context: None,
        session_context: None,
        durable_memory: None,
        prompt: "历史任务".to_owned(),
        local_images: Vec::new(),
        local_files: Vec::new(),
        response: "历史响应".to_owned(),
        assistant_part_id: "assistant:historical".to_owned(),
        usage: None,
        tool_receipts: Vec::new(),
        tool_calls: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
        failure: None,
        upstream_model: None,
        reasoning_effort: None,
        request_path: None,
        speed_mode: None,
        ttft_ms: None,
        generation_speed_tps: None,
        injected_context_messages: Vec::new(),
    };
    PreparedRunSession {
        record,
        model_history_json: "[]".to_owned(),
    }
}
