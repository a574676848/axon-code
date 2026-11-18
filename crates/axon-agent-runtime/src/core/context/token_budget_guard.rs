//! Token Budget 感知与窗口轮转边界策略。
//!
//! 阈值采用 Codex 的“距离自动换窗还剩多少 token”语义，且当前仅由运行时持有，
//! 不进入用户配置、请求参数或 Workspace 稳定前缀。
//!
//! 预警提前量按“模型步数”计量而不是按绝对 token：同一组常量在不同上下文窗口下
//! 必须留出同样多的可执行步数，否则大窗口的提醒会退化成“来不及收尾的通知”。

/// 无步均采样时的预警下限（绝对量兜底，覆盖会话前几步与短窗口）。
pub(crate) const TOKEN_BUDGET_WARN_MIN_TOKENS: u64 = 6_144;
/// 预警提前量占模型上下文窗口的最小比例。
pub(crate) const TOKEN_BUDGET_WARN_WINDOW_RATIO: f64 = 0.03;
/// 预警提前量覆盖的模型步数。
pub(crate) const TOKEN_BUDGET_WARN_STEPS: u64 = 4;
/// 预警提前量相对自动换窗触发线的上限百分比。
///
/// 单步极大时（例如连续读入多个大文件），按步数换算的提前量会超过触发线本身，
/// 预警会退化成“窗口一开就报”。用触发线的一半封顶，保证提醒只在真正进入
/// 后半程之后出现。
pub(crate) const TOKEN_BUDGET_WARN_MAX_TRIGGER_PERCENT: u64 = 50;
/// fallback 后允许完成一次短操作的固定缓冲。
pub(crate) const TOKEN_BUDGET_FALLBACK_BUFFER_TOKENS: u64 = 16_384;
/// 中段保值提示的相对占用（百分比整数，热路径避免浮点比较）。
pub(crate) const TOKEN_BUDGET_MIDWAY_USED_PERCENT: u64 = 55;
/// 距上次便签写入达到该 assistant 步数时发送中段保值提示。
pub(crate) const TOKEN_BUDGET_MIDWAY_STEPS: usize = 8;
/// 步均消耗保留的采样条数。
pub(crate) const TOKEN_BUDGET_PACE_SAMPLES: usize = 3;

/// 模型单步上下文消耗的滚动采样。
///
/// 采样窗口固定为 [`TOKEN_BUDGET_PACE_SAMPLES`]，用定长数组承载以避免热路径堆分配。
/// 取中位数而不是均值：一次大文件读取或一条大工具回执不会把后续所有判断整体抬高。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepPace {
    samples: [u64; TOKEN_BUDGET_PACE_SAMPLES],
    count: usize,
    next_slot: usize,
    last_tokens: Option<u64>,
}

impl StepPace {
    /// 观察一次上下文用量，返回本次增量。
    ///
    /// 用量下降表示刚发生窗口轮转或历史重写：清空采样并只保留新基线。
    /// 用量不变不产生采样，避免空转把中位数拉向 0。
    pub fn observe(&mut self, current_tokens: u64) -> Option<u64> {
        let previous = self.last_tokens.replace(current_tokens)?;
        if current_tokens < previous {
            self.samples = [0; TOKEN_BUDGET_PACE_SAMPLES];
            self.count = 0;
            self.next_slot = 0;
            return None;
        }
        let delta = current_tokens - previous;
        if delta == 0 {
            return None;
        }
        self.samples[self.next_slot] = delta;
        self.next_slot = (self.next_slot + 1) % TOKEN_BUDGET_PACE_SAMPLES;
        self.count = (self.count + 1).min(TOKEN_BUDGET_PACE_SAMPLES);
        Some(delta)
    }

    /// 返回最近若干步的中位消耗；没有采样时返回 `None`。
    #[must_use]
    pub fn tokens_per_step(&self) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let mut ordered = self.samples;
        ordered[..self.count].sort_unstable();
        Some(ordered[self.count / 2])
    }

    /// 返回当前采样条数。
    #[must_use]
    pub const fn sample_count(&self) -> usize {
        self.count
    }

    /// 清空采样与基线。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Token Budget 内部运行时策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenBudgetRuntimePolicy {
    fallback_buffer_tokens: u64,
}

