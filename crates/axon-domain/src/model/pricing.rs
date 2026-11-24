//! 模型费率合同与计费领域模型。

use serde::{Deserialize, Serialize};

/// 计费风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingStyle {
    /// OpenAI 风格：缓存写入不单独计价（单价为 0）。
    OpenAi,
    /// Claude 风格：支持缓存写入计价。
    Claude,
}

impl PricingStyle {
    /// 返回风格的字符串表示。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Claude => "claude",
        }
    }

    /// 从字符串解析计费风格。
    ///
    /// 同时接受 `open_ai` 与 `openai`：前者是本枚举的序列化形式（`snake_case`），
    /// 后者是配置与接口里手写的常见写法。只认其中一个会让「读回来的值再写回去」
    /// 失败，而这类回写正是编辑既有规则时必走的一步。
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let normalized = s.trim().to_ascii_lowercase().replace('_', "");
        if normalized == "openai" {
            Some(Self::OpenAi)
        } else if normalized == "claude" {
            Some(Self::Claude)
        } else {
            None
        }
    }
}

/// 模型价格规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPriceRule {
    /// 规则唯一标识。
    pub id: String,
    /// 模型匹配模式（支持 `*` 与 `?` 通配符）。
    pub model_pattern: String,
    /// 计费风格。
    pub pricing_style: PricingStyle,
    /// 输入单价（微美元 / 百万 Token）。
    pub input_price_per_m_micros: u64,
    /// 输出单价（微美元 / 百万 Token）。
    pub output_price_per_m_micros: u64,
    /// 缓存读取单价（微美元 / 百万 Token）。
    pub cache_read_price_per_m_micros: u64,
    /// 缓存写入单价（微美元 / 百万 Token）。
    pub cache_write_price_per_m_micros: u64,
    /// 费率倍率（单位为 basis points，10000 = 1.0x）。
    pub multiplier_basis_points: u32,
    /// 优先级（数值越大优先级越高）。
    pub priority: i32,
    /// 创建时间毫秒戳。
    pub created_at_ms: u64,
    /// 更新时间毫秒戳。
    pub updated_at_ms: u64,
}

impl ModelPriceRule {
    /// 判断给定的模型名称是否匹配本规则的 pattern。
    #[must_use]
    pub fn matches(&self, model: &str) -> bool {
        glob_match(&self.model_pattern, model)
    }

    /// 计算此模式的字面字符长度（排除 `*` 与 `?` 通配符）。
    #[must_use]
    pub fn pattern_literal_len(&self) -> usize {
        pattern_literal_len(&self.model_pattern)
    }
}

/// 计算模式字面字符数（排除 `*` 和 `?`）。
#[must_use]
pub fn pattern_literal_len(pattern: &str) -> usize {
    pattern.chars().filter(|&c| c != '*' && c != '?').count()
}

/// 规则全序比较器：
/// 1. `priority` 降序；
/// 2. 模式字面字符数最多（降序）；
/// 3. `updated_at_ms` 最新（降序）；
/// 4. `id` 字典序最小（升序）。
#[must_use]
pub fn compare_rules_priority(a: &ModelPriceRule, b: &ModelPriceRule) -> std::cmp::Ordering {
    b.priority
        .cmp(&a.priority)
        .then_with(|| b.pattern_literal_len().cmp(&a.pattern_literal_len()))
        .then_with(|| b.updated_at_ms.cmp(&a.updated_at_ms))
        .then_with(|| a.id.cmp(&b.id))
}

/// 从候选规则列表中挑选对目标模型的最优匹配规则。
#[must_use]
pub fn match_rule<'a>(rules: &'a [ModelPriceRule], model: &str) -> Option<&'a ModelPriceRule> {
    rules
        .iter()
        .filter(|rule| rule.matches(model))
        .min_by(|a, b| compare_rules_priority(a, b))
}

