use crate::AxonConfig;

use super::*;

#[test]
fn planner_changes_only_novice_choices() {
    let mut config = AxonConfig::default();
    config.providers.push(crate::ProviderConfig::new(
        "openai",
        "openai",
        "https://api.openai.com/v1",
    ));
    config.models.push(crate::ModelConfig {
        name: "主模型".to_owned(),
        id: "gpt-main".to_owned(),
        provider: "openai".to_owned(),
        context_window: crate::DEFAULT_MODEL_CONTEXT_WINDOW,
        max_output_tokens: Some(crate::DEFAULT_MODEL_MAX_OUTPUT_TOKENS),
        supports_vision: true,
        supports_embedding: false,
        embedding_dimension: 0,
        requires_reasoning_content_on_assistant_messages: false,
    });
    let plan = SetupPlanner
        .plan(
            &config,
            &SetupIntent {
                default_model: ExplicitEntry::Set("gpt-main".to_owned()),
                credential_env: ExplicitEntry::Set("OPENAI_API_KEY".to_owned()),
                ..SetupIntent::default()
            },
        )
        .unwrap();
    assert_eq!(plan.candidate_config.agent.default_model, "主模型");
    assert_eq!(
        plan.candidate_config.providers[0].api_key_env.as_deref(),
        Some("OPENAI_API_KEY")
    );
}

#[test]
fn planner_rejects_unknown_model_and_bad_env_reference() {
    let config = AxonConfig::default();
    let err = SetupPlanner
        .plan(
            &config,
            &SetupIntent {
                default_model: ExplicitEntry::Set("missing".to_owned()),
                ..SetupIntent::default()
            },
        )
        .unwrap_err();
    assert!(err.contains("找不到模型"));
    let err = SetupPlanner
        .plan(
            &config,
            &SetupIntent {
                credential_env: ExplicitEntry::Set("api-key".to_owned()),
                ..SetupIntent::default()
            },
        )
        .unwrap_err();
    assert!(err.contains("环境变量名"));
}
