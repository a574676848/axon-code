//! 本机模型 id 与目录规范模型的对齐。
//!
//! 代理与聚合网关基本不按目录 id 命名模型：同一个模型会被写成
//! `gemini-4-hydrogen-xhigh`（思考档位后缀）、`deepseek-v4.1-flash-1017`（日期戳）
//! 或 `google/gemini-4-argon`（厂商前缀）。规则按用量事实里的模型字符串匹配，
//! 所以这些写法各自要一条匹配式，而价格仍是目录里同一个模型的官方价。
//!
//! 对齐只认目录自己写明的东西：
//! 1. `canonical_model_id` 给出规范 id 与厂商归属；
//! 2. `reasoning_options` 给出该模型允许的思考档位取值；
//! 3. 纯数字后缀（4、6、8 位）视为日期或版本戳。
//!
//! 价格只取**厂商自营条目**（规范 id 的厂商段等于条目所在供应商）：聚合商会加价，
//! 拿它们的价顶替等于按任意一条覆盖事实。取不到就不写规则，由人显式建。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axon_domain::ModelPriceRule;

use super::pricing_catalog::{
    CatalogQuote, ModelsDevItem, ModelsDevProvider, SYNCED_RULE_PRIORITY, quote_from_item,
};

/// 允许从模型 id 上剥掉的最大修饰层数。
///
/// 实测最多两层（例如 `-high-1017`）；再多就成了我们替别人猜命名规则，
/// 而不是目录声明过的东西。
const MAX_SUFFIX_TOKENS: usize = 2;

/// 一条厂商自营报价，连同它声明的思考档位取值。
#[derive(Debug, Clone)]
struct FirstPartyEntry {
    quote: CatalogQuote,
    effort_values: BTreeSet<String>,
    /// 同一规范 id 的自营别名是否给出了互相矛盾的单价。
    conflict: bool,
}

/// 为本机声明、但目录里查不到同名条目的模型 id 生成同步规则。
pub(super) fn local_model_rules(
    catalog: &HashMap<String, ModelsDevProvider>,
    local_ids: &[String],
    now_ms: u64,
) -> Vec<ModelPriceRule> {
    let index = CatalogIndex::build(catalog);
    let mut rules = Vec::new();
    let mut seen_patterns = BTreeSet::new();
    for local_id in local_ids {
        let id = local_id.trim();
        // 带通配符的 id 不是具体模型：这类规则由人显式建，同步不得代劳。
        if id.is_empty() || id.contains('*') || id.contains('?') {
            continue;
        }
        let Some(entry) = index.resolve(id) else {
            continue;
        };
        if !seen_patterns.insert(id.to_ascii_lowercase()) {
            continue;
        }
        rules.push(ModelPriceRule {
            id: format!("models_dev_local_{}", sanitize_rule_id(id)),
            model_pattern: id.to_owned(),
            pricing_style: entry.quote.pricing_style,
            input_price_per_m_micros: entry.quote.input,
            output_price_per_m_micros: entry.quote.output,
            cache_read_price_per_m_micros: entry.quote.cache_read,
            cache_write_price_per_m_micros: entry.quote.cache_write,
            multiplier_basis_points: 10_000,
            priority: SYNCED_RULE_PRIORITY,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        });
    }
    rules.sort_by(|a, b| a.id.cmp(&b.id));
    rules
}

/// 目录的两种索引：规范 id → 自营报价，以及「裸模型 id → 它所属的厂商」。
struct CatalogIndex {
    quotes: BTreeMap<String, FirstPartyEntry>,
    owners: BTreeMap<String, BTreeSet<String>>,
}