/// 隐式全串锚定、ASCII 大小写不敏感的简易 glob 匹配（仅支持 `*` 与 `?`）。
#[must_use]
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p_chars: Vec<char> = pattern.chars().map(|c| c.to_ascii_lowercase()).collect();
    let t_chars: Vec<char> = text.chars().map(|c| c.to_ascii_lowercase()).collect();

    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut star_idx = None;
    let mut match_idx = 0;

    while t_idx < t_chars.len() {
        if p_idx < p_chars.len() && (p_chars[p_idx] == '?' || p_chars[p_idx] == t_chars[t_idx]) {
            p_idx += 1;
            t_idx += 1;
        } else if p_idx < p_chars.len() && p_chars[p_idx] == '*' {
            star_idx = Some(p_idx);
            p_idx += 1;
            match_idx = t_idx;
        } else if let Some(star) = star_idx {
            p_idx = star + 1;
            match_idx += 1;
            t_idx = match_idx;
        } else {
            return false;
        }
    }

    while p_idx < p_chars.len() && p_chars[p_idx] == '*' {
        p_idx += 1;
    }

    p_idx == p_chars.len()
}

/// 将美元/百万 Token 的浮点价格量化为微美元/百万 Token（四舍五入，Round-Half-Up）。
#[must_use]
pub fn quantize_price_per_m_from_usd(price_usd: f64) -> u64 {
    if price_usd <= 0.0 {
        return 0;
    }
    ((price_usd * 1_000_000.0) + 0.5) as u64
}

/// 单项计费（纯整数 Round-Half-Up）：
/// - `base = (tokens as u128 * price_per_m_micros as u128 + 500_000) / 1_000_000`
/// - `cost_micros = ((base as u128 * multiplier_basis_points as u128 + 5_000) / 10_000) as u64`
#[must_use]
pub fn calculate_item_cost_micros(
    tokens: u64,
    price_per_m_micros: u64,
    multiplier_basis_points: u32,
) -> u64 {
    if tokens == 0 || price_per_m_micros == 0 || multiplier_basis_points == 0 {
        return 0;
    }
    let base = (tokens as u128 * price_per_m_micros as u128 + 500_000) / 1_000_000;
    ((base * multiplier_basis_points as u128 + 5_000) / 10_000) as u64
}