impl Default for TokenBudgetRuntimePolicy {
    fn default() -> Self {
        Self {
            fallback_buffer_tokens: TOKEN_BUDGET_FALLBACK_BUFFER_TOKENS,
        }
    }
}

impl TokenBudgetRuntimePolicy {
    /// 计算距离自动换窗触发线的剩余 token。
    #[must_use]
    pub(crate) const fn remaining_before_rollover(
        trigger_tokens: u64,
        current_context_tokens: u64,
    ) -> u64 {
        trigger_tokens.saturating_sub(current_context_tokens)
    }

    /// 计算预警提前量。
    ///
    /// 取“覆盖若干步的用量”与“窗口比例下限”的较大者：有采样时按模型真实步幅
    /// 提前，没有采样时（会话前几步、进程刚重启）退回窗口比例，短窗口再用绝对下限兜底。
    /// 结果按触发线的一半封顶，避免单步过大时预警在窗口刚开启就触发。
    #[must_use]
    pub(crate) fn warn_headroom(
        trigger_tokens: u64,
        context_window: u64,
        tokens_per_step: Option<u64>,
    ) -> u64 {
        let ratio_floor = ((context_window as f64) * TOKEN_BUDGET_WARN_WINDOW_RATIO).round() as u64;
        let floor = ratio_floor.max(TOKEN_BUDGET_WARN_MIN_TOKENS);
        let by_steps = match tokens_per_step {
            Some(step) if step > 0 => floor.max(step.saturating_mul(TOKEN_BUDGET_WARN_STEPS)),
            _ => floor,
        };
        let cap =
            (trigger_tokens.saturating_mul(TOKEN_BUDGET_WARN_MAX_TRIGGER_PERCENT) / 100).max(floor);
        by_steps.min(cap)
    }

    /// 是否应发送收尾预警；调用方负责同一窗口只发送一次。
    #[must_use]
    pub(crate) const fn should_emit_warn(
        trigger_tokens: u64,
        current_context_tokens: u64,
        headroom: u64,
    ) -> bool {
        current_context_tokens < trigger_tokens
            && trigger_tokens.saturating_sub(current_context_tokens) <= headroom
    }

    /// 是否应发送中段保值提示；调用方负责同一窗口只发送一次。
    ///
    /// 两个触发条件取或：窗口占用过半说明剩余空间不再宽裕，
    /// 距上次便签写入的步数达标说明关键结论已经在对话里放了太久。
    /// 两者都不依赖磁盘读取，全部来自当前窗口的运行态消息。
    #[must_use]
    pub(crate) const fn should_emit_midway(
        trigger_tokens: u64,
        current_context_tokens: u64,
        steps_since_notes: usize,
    ) -> bool {
        current_context_tokens < trigger_tokens
            && (current_context_tokens.saturating_mul(100)
                >= trigger_tokens.saturating_mul(TOKEN_BUDGET_MIDWAY_USED_PERCENT)
                || steps_since_notes >= TOKEN_BUDGET_MIDWAY_STEPS)
    }

    /// fallback 缓冲耗尽时的强制轮转线，不得越过模型完整上下文窗口。
    ///
    /// 缓冲本身是绝对量：它衡量的是“写完便签再调用一次换窗”这一组动作的成本，
    /// 与窗口大小无关，因此按比例缩放的只有预警提前量。
    #[must_use]
    pub(crate) const fn forced_rollover_tokens(
        self,
        trigger_tokens: u64,
        context_window: u64,
    ) -> u64 {
        let buffered_tokens = trigger_tokens.saturating_add(self.fallback_buffer_tokens);
        if buffered_tokens < context_window {
            buffered_tokens
        } else {
            context_window
        }
    }

    /// 达到触发线但尚未耗尽缓冲时，是否应发送一次 fallback。
    #[must_use]
    pub(crate) const fn should_emit_fallback(
        self,
        trigger_tokens: u64,
        context_window: u64,
        current_context_tokens: u64,
        fallback_emitted: bool,
    ) -> bool {
        !fallback_emitted
            && current_context_tokens >= trigger_tokens
            && current_context_tokens < self.forced_rollover_tokens(trigger_tokens, context_window)
    }
}
