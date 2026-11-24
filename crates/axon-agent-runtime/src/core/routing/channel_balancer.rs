//! 渠道负载均衡 runtime。
//!
//! 桥接 `ModelChannelSelection`（纯领域）与 `AxonConfig`（配置），
//! 将同一模型名称下多个 provider 的 priority/weight 映射为候选渠道，
//! 输出带优先级、权重、亲和与失败重试链的渠道选择方案。
//!
//! 模型名称（model.name）是负载均衡的路由键，
//! 模型 ID（model.id）是实际发送给上游渠道的参数。

use std::collections::HashMap;

use axon_config::{AxonConfig, ModelConfig, ProviderConfig};
use axon_domain::{ModelChannelCandidate, ModelChannelPlan, ModelChannelSelection};

use crate::channel_health::{ChannelHealthRegistry, ChannelKey};

/// 已解析的渠道选择结果。
///
/// primary 为首选渠道，chain 为完整的失败重试链（含 primary）。
/// 顺序已按优先级、权重和亲和排好，调用方逐个尝试即可。
#[derive(Debug, Clone)]
pub struct ChannelSelection<'a> {
    /// 首选 model 配置。
    pub primary: &'a ModelConfig,
    /// 首选 provider 配置。
    pub primary_provider: &'a ProviderConfig,
    /// 按尝试顺序排列的 (model_config, provider_config) 链。
    /// 第 0 项 == primary，后续为失败回退。
    pub chain: Vec<(&'a ModelConfig, &'a ProviderConfig)>,
    /// 本次选择使用的 ModelChannelPlan（可审计）。
    pub plan: ModelChannelPlan,
}

/// 获取模型的负载均衡路由键。
///
/// 优先使用 model.name（非空时），fallback 到 model.id。
#[must_use]
pub fn model_routing_key(model: &ModelConfig) -> &str {
    if model.name.is_empty() {
        &model.id
    } else {
        &model.name
    }
}

/// 按模型名称（model.name）对 config.models 做渠道负载均衡。
///
/// 若模型名称仅有一个 provider 服务，返回单元素链（无负载均衡开销）。
/// 若多个 provider 服务同一模型名称，按 priority 分组，再在同组内按 weight/affinity
/// 生成尝试链。亲和不会把低优先级 provider 提升到高优先级组之前。
///
/// `selector` 为加权随机源（如 session_id 哈希），保证同 selector 结果可复现。
/// `affinity` 为上次成功渠道在 candidates 中的索引，仅在其 priority 组内生效。
#[must_use]
pub fn resolve_channel_selection<'a>(
    config: &'a AxonConfig,
    model_name: &str,
    selector: u64,
    affinity: Option<usize>,
) -> Option<ChannelSelection<'a>> {
    // 按路由键收集所有候选渠道。
    let indices: Vec<usize> = config
        .models
        .iter()
        .enumerate()
        .filter(|(_, model)| {
            model_routing_key(model) == model_name && config.model_is_enabled(model)
        })
        .map(|(index, _)| index)
        .collect();

    if indices.is_empty() {
        return None;
    }

    // 单 provider 直接返回，跳过负载均衡。
    if indices.len() == 1 {
        let index = indices[0];
        let model = &config.models[index];
        let provider = config.enabled_provider(&model.provider)?;
        return Some(ChannelSelection {
            primary: model,
            primary_provider: provider,
            chain: vec![(model, provider)],
            plan: ModelChannelPlan::default(),
        });
    }

    // 多 provider：映射为 domain 候选，调用领域选择器。
    let candidates: Vec<ModelChannelCandidate> = indices
        .iter()
        .filter_map(|&index| {
            let model = &config.models[index];
            let provider = config.enabled_provider(&model.provider)?;
            Some(ModelChannelCandidate::new(
                index,
                provider.priority,
                provider.weight,
            ))
        })
        .collect();

    let selection = ModelChannelSelection::new(candidates);
    let plan = selection.plan(selector, affinity);
    let primary_index = plan.primary()?;

    let primary_model = &config.models[primary_index];
    let primary_provider = config.enabled_provider(&primary_model.provider)?;

    let chain: Vec<(&ModelConfig, &ProviderConfig)> = plan
        .order()
        .iter()
        .filter_map(|&index| {
            let model = config.models.get(index)?;
            let provider = config.enabled_provider(&model.provider)?;
            Some((model, provider))
        })
        .collect();

    Some(ChannelSelection {
        primary: primary_model,
        primary_provider,
        chain,
        plan,
    })
}

