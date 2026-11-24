//! 模型渠道负载均衡领域对象。
//!
//! 参考 new-api 的渠道负载模式：同一模型 id 可由多个上游渠道（provider）服务，
//! 渠道按优先级分组，高优先级组整体先于低优先级组；组内按权重做加权选择实现
//! 负载均衡。在此之上叠加两个正交维度：
//! - 渠道亲和（affinity）：若调用方提供上一次成功的渠道且仍处于同一优先级组，
//!   则在组内固定为首选，保证多轮会话路由稳定、命中上游侧缓存。
//! - 失败重试链（retry chain）：选择结果不是单点，而是一条有序尝试链，首选失败时
//!   依次回退到同组其余渠道、再到更低优先级组，调用方据此逐个重试。
//!
//! 本模块为纯领域对象：不依赖时间、随机数或任何外部框架。加权选择所需的随机源由
//! 调用方以 `selector` 注入（如会话 id 哈希），既可复现又便于单测。

/// 渠道默认优先级，数值越大越优先。
pub const DEFAULT_CHANNEL_PRIORITY: i32 = 0;
/// 渠道默认权重，组内加权选择的相对配额。
pub const DEFAULT_CHANNEL_WEIGHT: u32 = 1;

/// 模型渠道候选。
///
/// 描述「某个上游渠道能服务目标模型 id」这一事实的负载均衡元数据，
/// 不持有 provider/model 业务字段，仅承载排序所需的最小信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelChannelCandidate {
    /// 候选在来源目录中的稳定索引，作为打破平局的最终依据。
    pub index: usize,
    /// 渠道优先级，数值越大越优先。
    pub priority: i32,
    /// 渠道权重，组内加权选择的相对配额；运行时按 `max(weight, 1)` 兜底。
    pub weight: u32,
}

impl ModelChannelCandidate {
    /// 创建渠道候选。
    #[must_use]
    pub const fn new(index: usize, priority: i32, weight: u32) -> Self {
        Self {
            index,
            priority,
            weight,
        }
    }

    /// 参与加权计算的有效权重，恒不小于 1，避免零权重导致整组无法选中。
    #[must_use]
    const fn effective_weight(self) -> u64 {
        if self.weight == 0 {
            1
        } else {
            self.weight as u64
        }
    }
}

/// 渠道选择器：持有同一模型 id 下的全部候选渠道。
#[derive(Debug, Clone, Default)]
pub struct ModelChannelSelection {
    candidates: Vec<ModelChannelCandidate>,
}

/// 渠道尝试计划。
///
/// `order` 按尝试顺序排列候选的 `index`：第 0 项为首选，其余为失败重试回退链。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelChannelPlan {
    order: Vec<usize>,
}

impl ModelChannelPlan {
    /// 完整尝试顺序（候选索引）。
    #[must_use]
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// 首选渠道索引。
    #[must_use]
    pub fn primary(&self) -> Option<usize> {
        self.order.first().copied()
    }

    /// 是否没有任何可用渠道。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

impl ModelChannelSelection {
    /// 以候选列表创建选择器。
    #[must_use]
    pub fn new(candidates: Vec<ModelChannelCandidate>) -> Self {
        Self { candidates }
    }

