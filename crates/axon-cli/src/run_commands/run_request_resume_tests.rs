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
    let command = resume_command(None);

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();
    assert_eq!(effective.model, None);

    let selected = select_run_model(&effective, &config).unwrap();
    assert_eq!(selected.name, config.agent.default_model);
    assert_eq!(selected.provider, "current-provider");
}

#[test]
fn resume_keeps_historical_model_when_it_still_exists() {
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
        context_window: 1_000_000,
        ..ModelConfig::default()
    });
    let previous = historical_session_with_model("historical-provider", "historical-model-id");
    let command = resume_command(None);

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();

    assert_eq!(effective.model.as_deref(), Some("historical-model"));
    let selected = select_run_model(&effective, &config).unwrap();
    assert_eq!(selected.name, "historical-model");
    assert_eq!(selected.provider, "historical-provider");
    // 运行侧读的就是这一份 ModelConfig：预算与压缩阈值都取它的 context_window，
    // 所以「模型是会话的、窗口是默认模型的」只会出现在显示层，不会出现在执行层。
    assert_eq!(
        selected.context_window, 1_000_000,
        "恢复会话后运行侧必须用会话模型的窗口"
    );
}

#[test]
fn resume_keeps_historical_model_when_provider_was_renamed() {
    let mut config = current_config();
    config.models.push(ModelConfig {
        name: "moved-model".to_owned(),
        id: "historical-model-id".to_owned(),
        provider: "current-provider".to_owned(),
        context_window: 200_000,
        ..ModelConfig::default()
    });
    let previous = historical_session_with_model("removed-provider", "historical-model-id");
    let command = resume_command(None);

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();

    assert_eq!(effective.model.as_deref(), Some("moved-model"));
    assert_eq!(
        select_run_model(&effective, &config).unwrap().provider,
        "current-provider"
    );
}

#[test]
fn resume_explicit_model_overrides_historical_model() {
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
    let mut command = resume_command(None);
    command.model = Some("current-model".to_owned());

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();

    assert_eq!(effective.model.as_deref(), Some("current-model"));
}

#[test]
fn resume_prefers_recorded_alias_when_one_id_has_many_aliases() {
    let mut config = current_config();
    // 同一 provider 与 upstream id 在配置里对应两个别名（先命中 builtin 的 fast_model）。
    config.models.push(ModelConfig {
        name: "fast_model".to_owned(),
        id: "shared-model-id".to_owned(),
        provider: "current-provider".to_owned(),
        context_window: 100_000,
        ..ModelConfig::default()
    });
    config.models.push(ModelConfig {
        name: "deepseek-v4.1-flash".to_owned(),
        id: "shared-model-id".to_owned(),
        provider: "current-provider".to_owned(),
        context_window: 400_000,
        ..ModelConfig::default()
    });
    let previous = historical_session_with_alias(
        "current-provider",
        "shared-model-id",
        Some("deepseek-v4.1-flash"),
    );
    let command = resume_command(None);

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();

    assert_eq!(effective.model.as_deref(), Some("deepseek-v4.1-flash"));
    let selected = select_run_model(&effective, &config).unwrap();
    // 别名带回的不只是名字：上下文窗口等参数也必须取用户当初选中的那一条。
    assert_eq!(selected.context_window, 400_000);
}

#[test]
fn resume_without_recorded_alias_falls_back_to_same_provider_id() {
    let mut config = current_config();
    config.models.push(ModelConfig {
        name: "fast_model".to_owned(),
        id: "shared-model-id".to_owned(),
        provider: "current-provider".to_owned(),
        context_window: 100_000,
        ..ModelConfig::default()
    });
    // 旧记录没有别名：按同 provider 同 id 匹配，取配置里第一条。
    let previous = historical_session_with_alias("current-provider", "shared-model-id", None);
    let command = resume_command(None);

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();

    assert_eq!(effective.model.as_deref(), Some("fast_model"));
}

#[test]
fn resume_ignores_alias_removed_from_config() {
    let mut config = current_config();
    config.models.push(ModelConfig {
        name: "fast_model".to_owned(),
        id: "shared-model-id".to_owned(),
        provider: "current-provider".to_owned(),
        ..ModelConfig::default()
    });
    let previous =
        historical_session_with_alias("current-provider", "shared-model-id", Some("removed-alias"));
    let command = resume_command(None);

    let effective = resumed_command(&command, Some(&previous), &config).unwrap();

    assert_eq!(effective.model.as_deref(), Some("fast_model"));
}

fn resume_command(model: Option<&str>) -> RunCommand {
    RunCommand {
        prompt: "继续处理".to_owned(),
        model: model.map(ToOwned::to_owned),
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: true,
        session: Some("historical-session".to_owned()),
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    }
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
    historical_session_with_alias(provider, model, Some(model))
}

fn historical_session_with_alias(
    provider: &str,
    model: &str,
    alias: Option<&str>,
) -> PreparedRunSession {
    let record = RunSessionRecord {
        session_id: "historical-session".to_owned(),
        parent_session_id: None,
        created_at_ms: 1,
        startup_timeline: super::super::run_startup_snapshot::test_startup_timeline(),
        cwd: None,
        provider: provider.to_owned(),
        model: model.to_owned(),
        model_name: alias.map(ToOwned::to_owned),
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
        reflection_notice: None,
        turn_trace: None,
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