/// 带渠道健康过滤的选择入口（生产路径）。
///
/// 在 `resolve_channel_selection` 的基础上叠加熔断隔离：
/// 1. 先按 priority/weight/affinity 生成完整尝试链。
/// 2. 跳过仍处于冷却窗口内的渠道。
/// 3. **降级保底（fail-open）**：若全部渠道被熔断，仍返回原始首选渠道，
///    避免健康判断误杀导致完全不可用。
///
/// `health` 为可变健康注册表，`now_ms` 为注入的当前时间（保证可测）。
pub fn resolve_channel_selection_with_health<'a>(
    config: &'a AxonConfig,
    model_name: &str,
    selector: u64,
    affinity: Option<usize>,
    health: &mut ChannelHealthRegistry,
    now_ms: u64,
) -> Option<ChannelSelection<'a>> {
    let base = resolve_channel_selection(config, model_name, selector, affinity)?;

    // 单渠道路径：无需健康过滤，直接返回（零开销）。
    if base.chain.len() == 1 {
        return Some(base);
    }

    // 过滤掉仍在冷却窗口内的渠道，保留健康渠道按原顺序排列。
    let healthy: Vec<(&'a ModelConfig, &'a ProviderConfig)> = base
        .chain
        .iter()
        .filter(|(_model, provider)| {
            let key = ChannelKey {
                model_name: model_name.to_owned(),
                provider: provider.name.clone(),
            };
            health.is_available(&key, now_ms)
        })
        .copied()
        .collect();

    // 全部熔断：降级保底返回原始首选，避免无渠道可用。
    if healthy.is_empty() {
        return Some(base);
    }

    let (primary_model, primary_provider) = healthy[0];
    Some(ChannelSelection {
        primary: primary_model,
        primary_provider,
        chain: healthy,
        plan: base.plan,
    })
}

/// 从 config 中收集同一模型名称下所有 provider 名称。
///
/// 用于上层判断是否需要渠道负载均衡。
#[must_use]
pub fn model_channel_providers(config: &AxonConfig, model_name: &str) -> Vec<String> {
    config
        .models
        .iter()
        .filter(|model| model_routing_key(model) == model_name && config.model_is_enabled(model))
        .map(|model| model.provider.clone())
        .collect()
}