/// 命中规则下的总计费（微美元）：
/// 计费项：`uncached_input × P_in + output × P_out + cache_read × P_read + cache_write × P_write`
/// 其中 `uncached_input = total_input_tokens.saturating_sub(cache_read).saturating_sub(cache_write)`。
/// 若风格为 `OpenAi`，`P_write` 强制视为 0。
#[must_use]
pub fn calculate_cost_micros(
    rule: &ModelPriceRule,
    total_input_tokens: u64,
    cache_read_input_tokens: u64,
    cache_write_input_tokens: u64,
    output_tokens: u64,
) -> u64 {
    let uncached_input = total_input_tokens
        .saturating_sub(cache_read_input_tokens)
        .saturating_sub(cache_write_input_tokens);
    let p_in = rule.input_price_per_m_micros;
    let p_out = rule.output_price_per_m_micros;
    let p_read = rule.cache_read_price_per_m_micros;
    let p_write = match rule.pricing_style {
        PricingStyle::OpenAi => 0,
        PricingStyle::Claude => rule.cache_write_price_per_m_micros,
    };
    let m = rule.multiplier_basis_points;

    let cost_in = calculate_item_cost_micros(uncached_input, p_in, m);
    let cost_out = calculate_item_cost_micros(output_tokens, p_out, m);
    let cost_read = calculate_item_cost_micros(cache_read_input_tokens, p_read, m);
    let cost_write = calculate_item_cost_micros(cache_write_input_tokens, p_write, m);

    cost_in
        .saturating_add(cost_out)
        .saturating_add(cost_read)
        .saturating_add(cost_write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_semantics() {
        // glob 匹配与大小写不敏感
        assert!(glob_match("gpt-4*", "gpt-4"));
        assert!(glob_match("gpt-4*", "GPT-4O"));
        assert!(glob_match("gpt-4*", "gpt-4-turbo-preview"));
        assert!(!glob_match("gpt-4*", "chatgpt-4")); // 隐式全串锚定 ^gpt-4*$

        // 单字符通配符 ?
        assert!(glob_match("claude-3-?-sonnet", "claude-3-5-sonnet"));
        assert!(!glob_match("claude-3-?-sonnet", "claude-3-55-sonnet"));

        // 空串通配符 *
        assert!(glob_match("*", "any-model-name"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn test_compare_rules_priority_four_tiers() {
        let base_rule = ModelPriceRule {
            id: "rule_b".to_string(),
            model_pattern: "gpt-4*".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 2_500_000,
            output_price_per_m_micros: 10_000_000,
            cache_read_price_per_m_micros: 1_250_000,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };

        // 1. priority 较高者胜出
        let mut high_priority = base_rule.clone();
        high_priority.id = "rule_a".to_string();
        high_priority.priority = 10;
        assert_eq!(
            compare_rules_priority(&high_priority, &base_rule),
            std::cmp::Ordering::Less
        );

        // 2. 字面字符数较长者胜出
        let mut more_literal = base_rule.clone();
        more_literal.id = "rule_c".to_string();
        more_literal.model_pattern = "gpt-4o*".to_string(); // 6 chars vs 5 chars
        assert_eq!(
            compare_rules_priority(&more_literal, &base_rule),
            std::cmp::Ordering::Less
        );

        // 3. updated_at_ms 最新者胜出
        let mut newer = base_rule.clone();
        newer.id = "rule_d".to_string();
        newer.updated_at_ms = 2000;
        assert_eq!(
            compare_rules_priority(&newer, &base_rule),
            std::cmp::Ordering::Less
        );

        // 4. id 字典序最小者胜出
        let mut smaller_id = base_rule.clone();
        smaller_id.id = "rule_a".to_string();
        assert_eq!(
            compare_rules_priority(&smaller_id, &base_rule),
            std::cmp::Ordering::Less
        );

        // match_rule 无兜底规则
        let rules = vec![base_rule];
        assert!(match_rule(&rules, "claude-3-5-sonnet").is_none());
        assert!(match_rule(&rules, "gpt-4o").is_some());
    }

    #[test]
    fn test_uncached_input_invariant() {
        // 自证不变式：total=1000 / read=600 / write=0 -> uncached=400
        let rule = ModelPriceRule {
            id: "rule_test".to_string(),
            model_pattern: "*".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 1_000_000, // $1 / M -> 1 micro / token
            output_price_per_m_micros: 0,
            cache_read_price_per_m_micros: 0,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        // 1000 total, 600 read, 0 write -> uncached 400 * 1_000_000 / 1_000_000 = 400 micros
        let cost = calculate_cost_micros(&rule, 1000, 600, 0, 0);
        assert_eq!(cost, 400);

        // 含 cache write: total=1000, read=400, write=200 -> uncached=400
        let cost2 = calculate_cost_micros(&rule, 1000, 400, 200, 0);
        assert_eq!(cost2, 400);
    }

    #[test]
    fn pricing_style_parses_its_own_serialized_form() {
        // 序列化出来的值必须能被解析回去：编辑既有规则时，界面拿到的正是这一份文本。
        for style in [PricingStyle::OpenAi, PricingStyle::Claude] {
            let serialized = serde_json::to_value(style)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .expect("计费风格应可序列化为字符串");
            assert_eq!(
                PricingStyle::parse(&serialized),
                Some(style),
                "序列化形式 {serialized} 必须可被解析"
            );
        }
        assert_eq!(PricingStyle::parse("openai"), Some(PricingStyle::OpenAi));
        assert_eq!(PricingStyle::parse(" OpenAI "), Some(PricingStyle::OpenAi));
        assert_eq!(PricingStyle::parse("CLAUDE"), Some(PricingStyle::Claude));
        assert_eq!(PricingStyle::parse("unknown"), None);
    }

    #[test]
    fn test_pricing_precision_combinations() {
        // OpenAI 风格组合用例
        let openai_rule = ModelPriceRule {
            id: "openai_rule".to_string(),
            model_pattern: "gpt-4o".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 2_500_000,      // $2.50 / M
            output_price_per_m_micros: 10_000_000,    // $10.00 / M
            cache_read_price_per_m_micros: 1_250_000, // $1.25 / M
            cache_write_price_per_m_micros: 3_750_000, // 即使配置，OpenAI 风格也为 0
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };

        // total=1000, read=200, write=100, out=500
        // uncached = 1000 - 200 - 100 = 700
        // cost_in = (700 * 2_500_000 + 500_000) / 1_000_000 = (1_750_000_000 + 500_000) / 1_000_000 = 1750
        // cost_out = (500 * 10_000_000 + 500_000) / 1_000_000 = 5000
        // cost_read = (200 * 1_250_000 + 500_000) / 1_000_000 = 250
        // cost_write = 0 (OpenAi 风格强制为 0)
        // total = 1750 + 5000 + 250 = 7000
        let cost = calculate_cost_micros(&openai_rule, 1000, 200, 100, 500);
        assert_eq!(cost, 7000);

        // Claude 风格组合用例
        let claude_rule = ModelPriceRule {
            id: "claude_rule".to_string(),
            model_pattern: "claude-3-5-sonnet".to_string(),
            pricing_style: PricingStyle::Claude,
            input_price_per_m_micros: 3_000_000,    // $3.00 / M
            output_price_per_m_micros: 15_000_000,  // $15.00 / M
            cache_read_price_per_m_micros: 300_000, // $0.30 / M
            cache_write_price_per_m_micros: 3_750_000, // $3.75 / M
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };

        // total=1000, read=200, write=100, out=500
        // uncached = 700
        // cost_in = (700 * 3_000_000 + 500_000) / 1_000_000 = 2100
        // cost_out = (500 * 15_000_000 + 500_000) / 1_000_000 = 7500
        // cost_read = (200 * 300_000 + 500_000) / 1_000_000 = 60
        // cost_write = (100 * 3_750_000 + 500_000) / 1_000_000 = 375
        // total = 2100 + 7500 + 60 + 375 = 10035
        let claude_cost = calculate_cost_micros(&claude_rule, 1000, 200, 100, 500);
        assert_eq!(claude_cost, 10035);

        // 倍率测试 multiplier_basis_points = 12000 (1.2x)
        let mut custom_multiplier = claude_rule.clone();
        custom_multiplier.multiplier_basis_points = 12_000;
        // cost_in: base=2100 -> (2100 * 12000 + 5000) / 10000 = 2520
        // cost_out: base=7500 -> (7500 * 12000 + 5000) / 10000 = 9000
        // cost_read: base=60 -> (60 * 12000 + 5000) / 10000 = 72
        // cost_write: base=375 -> (375 * 12000 + 5000) / 10000 = 450
        // total = 2520 + 9000 + 72 + 450 = 12042
        let mult_cost = calculate_cost_micros(&custom_multiplier, 1000, 200, 100, 500);
        assert_eq!(mult_cost, 12042);

        // 极小值舍入为 0 (Some(0))
        // 1 token, 1 micro/M
        let min_rule = ModelPriceRule {
            id: "min".to_string(),
            model_pattern: "*".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 1,
            output_price_per_m_micros: 0,
            cache_read_price_per_m_micros: 0,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        let zero_cost = calculate_cost_micros(&min_rule, 1, 0, 0, 0);
        assert_eq!(zero_cost, 0);

        // 单价为 0
        let free_rule = ModelPriceRule {
            id: "free".to_string(),
            model_pattern: "*".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 0,
            output_price_per_m_micros: 0,
            cache_read_price_per_m_micros: 0,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        let free_cost = calculate_cost_micros(&free_rule, 10000, 5000, 0, 2000);
        assert_eq!(free_cost, 0);
    }
}