    /// 是否没有候选渠道。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// 规划渠道尝试链。
    ///
    /// 规则：
    /// 1. 候选按优先级降序分组，高优先级组整体排在低优先级组之前。
    /// 2. 每组内优先保持该组命中的亲和渠道；未命中时用 `selector` 做加权首选。
    /// 3. 亲和不跨越 priority 边界，低优先级渠道只能在高优先级失败后回退。
    ///
    /// `selector` 为调用方注入的加权随机源（如会话 id 哈希），保证可复现。
    #[must_use]
    pub fn plan(&self, selector: u64, affinity: Option<usize>) -> ModelChannelPlan {
        if self.candidates.is_empty() {
            return ModelChannelPlan::default();
        }

        let mut priorities: Vec<i32> = self.candidates.iter().map(|c| c.priority).collect();
        priorities.sort_unstable_by(|left, right| right.cmp(left));
        priorities.dedup();

        let mut order = Vec::with_capacity(self.candidates.len());
        for priority in priorities {
            let group: Vec<ModelChannelCandidate> = self
                .candidates
                .iter()
                .filter(|candidate| candidate.priority == priority)
                .copied()
                .collect();
            order.extend(order_group(&group, selector, affinity));
        }

        ModelChannelPlan { order }
    }
}

/// 组内加权选择：返回命中候选在组内的位置。
fn weighted_pick(group: &[ModelChannelCandidate], selector: u64) -> usize {
    let total: u64 = group
        .iter()
        .map(|candidate| candidate.effective_weight())
        .sum();
    if total == 0 {
        return 0;
    }
    let mut target = selector % total;
    for (position, candidate) in group.iter().enumerate() {
        let weight = candidate.effective_weight();
        if target < weight {
            return position;
        }
        target -= weight;
    }
    group.len() - 1
}

/// 组内排序：亲和命中时以亲和渠道为首选，否则按权重选择；其余按权重降序、索引升序。
fn order_group(
    group: &[ModelChannelCandidate],
    selector: u64,
    affinity: Option<usize>,
) -> Vec<usize> {
    if group.is_empty() {
        return Vec::new();
    }
    let primary_position = affinity
        .and_then(|index| group.iter().position(|candidate| candidate.index == index))
        .unwrap_or_else(|| weighted_pick(group, selector));
    let primary = group[primary_position];

    let mut rest: Vec<ModelChannelCandidate> = group
        .iter()
        .enumerate()
        .filter(|(position, _)| *position != primary_position)
        .map(|(_, candidate)| *candidate)
        .collect();
    rest.sort_by(|left, right| {
        right
            .weight
            .cmp(&left.weight)
            .then_with(|| left.index.cmp(&right.index))
    });

    let mut ordered = Vec::with_capacity(group.len());
    ordered.push(primary.index);
    ordered.extend(rest.iter().map(|candidate| candidate.index));
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(candidates: &[(usize, i32, u32)]) -> ModelChannelSelection {
        ModelChannelSelection::new(
            candidates
                .iter()
                .map(|&(index, priority, weight)| {
                    ModelChannelCandidate::new(index, priority, weight)
                })
                .collect(),
        )
    }

    #[test]
    fn higher_priority_group_precedes_lower() {
        // 渠道 0 优先级低，渠道 1、2 优先级高。
        let selection = selection(&[(0, 0, 100), (1, 10, 1), (2, 10, 1)]);
        let plan = selection.plan(0, None);
        // 高优先级组的两个渠道必须整体排在低优先级渠道之前。
        let low_position = plan.order().iter().position(|&i| i == 0).unwrap();
        assert!(low_position == plan.order().len() - 1);
        assert!(plan.order()[..low_position].contains(&1));
        assert!(plan.order()[..low_position].contains(&2));
    }

    #[test]
    fn weighted_pick_respects_selector_buckets() {
        // 同组：渠道 0 权重 1，渠道 1 权重 3，总权重 4。
        let selection = selection(&[(0, 0, 1), (1, 0, 3)]);
        // selector=0 落在渠道 0 的桶。
        assert_eq!(selection.plan(0, None).primary(), Some(0));
        // selector=1 落入渠道 1 的桶（其覆盖 [1,4)）。
        assert_eq!(selection.plan(1, None).primary(), Some(1));
        assert_eq!(selection.plan(3, None).primary(), Some(1));
        // selector=4 回绕到渠道 0。
        assert_eq!(selection.plan(4, None).primary(), Some(0));
    }

    #[test]
    fn plan_is_full_permutation_without_loss() {
        let selection = selection(&[(0, 5, 2), (1, 5, 1), (2, 1, 9)]);
        let plan = selection.plan(7, None);
        let mut seen = plan.order().to_vec();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2]);
    }

    #[test]
    fn affinity_stays_within_its_priority_group() {
        let selection = selection(&[(0, 10, 5), (1, 10, 5), (2, 0, 1)]);
        let plan = selection.plan(0, Some(2));

        assert_ne!(plan.primary(), Some(2));
        assert_eq!(plan.order().last(), Some(&2));
        assert_eq!(plan.order().len(), 3);
    }

    #[test]
    fn affinity_pins_candidate_within_same_priority_group() {
        let selection = selection(&[(0, 10, 5), (1, 10, 5), (2, 0, 1)]);
        let plan = selection.plan(0, Some(1));

        assert_eq!(plan.primary(), Some(1));
        assert_eq!(plan.order().last(), Some(&2));
    }

    #[test]
    fn affinity_miss_is_ignored() {
        let selection = selection(&[(0, 0, 1), (1, 0, 1)]);
        // 亲和索引不存在于候选中时忽略，不影响正常排序。
        let plan = selection.plan(0, Some(99));
        assert_eq!(plan.order().len(), 2);
        assert_eq!(plan.primary(), Some(0));
    }

    #[test]
    fn zero_weight_still_participates_as_fallback() {
        let selection = selection(&[(0, 0, 0), (1, 0, 0)]);
        let plan = selection.plan(0, None);
        assert_eq!(plan.order().len(), 2);
    }
}