/// 构建 model.name -> provider 名称列表索引。
///
/// 用于批量查询或亲和缓存初始化。
#[must_use]
pub fn build_channel_index(config: &AxonConfig) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    for model in config
        .models
        .iter()
        .filter(|model| config.model_is_enabled(model))
    {
        index
            .entry(model_routing_key(model).to_owned())
            .or_default()
            .push(model.provider.clone());
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_config::{PROVIDER_PROTOCOL_OPENAI, ProviderConfig};

    fn make_config() -> AxonConfig {
        let mut config = AxonConfig::default();
        config.providers.push(ProviderConfig {
            name: "p-high".to_owned(),
            priority: 10,
            weight: 3,
            ..ProviderConfig::new(
                "p-high",
                PROVIDER_PROTOCOL_OPENAI,
                "https://a.example.com/v1",
            )
        });
        config.providers.push(ProviderConfig {
            name: "p-low".to_owned(),
            priority: 0,
            weight: 1,
            ..ProviderConfig::new(
                "p-low",
                PROVIDER_PROTOCOL_OPENAI,
                "https://b.example.com/v1",
            )
        });
        config.models.push(ModelConfig {
            name: "gpt-4o".to_owned(),
            id: "gpt-4o-2024-08-06".to_owned(),
            provider: "p-high".to_owned(),
            context_window: 128_000,
            ..ModelConfig::default()
        });
        config.models.push(ModelConfig {
            name: "gpt-4o".to_owned(),
            id: "gpt-4o".to_owned(),
            provider: "p-low".to_owned(),
            context_window: 128_000,
            ..ModelConfig::default()
        });
        config
    }

    #[test]
    fn single_channel_returns_directly() {
        let mut config = AxonConfig::default();
        config.providers.push(ProviderConfig::new(
            "solo",
            PROVIDER_PROTOCOL_OPENAI,
            "https://x.com/v1",
        ));
        config.models.push(ModelConfig {
            name: "llama".to_owned(),
            id: "llama-3.1".to_owned(),
            provider: "solo".to_owned(),
            context_window: 4096,
            ..ModelConfig::default()
        });
        let result = resolve_channel_selection(&config, "llama", 0, None).unwrap();
        assert_eq!(result.chain.len(), 1);
        assert_eq!(result.primary_provider.name, "solo");
        assert_eq!(result.primary.id, "llama-3.1");
        assert!(resolve_channel_selection(&config, "nonexistent", 0, None).is_none());
    }

    #[test]
    fn multi_channel_prioritizes_high_priority() {
        let config = make_config();
        let result = resolve_channel_selection(&config, "gpt-4o", 0, None).unwrap();
        assert_eq!(result.primary_provider.name, "p-high");
        assert_eq!(result.chain.len(), 2);
        // 首选的 id 是 p-high 渠道映射的 id。
        assert_eq!(result.primary.id, "gpt-4o-2024-08-06");
    }

    #[test]
    fn disabled_provider_is_excluded_from_channel_selection_and_index() {
        let mut config = make_config();
        config
            .providers
            .iter_mut()
            .find(|provider| provider.name == "p-high")
            .expect("测试 Provider 应存在")
            .enabled = false;

        let selection =
            resolve_channel_selection(&config, "gpt-4o", 0, None).expect("仍有一个启用渠道");
        let index = build_channel_index(&config);

        assert_eq!(selection.primary_provider.name, "p-low");
        assert_eq!(selection.chain.len(), 1);
        assert_eq!(index.get("gpt-4o"), Some(&vec!["p-low".to_owned()]));
    }

    #[test]
    fn affinity_does_not_override_higher_priority_provider() {
        let config = make_config();
        let result = resolve_channel_selection(&config, "gpt-4o", 0, Some(1)).unwrap();

        assert_eq!(result.primary_provider.name, "p-high");
        assert_eq!(result.primary.id, "gpt-4o-2024-08-06");
        assert_eq!(
            result
                .chain
                .last()
                .map(|(_, provider)| provider.name.as_str()),
            Some("p-low")
        );
    }

    fn make_registry() -> ChannelHealthRegistry {
        ChannelHealthRegistry::new(axon_domain::BreakerPolicy {
            failure_threshold: 1,
            cooldown_ms: 5_000,
        })
    }

    fn trip_breaker(health: &mut ChannelHealthRegistry, provider: &str, now_ms: u64) {
        health.record_failure(
            &ChannelKey {
                model_name: "gpt-4o".to_owned(),
                provider: provider.to_owned(),
            },
            now_ms,
        );
    }

    #[test]
    fn health_filter_excludes_open_channel() {
        let config = make_config();
        let mut health = make_registry();
        // 熔断首选渠道 p-high（阈值=1，一次失败即 Open）。
        trip_breaker(&mut health, "p-high", 1_000);

        let result =
            resolve_channel_selection_with_health(&config, "gpt-4o", 0, None, &mut health, 1_100)
                .unwrap();
        // p-high 被排除，仅剩 p-low。
        assert_eq!(result.chain.len(), 1);
        assert_eq!(result.primary_provider.name, "p-low");
    }

    #[test]
    fn health_filter_fails_open_when_all_broken() {
        let config = make_config();
        let mut health = make_registry();
        trip_breaker(&mut health, "p-high", 1_000);
        trip_breaker(&mut health, "p-low", 1_000);

        // 全熔断：降级保底返回原始首选，链不为空。
        let result =
            resolve_channel_selection_with_health(&config, "gpt-4o", 0, None, &mut health, 1_100)
                .unwrap();
        assert_eq!(result.chain.len(), 2);
        assert_eq!(result.primary_provider.name, "p-high");
    }

    #[test]
    fn health_filter_recovers_after_cooldown() {
        let config = make_config();
        let mut health = make_registry();
        trip_breaker(&mut health, "p-high", 1_000);

        // 冷却窗口（5s）过后，p-high 恢复放行，重回首选。
        let result =
            resolve_channel_selection_with_health(&config, "gpt-4o", 0, None, &mut health, 6_001)
                .unwrap();
        assert_eq!(result.chain.len(), 2);
        assert_eq!(result.primary_provider.name, "p-high");
    }
}
