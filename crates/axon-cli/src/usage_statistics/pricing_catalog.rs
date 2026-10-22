//! Models.dev 费率目录的解析。
//!
//! 线上 api.json 的形状是 `{ "<provider>": { "models": { "<id>": {...} } } }`，
//! 与同步客户端（拉取、防抖、落库）分开成篇：解析规则要按真实形状、同名供应商、
//! 零价条目与去重四条纪律逐条讲清，和 HTTP 传输不是一回事。

use std::collections::HashMap;
use std::path::Path;

use axon_domain::{ModelPriceRule, PricingStyle, quantize_price_per_m_from_usd};
use serde::Deserialize;

/// Models.dev 单项成本。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelsDevCost {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// Models.dev 模型项容错模型。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelsDevItem {
    pub id: String,
    pub name: Option<String>,
    pub cost: Option<ModelsDevCost>,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// Models.dev 的目录条目：一个供应商下的全部模型。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
struct ModelsDevProvider {
    models: HashMap<String, ModelsDevItem>,
}

/// 同步写入的规则优先级。
///
/// 取负值：目录里的条目**永远让位于手工规则**。同一个模型在不同供应商下有不同价
/// （厂商自营与聚合商能差好几倍），而规则只按模型匹配，因此同步不能靠「后写的赢」
/// 去覆盖用户自己填的那一条。四级全序里 priority 是第一顺位，负值即天然让位。
pub const SYNCED_RULE_PRIORITY: i32 = -1;

/// 解析 Models.dev 的目录结构（供应商 → 模型 → 费率），并按本地供应商过滤。
///
/// 线上 api.json 的实际形状是 `{ "<provider>": { "models": { "<id>": {...} } } }`。
/// 只按扁平形状解析会把它读成一堆「没有单价的供应商」而整批跳过，回执却是成功：
/// 表现为「同步成功但一条规则都没写」。这里按真实形状解析，并且：
///
/// 1. **只收与本地配置同名供应商的条目**：同一个模型 id 在不同供应商下价差很大，
///    全量导入等于按任意一条覆盖事实，宁可少收也不乱收；
/// 2. **跳过全零条目**：目录里的 0 价是订阅或免费额度这类计划信息，不是可比单价；
///    真要把某个模型按 0 计价，应当由人显式建一条规则；
/// 3. **按「匹配式 + 单价」去重**：多个同名供应商给出同一份价时只留一条；
/// 4. 写入的规则优先级取负值，手工规则始终优先（见 [`SYNCED_RULE_PRIORITY`]）。
pub fn parse_models_dev_catalog(
    json_str: &str,
    now_ms: u64,
    local_providers: &std::collections::BTreeSet<String>,
) -> Result<Vec<ModelPriceRule>, String> {
    let catalog: HashMap<String, ModelsDevProvider> = serde_json::from_str(json_str)
        .map_err(|error| format!("无法识别的 Models.dev 目录格式: {error}"))?;

    let mut rules = Vec::new();
    let mut seen: std::collections::BTreeSet<(String, u64, u64, u64, u64)> =
        std::collections::BTreeSet::new();
    for (provider_name, provider) in catalog {
        if !local_providers.contains(&provider_name.to_ascii_lowercase()) {
            continue;
        }
        for (model_key, mut item) in provider.models {
            if item.id.trim().is_empty() {
                item.id = model_key;
            }
            let Some(mut rule) = rule_from_item(&item, now_ms, Some(&provider_name)) else {
                continue;
            };
            if rule.input_price_per_m_micros == 0
                && rule.output_price_per_m_micros == 0
                && rule.cache_read_price_per_m_micros == 0
                && rule.cache_write_price_per_m_micros == 0
            {
                continue;
            }
            let key = (
                rule.model_pattern.clone(),
                rule.input_price_per_m_micros,
                rule.output_price_per_m_micros,
                rule.cache_read_price_per_m_micros,
                rule.cache_write_price_per_m_micros,
            );
            if !seen.insert(key) {
                continue;
            }
            rule.priority = SYNCED_RULE_PRIORITY;
            rules.push(rule);
        }
    }
    rules.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(rules)
}

