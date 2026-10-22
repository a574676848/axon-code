use axon_agent_runtime::{
    agent_model_slot_candidates, channel_metrics_key, model_routing_key,
    resolve_channel_selection_with_health, resolve_run_model_profile_id,
    select_default_run_model_from_config,
};
use axon_config::{AxonConfig, ModelConfig, ProviderConfig};
use axon_domain::{BreakerPolicy, ModelProfileId};

use super::run_channel_affinity::{with_affinity_cache, with_health_registry, with_metrics_router};
use crate::RunCommand;

/// `agent.fast_model` 配置槽位名；fast 子代理与后台 Goal Execution 共用该槽位。
pub(super) const FAST_MODEL_SLOT_NAME: &str = "agent.fast_model";

/// 渠道负载均衡后的模型选择结果。
#[derive(Debug, Clone)]
pub(crate) struct ChannelBalancedModel {
    /// 首选 model 配置。
    pub(crate) model: ModelConfig,
    /// 首选 provider 配置。
    pub(crate) provider: ProviderConfig,
    /// 完整失败重试链（model_config, provider_config）。
    /// 第 0 项 == (model, provider)；后续为失败回退。
    pub(crate) chain: Vec<(ModelConfig, ProviderConfig)>,
    /// 本次负载均衡使用的路由键（model.name 或 fallback 到 model.id）。
    /// 用于健康注册表与亲和缓存的键，保证 selection / failover 一致。
    pub(crate) routing_key: String,
}

/// 渠道负载均衡模型选择入口。
///
/// 在 `select_run_model` 选出能力匹配的 model 后，
/// 按 model.name 查找所有可服务的 provider，用 priority/weight/affinity 排序，
/// 并叠加渠道健康过滤（跳过熔断渠道，全熔断降级保底）。
pub(super) fn select_channel_balanced_model(
    command: &RunCommand,
    config: &AxonConfig,
    session_id: &str,
    session_selector: u64,
    now_ms: u64,
) -> Result<ChannelBalancedModel, String> {
    let selected = select_run_model(command, config)?;
    select_channel_balanced_model_for_selection(
        config,
        selected,
        session_id,
        session_selector,
        now_ms,
    )
}

pub(crate) fn select_channel_balanced_model_for_selection(
    config: &AxonConfig,
    selected: &ModelConfig,
    session_id: &str,
    session_selector: u64,
    now_ms: u64,
) -> Result<ChannelBalancedModel, String> {
    let routing_key = model_routing_key(selected).to_owned();
    let balancer = config.channel_balancer;

    // 从进程级亲和缓存查上次成功渠道，映射为 config.models 索引。
    let affinity = with_affinity_cache(|cache| {
        cache
            .get(&routing_key, session_id, now_ms)
            .and_then(|provider| affinity_index(config, &routing_key, provider))
    });

    let policy = BreakerPolicy {
        failure_threshold: balancer.failure_threshold,
        cooldown_ms: balancer.cooldown_ms,
    };

    // 健康过滤选择（生产路径）：跳过熔断渠道，全熔断时降级保底返回原始首选。
    let (_, _, chain) = with_health_registry(policy, |health| {
        resolve_channel_selection_with_health(
            config,
            &routing_key,
            session_selector,
            affinity,
            health,
            now_ms,
        )
        .map(|selection| {
            let chain: Vec<(ModelConfig, ProviderConfig)> = selection
                .chain
                .iter()
                .map(|(mc, pc)| ((*mc).clone(), (*pc).clone()))
                .collect();
            (
                selection.primary.clone(),
                selection.primary_provider.clone(),
                chain,
            )
        })
    })
    .ok_or_else(|| format!("模型没有可用的启用 Provider：{routing_key}"))?;
    let chain = if affinity.is_none() {
        reorder_new_session_chain(&routing_key, chain)
    } else {
        chain
    };
    let (model, provider) = chain
        .first()
        .cloned()
        .ok_or_else(|| format!("模型没有可用的启用 Provider：{routing_key}"))?;
    Ok(ChannelBalancedModel {
        model,
        provider,
        chain,
        routing_key,
    })
}

