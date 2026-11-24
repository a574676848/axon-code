//! 配置投影：从 `config.toml` 算出 TUI 可切换的模型清单与两个默认值。
//!
//! 单独成模块，是因为它与 catalog 的其它字段（skills、MCP、会话清单）来源不同：
//! 那些来自后台线程，这一份来自配置文件，并且要被启动与运行期复读两条路径共用。

use std::collections::BTreeSet;

use axon_tui_runtime::{AgentTuiConfigProjection, AgentTuiModelCatalogItem};

/// 把配置文件投影成 TUI 需要的模型清单与两个默认值。
///
/// 三条规则与 `/model` 的可选口径逐字一致：只看启用的供应商、排除向量模型、
/// 同名只留第一条（别名重复时以配置顺序在前的为准）。启动与后台复读共用这一份，
/// 因此 `config.toml` 改完之后 TUI 里的清单不会与刚启动时那套规则分叉。
pub(super) fn tui_config_projection(config: &axon_config::AxonConfig) -> AgentTuiConfigProjection {
    let mut seen_names = BTreeSet::new();
    let models = config
        .models
        .iter()
        .filter(|model| config.model_is_enabled(model))
        .filter(|model| !model.supports_embedding)
        .filter(|model| seen_names.insert(model.name.clone()))
        .map(|model| {
            AgentTuiModelCatalogItem::new_with_runtime_defaults(
                model.name.clone(),
                model.id.clone(),
                model.provider.clone(),
                (model.context_window > 0).then_some(model.context_window),
                None,
            )
            .with_supports_vision(model.supports_vision)
        })
        .collect::<Vec<_>>();
    let defaults = super::default_tui_model_selection(config);
    let default_reasoning_effort = config.agent.reasoning_effort.trim().to_owned();
    /*
     * 默认模型必须自己也在这份候选里：`agent.default_model` 允许指向向量模型
     * （校验只要求它引用已配置模型），那样回填出来的名字在 `/model` 清单里根本不存在。
     * 查不到就留空，让投影不覆盖界面现在的值，而不是把一个不可选的名字写进头部。
     */
    let default_model = defaults
        .map(|(model, _)| model)
        .filter(|model| models.iter().any(|candidate| candidate.label == *model));
    AgentTuiConfigProjection {
        models,
        providers: config
            .providers
            .iter()
            .filter(|provider| provider.enabled)
            .map(|provider| provider.name.clone())
            .collect(),
        default_model,
        default_reasoning_effort: (!default_reasoning_effort.is_empty())
            .then_some(default_reasoning_effort),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str, enabled: bool) -> axon_config::ProviderConfig {
        let mut provider = axon_config::ProviderConfig::new(
            name,
            axon_config::PROVIDER_PROTOCOL_OPENAI,
            "https://enabled.invalid/v1",
        );
        provider.enabled = enabled;
        provider
    }

    fn model(name: &str, provider: &str) -> axon_config::ModelConfig {
        axon_config::ModelConfig {
            name: name.to_owned(),
            id: name.to_owned(),
            provider: provider.to_owned(),
            ..axon_config::ModelConfig::default()
        }
    }

    #[test]
    fn config_projection_keeps_only_enabled_chat_models_and_their_defaults() {
        let mut config = axon_config::AxonConfig::default();
        config.providers.extend([
            provider("enabled-provider", true),
            provider("disabled-provider", false),
        ]);
        config.models.extend([
            model("enabled-model", "enabled-provider"),
            model("disabled-model", "disabled-provider"),
            axon_config::ModelConfig {
                supports_embedding: true,
                embedding_dimension: 1536,
                ..model("embedding-model", "enabled-provider")
            },
        ]);
        config.agent.default_model = "enabled-model".to_owned();
        config.agent.reasoning_effort = "high".to_owned();

        let projection = tui_config_projection(&config);

        assert_eq!(
            projection
                .models
                .iter()
                .map(|model| model.label.as_str())
                .collect::<Vec<_>>(),
            vec!["enabled-model"],
            "停用供应商的模型与向量模型都不进 /model 候选"
        );
        assert_eq!(projection.providers, vec!["enabled-provider".to_owned()]);
        assert_eq!(projection.default_model.as_deref(), Some("enabled-model"));
        assert_eq!(
            projection.default_reasoning_effort.as_deref(),
            Some("high"),
            "配置里的默认强度要随投影一起带出去，/think 才读得到当前值"
        );
    }

    #[test]
    fn config_projection_drops_a_default_model_outside_the_chat_catalog() {
        // 校验只要求 `agent.default_model` 引用已配置模型，因此它可以指向向量模型。
        let mut config = axon_config::AxonConfig::default();
        config.providers.push(provider("enabled-provider", true));
        config.models.push(axon_config::ModelConfig {
            supports_embedding: true,
            embedding_dimension: 1536,
            ..model("embedding-model", "enabled-provider")
        });
        config.agent.default_model = "embedding-model".to_owned();

        let projection = tui_config_projection(&config);

        assert!(projection.models.is_empty());
        assert_eq!(
            projection.default_model, None,
            "默认模型不在 /model 候选里时留空，不能把一个选不到的名字回填进头部"
        );
    }
}