impl CatalogIndex {
    fn build(catalog: &HashMap<String, ModelsDevProvider>) -> Self {
        let mut quotes: BTreeMap<String, FirstPartyEntry> = BTreeMap::new();
        let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (provider_key, provider) in catalog {
            for item in provider.models.values() {
                // 没有规范 id 的条目说不清归属：宁可不用，也不把聚合商当成厂商。
                let Some((owner, model)) = canonical_of(item) else {
                    continue;
                };
                owners
                    .entry(model.clone())
                    .or_default()
                    .insert(owner.clone());
                if !owner.eq_ignore_ascii_case(provider_key) {
                    continue; // 转售条目：带加价，不参与官方价
                }
                let Some(quote) = quote_from_item(item) else {
                    continue;
                };
                // 全零条目是订阅或免费额度这类计划信息，不是可比单价。
                if quote.input == 0 && quote.output == 0 {
                    continue;
                }
                let key = canonical_key(&owner, &model);
                let effort_values = effort_values_of(item);
                match quotes.get_mut(&key) {
                    Some(existing) => {
                        if existing.quote != quote {
                            existing.conflict = true;
                        }
                        existing.effort_values.extend(effort_values);
                    }
                    None => {
                        quotes.insert(
                            key,
                            FirstPartyEntry {
                                quote,
                                effort_values,
                                conflict: false,
                            },
                        );
                    }
                }
            }
        }
        Self { quotes, owners }
    }

    /// 解析一个本机模型 id；解析不出唯一报价时返回 None。
    fn resolve(&self, local_id: &str) -> Option<&FirstPartyEntry> {
        let (explicit_owner, bare) = split_owner(local_id);
        for (base, suffixes) in strip_candidates(bare) {
            let keys: Vec<String> = match &explicit_owner {
                // 厂商前缀写法本身就是规范 id 的形状，不需要再猜归属。
                Some(owner) => vec![canonical_key(owner, &base)],
                None => match self.owners.get(&base.to_ascii_lowercase()) {
                    Some(owners) => owners
                        .iter()
                        .map(|owner| canonical_key(owner, &base))
                        .collect(),
                    None => continue,
                },
            };
            let mut matched: Option<&FirstPartyEntry> = None;
            let mut ambiguous = false;
            for key in keys {
                let Some(entry) = self.quote_for(&key, &suffixes) else {
                    continue;
                };
                match matched {
                    // 同一个裸 id 在多厂商下给出不同价：这是两个模型，不猜。
                    Some(current) if current.quote != entry.quote => ambiguous = true,
                    Some(_) => {}
                    None => matched = Some(entry),
                }
            }
            if ambiguous {
                return None;
            }
            if matched.is_some() {
                return matched;
            }
        }
        None
    }

    /// 取某个规范 id 的报价，并核验剥掉的每一层都是目录声明过的修饰。
    fn quote_for(&self, key: &str, suffixes: &[String]) -> Option<&FirstPartyEntry> {
        let entry = self.quotes.get(key)?;
        if entry.conflict {
            return None;
        }
        let declared = suffixes.iter().all(|suffix| {
            is_date_stamp(suffix) || entry.effort_values.contains(&suffix.to_ascii_lowercase())
        });
        declared.then_some(entry)
    }
}

/// 拆出可选的厂商前缀，返回（厂商，裸模型 id）。
fn split_owner(id: &str) -> (Option<String>, &str) {
    match id.split_once('/') {
        Some((owner, rest)) if !owner.is_empty() && !rest.is_empty() => {
            (Some(owner.to_owned()), rest)
        }
        _ => (None, id),
    }
}

/// 规范 id：`<厂商>/<型号>`，统一小写。
fn canonical_key(owner: &str, model: &str) -> String {
    format!(
        "{}/{}",
        owner.trim().to_ascii_lowercase(),
        model.trim().to_ascii_lowercase()
    )
}

/// 从条目里取出规范 id 的（厂商，型号）。
fn canonical_of(item: &ModelsDevItem) -> Option<(String, String)> {
    let canonical = item.canonical_model_id.as_deref()?.trim();
    let (owner, model) = canonical.split_once('/')?;
    if owner.is_empty() || model.is_empty() {
        return None;
    }
    Some((owner.to_ascii_lowercase(), model.to_ascii_lowercase()))
}