/// 新会话动态评分仅在同 priority 组内生效，避免突破静态优先级。
fn reorder_new_session_chain(
    routing_key: &str,
    mut chain: Vec<(ModelConfig, ProviderConfig)>,
) -> Vec<(ModelConfig, ProviderConfig)> {
    let mut start = 0;
    while start < chain.len() {
        let priority = chain[start].1.priority;
        let end = chain[start..]
            .iter()
            .position(|(_, provider)| provider.priority != priority)
            .map_or(chain.len(), |offset| start + offset);
        let mut group = chain[start..end].to_vec();
        with_metrics_router(|metrics| {
            group.sort_by(|left, right| {
                let left_key = channel_metrics_key(routing_key, &left.1.name);
                let right_key = channel_metrics_key(routing_key, &right.1.name);
                let left_score = metrics.compute_score(&left_key, 0).adjusted_score;
                let right_score = metrics.compute_score(&right_key, 0).adjusted_score;
                left_score
                    .partial_cmp(&right_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        });
        chain.splice(start..end, group);
        start = end;
    }
    chain
}

pub(super) fn select_channel_balanced_model_for_agent_slot(
    config: &AxonConfig,
    slot: &str,
    slot_name: &str,
    session_id: &str,
    session_selector: u64,
    now_ms: u64,
) -> Result<ChannelBalancedModel, String> {
    let candidates = agent_model_slot_candidates(config, slot);
    let (_, selected) = candidates
        .first()
        .ok_or_else(|| format!("{slot_name} 必须引用已配置模型名：{slot}"))?;
    select_channel_balanced_model_for_selection(
        config,
        selected,
        session_id,
        session_selector,
        now_ms,
    )
}

/// 将亲和 provider 名称映射为 config.models 索引。
///
/// 与 `resolve_channel_selection` 的候选索引规则一致（config.models 位置），
/// 保证亲和命中能正确提到链首。
fn affinity_index(config: &AxonConfig, routing_key: &str, provider: &str) -> Option<usize> {
    config.models.iter().enumerate().find_map(|(index, model)| {
        if config.model_is_enabled(model)
            && model_routing_key(model) == routing_key
            && model.provider == provider
        {
            Some(index)
        } else {
            None
        }
    })
}

pub(super) fn select_run_model<'a>(
    command: &RunCommand,
    config: &'a AxonConfig,
) -> Result<&'a ModelConfig, String> {
    if let Some(requested) = command.model.as_deref() {
        return config
            .models
            .iter()
            .filter(|model| config.model_is_enabled(model))
            .find(|model| model_routing_key(model) == requested)
            .or_else(|| {
                config
                    .models
                    .iter()
                    .filter(|model| config.model_is_enabled(model))
                    .find(|model| model.id == requested)
            })
            .ok_or_else(|| format!("未找到可用于 axon run 的模型：{requested}"));
    }

    let selection = select_default_run_model_from_config(config)?;
    config
        .models
        .get(selection.config_index)
        .ok_or_else(|| "agent.default_model 无法映射到配置目录".to_owned())
}

/// 根据 provider + model 字符串解析 ModelProfileId。
///
/// 使用 runtime run-model candidate 顺序（候选列表中的位置 + 1），
/// 保证 `session/set_model` 能正确映射到运行时路由使用的 ModelProfileId。
pub(crate) fn resolve_model_profile_id(
    config: &AxonConfig,
    provider: &str,
    model: &str,
) -> Option<ModelProfileId> {
    resolve_run_model_profile_id(config, provider, model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputFormat;
    use axon_config::ProviderConfig;

    #[test]
    fn select_run_model_accepts_model_alias_name() {
        let command = RunCommand {
            prompt: "解释错误".to_owned(),
            model: Some("gpt-5.5".to_owned()),
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let mut config = AxonConfig::default();
        config.providers.push(ProviderConfig::new(
            "p-a",
            "openai",
            "https://provider.invalid/v1",
        ));
        config.models.push(ModelConfig {
            name: "gpt-5.5".to_owned(),
            id: "upstream-gpt-5.5".to_owned(),
            provider: "p-a".to_owned(),
            context_window: 128_000,
            ..ModelConfig::default()
        });

        let selected = select_run_model(&command, &config).unwrap();

        assert_eq!(selected.name, "gpt-5.5");
        assert_eq!(selected.id, "upstream-gpt-5.5");
    }

    #[test]
    fn channel_selection_keeps_selected_alias_when_upstream_id_is_shared() {
        let command = RunCommand {
            prompt: "解释错误".to_owned(),
            model: Some("alias-b".to_owned()),
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let mut config = AxonConfig::default();
        config.providers.push(ProviderConfig::new(
            "p-a",
            "openai",
            "https://provider.invalid/v1",
        ));
        config.models.extend([
            ModelConfig {
                name: "alias-a".to_owned(),
                id: "shared-upstream-id".to_owned(),
                provider: "p-a".to_owned(),
                context_window: 128_000,
                ..ModelConfig::default()
            },
            ModelConfig {
                name: "alias-b".to_owned(),
                id: "shared-upstream-id".to_owned(),
                provider: "p-a".to_owned(),
                context_window: 64_000,
                ..ModelConfig::default()
            },
        ]);

        let selected =
            select_channel_balanced_model(&command, &config, "session-alias-b", 0, 1).unwrap();

        assert_eq!(selected.model.name, "alias-b");
        assert_eq!(selected.model.id, "shared-upstream-id");
        assert_eq!(selected.model.context_window, 64_000);
    }

    #[test]
    fn select_run_model_rejects_provider_model_selector() {
        let command = RunCommand {
            prompt: "解释错误".to_owned(),
            model: Some("p-a/upstream-gpt-5.5".to_owned()),
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let mut config = AxonConfig::default();
        config.providers.push(ProviderConfig::new(
            "p-a",
            "openai",
            "https://provider.invalid/v1",
        ));
        config.models.push(ModelConfig {
            name: "gpt-5.5".to_owned(),
            id: "upstream-gpt-5.5".to_owned(),
            provider: "p-a".to_owned(),
            context_window: 128_000,
            ..ModelConfig::default()
        });

        let error = select_run_model(&command, &config).unwrap_err();

        assert_eq!(error, "未找到可用于 axon run 的模型：p-a/upstream-gpt-5.5");
    }

    #[test]
    fn select_run_model_rejects_model_from_disabled_provider() {
        let command = RunCommand {
            prompt: "解释错误".to_owned(),
            model: Some("gpt-disabled".to_owned()),
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let mut provider =
            ProviderConfig::new("p-disabled", "openai", "https://provider.invalid/v1");
        provider.enabled = false;
        let mut config = AxonConfig::default();
        config.providers.push(provider);
        config.models.push(ModelConfig {
            name: "gpt-disabled".to_owned(),
            id: "upstream-disabled".to_owned(),
            provider: "p-disabled".to_owned(),
            context_window: 128_000,
            ..ModelConfig::default()
        });

        let error = select_run_model(&command, &config).unwrap_err();

        assert_eq!(error, "未找到可用于 axon run 的模型：gpt-disabled");
    }
}