/// 从一个目录条目构造价格规则；缺少核心单价时返回 None。
fn rule_from_item(
    item: &ModelsDevItem,
    now_ms: u64,
    provider: Option<&str>,
) -> Option<ModelPriceRule> {
    if item.id.trim().is_empty() {
        return None;
    }
    let input_opt = item.cost.as_ref().and_then(|c| c.input).or(item.input);
    let output_opt = item.cost.as_ref().and_then(|c| c.output).or(item.output);
    let (input_usd, output_usd) = match (input_opt, output_opt) {
        (Some(input), Some(output)) if input >= 0.0 && output >= 0.0 => (input, output),
        _ => return None,
    };
    let cache_read_usd = item
        .cost
        .as_ref()
        .and_then(|c| c.cache_read)
        .or(item.cache_read)
        .unwrap_or(0.0);
    let cache_write_usd = item
        .cost
        .as_ref()
        .and_then(|c| c.cache_write)
        .or(item.cache_write)
        .unwrap_or(0.0);
    let pricing_style = if cache_write_usd > 0.0 {
        PricingStyle::Claude
    } else {
        PricingStyle::OpenAi
    };
    // 规则 id 带供应商：同一个模型 id 在不同供应商下是两条不同的费率事实。
    let id = match provider {
        Some(provider) => format!("models_dev_{provider}_{}", item.id),
        None => format!("models_dev_{}", item.id),
    };
    Some(ModelPriceRule {
        id,
        model_pattern: item.id.clone(),
        pricing_style,
        input_price_per_m_micros: quantize_price_per_m_from_usd(input_usd),
        output_price_per_m_micros: quantize_price_per_m_from_usd(output_usd),
        cache_read_price_per_m_micros: quantize_price_per_m_from_usd(cache_read_usd),
        cache_write_price_per_m_micros: quantize_price_per_m_from_usd(cache_write_usd),
        multiplier_basis_points: 10_000,
        priority: 0,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    })
}

/// 读取本机配置里声明的供应商名（小写），作为目录过滤的对照表。
pub(super) fn local_provider_names(axon_root: &Path) -> std::collections::BTreeSet<String> {
    let path = axon_root.join("config.toml");
    let Ok(loaded) = axon_config::ConfigLoader::load_from_path(&path) else {
        return std::collections::BTreeSet::new();
    };
    loaded
        .config
        .providers
        .iter()
        .map(|provider| provider.name.to_ascii_lowercase())
        .collect()
}