/// 条目声明的思考档位取值（小写）。
fn effort_values_of(item: &ModelsDevItem) -> BTreeSet<String> {
    item.reasoning_options
        .iter()
        .filter(|option| option.is_effort())
        .flat_map(|option| option.values.iter())
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

/// 「原样 + 逐层剥掉 `-` 后缀」的候选基名，按剥离层数从少到多。
fn strip_candidates(id: &str) -> Vec<(String, Vec<String>)> {
    let mut candidates = vec![(id.to_owned(), Vec::new())];
    let mut base = id.to_owned();
    let mut stripped: Vec<String> = Vec::new();
    for _ in 0..MAX_SUFFIX_TOKENS {
        let Some((head, tail)) = base.rsplit_once('-') else {
            break;
        };
        if head.is_empty() || tail.is_empty() {
            break;
        }
        stripped.push(tail.to_ascii_lowercase());
        base = head.to_owned();
        candidates.push((base.clone(), stripped.clone()));
    }
    candidates
}

/// 纯数字后缀（4、6、8 位）视为日期或版本戳，例如 `deepseek-v4.1-flash-1017`。
fn is_date_stamp(token: &str) -> bool {
    matches!(token.len(), 4 | 6 | 8) && token.chars().all(|c| c.is_ascii_digit())
}

/// 规则 id 只保留可读字符，保证同一个本机 id 每次都得到同一个规则 id。
fn sanitize_rule_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_statistics::pricing_catalog::parse_catalog_providers;

    /// 目录夹具：三家自营厂商 + 一个转售商 + 一个计划条目 + 一对自相矛盾的别名。
    const FIXTURE: &str = r#"{
        "google": {
            "models": {
                "gemini-4-hydrogen": {
                    "id": "gemini-4-hydrogen",
                    "canonical_model_id": "google/gemini-4-hydrogen",
                    "reasoning_options": [{"type": "effort", "values": ["low", "high", "xhigh"]}],
                    "cost": { "input": 1.5, "output": 7.5, "cache_read": 0.075 }
                },
                "gemini-3.8-flash": {
                    "id": "gemini-3.8-flash",
                    "canonical_model_id": "google/gemini-3.8-flash",
                    "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}],
                    "cost": { "input": 0.75, "output": 3.75, "cache_read": 0.075 }
                },
                "gemini-4-argon": {
                    "id": "gemini-4-argon",
                    "canonical_model_id": "google/gemini-4-argon",
                    "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}],
                    "cost": { "input": 2, "output": 10, "cache_read": 0.1 }
                }
            }
        },
        "cortecs": {
            "models": {
                "gemini-4-argon": {
                    "id": "gemini-4-argon",
                    "canonical_model_id": "google/gemini-4-argon",
                    "cost": { "input": 2.25, "output": 11.249 }
                }
            }
        },
        "openai": {
            "models": {
                "gpt-6-astra": {
                    "id": "gpt-6-astra",
                    "canonical_model_id": "openai/gpt-6-astra",
                    "cost": { "input": 10, "output": 50, "cache_read": 1 }
                }
            }
        },
        "deepseek": {
            "models": {
                "deepseek-flash": {
                    "id": "deepseek-flash",
                    "canonical_model_id": "deepseek/deepseek-v4.1-flash-1017",
                    "cost": { "input": 0.15, "output": 0.6, "cache_read": 0.003 }
                }
            }
        },
        "alibaba-cn": {
            "models": {
                "deepseek-v4.1-flash": {
                    "id": "deepseek-v4.1-flash",
                    "canonical_model_id": "deepseek/deepseek-v4.1-flash",
                    "cost": { "input": 0.29754, "output": 1.19015 }
                }
            }
        },
        "fw": {
            "models": {
                "fk-1": {
                    "id": "fk-1",
                    "canonical_model_id": "fw/fk-1",
                    "cost": { "input": 0, "output": 0 }
                }
            }
        },
        "vendorc": {
            "models": {
                "vc-1": {
                    "id": "vc-1",
                    "canonical_model_id": "vendorc/vc-1",
                    "cost": { "input": 1, "output": 2 }
                },
                "vc-1-alias": {
                    "id": "vc-1-alias",
                    "canonical_model_id": "vendorc/vc-1",
                    "cost": { "input": 3, "output": 4 }
                }
            }
        }
    }"#;

    fn catalog() -> HashMap<String, ModelsDevProvider> {
        parse_catalog_providers(FIXTURE).expect("目录夹具应可解析")
    }

    fn rule<'a>(rules: &'a [ModelPriceRule], pattern: &str) -> &'a ModelPriceRule {
        rules
            .iter()
            .find(|rule| rule.model_pattern == pattern)
            .unwrap_or_else(|| panic!("缺少 {pattern} 的规则"))
    }

    #[test]
    fn proxy_side_suffixes_resolve_to_the_first_party_price() {
        let ids: Vec<String> = [
            "gemini-4-hydrogen-xhigh",
            "gemini-3.8-flash-high",
            "gemini-4-argon-high",
            "google/gemini-4-argon",
            "deepseek-v4.1-flash-1017",
            "gpt-6-astra",
        ]
        .iter()
        .map(|id| (*id).to_owned())
        .collect();

        let rules = local_model_rules(&catalog(), &ids, 7_000);
        assert_eq!(rules.len(), ids.len(), "六个别名都应定价");

        let hydrogen = rule(&rules, "gemini-4-hydrogen-xhigh");
        assert_eq!(
            (
                hydrogen.input_price_per_m_micros,
                hydrogen.output_price_per_m_micros,
                hydrogen.cache_read_price_per_m_micros,
            ),
            (1_500_000, 7_500_000, 75_000)
        );
        // 匹配式保留本机写法：用量事实里就是这条字符串。
        assert_eq!(hydrogen.id, "models_dev_local_gemini-4-hydrogen-xhigh");
        assert_eq!(hydrogen.priority, SYNCED_RULE_PRIORITY);
        assert_eq!(hydrogen.multiplier_basis_points, 10_000);

        // 厂商前缀写法不需要再猜归属。
        let prefixed = rule(&rules, "google/gemini-4-argon");
        assert_eq!(prefixed.input_price_per_m_micros, 2_000_000);
        assert_eq!(prefixed.id, "models_dev_local_google_gemini-4-argon");

        // 日期戳本身就是规范 id 的一部分，不必剥后缀。
        let dated = rule(&rules, "deepseek-v4.1-flash-1017");
        assert_eq!(dated.output_price_per_m_micros, 600_000);
    }

    #[test]
    fn reseller_listings_never_replace_the_first_party_price() {
        let rules = local_model_rules(&catalog(), &["gemini-4-argon-high".to_owned()], 1);
        let argon = rule(&rules, "gemini-4-argon-high");
        // cortecs 的 2.25 / 11.249 是加价后的转售价，不能顶替 google 的 2 / 10。
        assert_eq!(argon.input_price_per_m_micros, 2_000_000);
        assert_eq!(argon.output_price_per_m_micros, 10_000_000);
    }

    #[test]
    fn date_stamp_alias_falls_back_to_the_base_model() {
        let rules = local_model_rules(&catalog(), &["gpt-6-astra-20260117".to_owned()], 1);
        let dated = rule(&rules, "gpt-6-astra-20260117");
        assert_eq!(dated.input_price_per_m_micros, 10_000_000);
        assert_eq!(dated.cache_read_price_per_m_micros, 1_000_000);
    }

    #[test]
    fn unknown_or_unowned_suffixes_produce_no_rule() {
        let ids: Vec<String> = [
            // `turbo` 既不是日期戳，也不在 gemini-4-hydrogen 声明的档位里
            "gemini-4-hydrogen-turbo",
            // 两层都没命中声明
            "gemini-4-hydrogen-high-ultra",
            // 只有转售商列出了这个规范 id，没有自营价
            "deepseek-v4.1-flash",
            // 目录里根本没有这个模型
            "neohorse-1.5-flash-preview",
        ]
        .iter()
        .map(|id| (*id).to_owned())
        .collect();

        assert!(
            local_model_rules(&catalog(), &ids, 1).is_empty(),
            "解析不出自营价时不得凭空造一条规则"
        );
    }

    #[test]
    fn plan_entries_and_conflicting_aliases_are_skipped() {
        let ids: Vec<String> = vec!["fk-1".to_owned(), "vc-1".to_owned()];
        assert!(
            local_model_rules(&catalog(), &ids, 1).is_empty(),
            "全零计划条目与自相矛盾的自营别名都不该产出规则"
        );
    }

    #[test]
    fn wildcard_or_blank_local_ids_are_ignored() {
        let ids: Vec<String> = vec!["gemini-*".to_owned(), "  ".to_owned(), String::new()];
        assert!(local_model_rules(&catalog(), &ids, 1).is_empty());
    }

    #[test]
    fn duplicate_local_ids_keep_one_rule() {
        let ids = vec![
            "gemini-4-argon-high".to_owned(),
            "gemini-4-argon-high".to_owned(),
            "GEMINI-4-ARGON-HIGH".to_owned(),
        ];
        assert_eq!(local_model_rules(&catalog(), &ids, 1).len(), 1);
    }
}