/// 解析 Models.dev 响应 JSON。
///
/// 具备高度容错性：
/// 1. 支持数组 `[ ... ]`、带 models 包装的 `{ "models": [ ... ] }` 或键值映射 `{ "model_id": { ... } }`；
/// 2. 缺少核心 input 或 output 单价的模型跳过并告警；
/// 3. 单价量化为纯整数微美元；
/// 4. 绝不 panic。
pub fn parse_models_dev_json(json_str: &str, now_ms: u64) -> Result<Vec<ModelPriceRule>, String> {
    let mut items = Vec::new();

    if let Ok(list) = serde_json::from_str::<Vec<ModelsDevItem>>(json_str) {
        items = list;
    } else if let Ok(wrapper) =
        serde_json::from_str::<HashMap<String, Vec<ModelsDevItem>>>(json_str)
    {
        if let Some(list) = wrapper.get("models") {
            items = list.clone();
        }
    } else if let Ok(map) = serde_json::from_str::<HashMap<String, ModelsDevItem>>(json_str) {
        for (key, mut item) in map {
            if item.id.is_empty() {
                item.id = key;
            }
            items.push(item);
        }
    } else {
        return Err("无法识别的 Models.dev JSON 格式".to_owned());
    }

    let mut rules = Vec::with_capacity(items.len());
    for item in items {
        if item.id.trim().is_empty() {
            continue;
        }

        let input_opt = item.cost.as_ref().and_then(|c| c.input).or(item.input);
        let output_opt = item.cost.as_ref().and_then(|c| c.output).or(item.output);

        let (input_usd, output_usd) = match (input_opt, output_opt) {
            (Some(inp), Some(out)) if inp >= 0.0 && out >= 0.0 => (inp, out),
            _ => {
                axon_logger::warning(&format!("跳过缺少核心输入/输出单价的模型: {}", item.id));
                continue;
            }
        };

        let cache_read_usd = item
            .cost
            .as_ref()
            .and_then(|c| c.cache_read)
            .or(item.cache_read)
            .unwrap_or(0.0);
        let cache_write_usd = item
            .cost
            .as_ref()
            .and_then(|c| c.cache_write)
            .or(item.cache_write)
            .unwrap_or(0.0);

        let pricing_style = if cache_write_usd > 0.0 {
            PricingStyle::Claude
        } else {
            PricingStyle::OpenAi
        };

        rules.push(ModelPriceRule {
            id: format!("models_dev_{}", item.id),
            model_pattern: item.id,
            pricing_style,
            input_price_per_m_micros: quantize_price_per_m_from_usd(input_usd),
            output_price_per_m_micros: quantize_price_per_m_from_usd(output_usd),
            cache_read_price_per_m_micros: quantize_price_per_m_from_usd(cache_read_usd),
            cache_write_price_per_m_micros: quantize_price_per_m_from_usd(cache_write_usd),
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        });
    }

    Ok(rules)
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    const FIXTURE_CATALOG: &str = r#"{
        "deepseek": {
            "id": "deepseek",
            "name": "DeepSeek",
            "models": {
                "deepseek-flash": {
                    "id": "deepseek-flash",
                    "name": "DeepSeek V4.1 Flash",
                    "cost": { "input": 0.15, "output": 0.6, "cache_read": 0.003 }
                },
                "deepseek-v4-pro": {
                    "id": "deepseek-v4-pro",
                    "name": "DeepSeek V4 Pro",
                    "cost": { "input": 0.66, "output": 1.98, "cache_read": 0.022 }
                }
            }
        },
        "opencode": {
            "id": "opencode",
            "name": "OpenCode",
            "models": {
                "deepseek-flash": {
                    "id": "deepseek-flash",
                    "cost": { "input": 0.3, "output": 1.2, "cache_read": 0.006 }
                }
            }
        },
        "alibaba": {
            "id": "alibaba",
            "name": "Alibaba",
            "models": {
                "qwen3.8-flash": {
                    "id": "qwen3.8-flash",
                    "cost": { "input": 0.15, "output": 0.47, "cache_read": 0.016, "cache_write": 0.2 }
                }
            }
        }
    }"#;

    #[test]
    fn catalog_parser_reads_provider_nesting_and_keeps_same_named_providers() {
        let providers: std::collections::BTreeSet<String> =
            ["deepseek".to_owned(), "alibaba".to_owned()]
                .into_iter()
                .collect();
        let rules =
            parse_models_dev_catalog(FIXTURE_CATALOG, 1_000, &providers).expect("目录应可解析");

        // opencode 不在本地供应商里：它那条同名模型不进来，避免用聚合商的价覆盖厂商自营价。
        assert_eq!(rules.len(), 3);
        assert!(
            rules.iter().all(|rule| !rule.id.contains("opencode")),
            "非本地同名供应商的条目必须被过滤"
        );
        assert!(
            rules
                .iter()
                .all(|rule| rule.priority == SYNCED_RULE_PRIORITY),
            "同步写入的规则优先级必须低于手工规则"
        );

        let flash = rules
            .iter()
            .find(|rule| rule.model_pattern == "deepseek-flash")
            .expect("deepseek-flash 应被收录");
        assert_eq!(flash.id, "models_dev_deepseek_deepseek-flash");
        assert_eq!(
            flash.input_price_per_m_micros, 150_000,
            "0.15 美元 = 150000 微美元"
        );
        assert_eq!(flash.output_price_per_m_micros, 600_000);
        assert_eq!(flash.cache_read_price_per_m_micros, 3_000);
        assert_eq!(flash.pricing_style, PricingStyle::OpenAi);

        let qwen = rules
            .iter()
            .find(|rule| rule.model_pattern == "qwen3.8-flash")
            .expect("qwen3.8-flash 应被收录");
        assert_eq!(
            qwen.pricing_style,
            PricingStyle::Claude,
            "有缓存写入单价时按 Claude 风格计价"
        );
        assert_eq!(qwen.cache_write_price_per_m_micros, 200_000);
    }

    #[test]
    fn catalog_parser_skips_free_and_duplicate_entries() {
        let catalog = r#"{
            "agnes": {
                "models": {
                    "agnes-3.0-flash": { "id": "agnes-3.0-flash", "cost": { "input": 0, "output": 0 } },
                    "agnes-2.5-pro-alpha": { "id": "agnes-2.5-pro-alpha", "cost": { "input": 0.45, "output": 0.9 } }
                }
            },
            "sensenova": {
                "models": {
                    "agnes-2.5-pro-alpha": { "id": "agnes-2.5-pro-alpha", "cost": { "input": 0.45, "output": 0.9 } },
                    "sensenova-6.8-flash-lite": { "id": "sensenova-6.8-flash-lite", "cost": { "input": 0, "output": 0 } }
                }
            }
        }"#;
        let providers: std::collections::BTreeSet<String> =
            ["agnes".to_owned(), "sensenova".to_owned()]
                .into_iter()
                .collect();
        let rules = parse_models_dev_catalog(catalog, 1_000, &providers).expect("目录应可解析");

        // 全零条目是订阅或免费额度这类计划信息，不是可比单价：进了目录会把按量计费的
        // 模型算成免费，因此一律跳过；真要按 0 计价由人显式建规则。
        assert!(
            rules
                .iter()
                .all(|rule| rule.input_price_per_m_micros > 0 || rule.output_price_per_m_micros > 0),
            "全零条目必须被跳过"
        );
        // 两个供应商给出同一份价：只留一条。
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].model_pattern, "agnes-2.5-pro-alpha");
    }

    #[test]
    fn catalog_parser_skips_models_without_core_prices() {
        let catalog = r#"{
            "deepseek": {
                "models": {
                    "no-cost": { "id": "no-cost" },
                    "only-input": { "id": "only-input", "cost": { "input": 1.0 } },
                    "ok": { "id": "ok", "cost": { "input": 1.0, "output": 2.0 } }
                }
            }
        }"#;
        let providers: std::collections::BTreeSet<String> =
            ["deepseek".to_owned()].into_iter().collect();
        let rules = parse_models_dev_catalog(catalog, 1_000, &providers).expect("目录应可解析");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].model_pattern, "ok");
    }
}
