//! TUI token usage 观测模型。

use serde::Serialize;

use crate::r#loop::agent_tui::display_units::format_decimal_count;

/// Transcript usage part。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiUsageLine {
    /// 稳定 usage part id。
    pub id: String,
    /// token usage 快照。
    pub usage: AgentTuiTokenUsage,
}

/// TUI token usage 快照。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiTokenUsage {
    /// 输入 token。
    pub input_tokens: u64,
    /// cache read 输入 token。
    pub cache_read_input_tokens: u64,
    /// cache write 输入 token。
    pub cache_write_input_tokens: u64,
    /// 输出 token。
    pub output_tokens: u64,
    /// reasoning token。
    pub reasoning_output_tokens: u64,
    /// provider 观测到的当前上下文 token。
    pub current_context_tokens: u64,
}

impl AgentTuiTokenUsage {
    /// 创建 token usage 快照。
    #[must_use]
    pub const fn new(
        input_tokens: u64,
        cache_read_input_tokens: u64,
        cache_write_input_tokens: u64,
        output_tokens: u64,
        reasoning_output_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            cache_read_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            current_context_tokens: 0,
        }
    }

    /// 设置 provider 观测到的当前上下文 token。
    #[must_use]
    pub const fn with_current_context_tokens(mut self, current_context_tokens: u64) -> Self {
        self.current_context_tokens = current_context_tokens;
        self
    }

    /// 当前上下文 token 读数；`0` 表示这一帧没有带来读数（未知）。
    ///
    /// 这里不做任何回退：用量帧没有携带上下文占用时就是未知，既不能拿归一化总量冒充，
    /// 也不能沿用上一次的非零值。界面据此显示未知（INV-6）。
    #[must_use]
    pub fn current_context_tokens(self) -> Option<u64> {
        (self.current_context_tokens > 0).then_some(self.current_context_tokens)
    }

    /// 计算总 token。
    #[must_use]
    pub const fn total_tokens(self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_write_input_tokens)
            .saturating_add(self.output_tokens)
            .saturating_add(self.reasoning_output_tokens)
    }

    /// 计算缓存 token 总量。
    #[must_use]
    pub const fn cache_tokens(self) -> u64 {
        self.cache_read_input_tokens
            .saturating_add(self.cache_write_input_tokens)
    }

    /// 完整输入 token，包含非缓存输入、缓存读取和缓存写入。
    #[must_use]
    pub const fn complete_input_tokens(self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_write_input_tokens)
    }

    /// 缓存读取占完整输入的命中率，单位为 basis points。
    #[must_use]
    pub fn cache_hit_rate_basis_points(self) -> u16 {
        axon_domain::ratio_basis_points(self.cache_read_input_tokens, self.complete_input_tokens())
    }

    /// 是否包含有效 token 观测。
    #[must_use]
    pub const fn has_tokens(self) -> bool {
        self.total_tokens() > 0
    }

    /// 累加另一个 usage 快照。
    ///
    /// 累加量相加；当前上下文占用是「最近一次请求」的读数，按最后一条帧覆盖，
    /// 新帧没有读数时置为未知，而不是保留旧值。
    pub fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(other.cache_write_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
        self.current_context_tokens = other.current_context_tokens;
    }

    /// 本轮读数：已收口流的累计加上在跑流的快照。
    ///
    /// 累加量相加；当前上下文占用这类「最近一次请求」的读数以在跑流为准，
    /// 在跑流没有读数时就是未知。
    #[must_use]
    pub fn fold(self, stream: Self) -> Self {
        let mut merged = self;
        merged.add_assign(stream);
        merged
    }

    /// 渲染为短 usage 文案。
    #[must_use]
    pub fn render_text(self) -> String {
        format!("用量 {}", self.render_total())
    }

    /// 渲染总 token。
    #[must_use]
    pub fn render_total(self) -> String {
        Self::render_token_count(self.total_tokens())
    }

    /// 渲染输入 token。
    #[must_use]
    pub fn render_input(self) -> String {
        Self::render_token_count(self.input_tokens)
    }

    /// 渲染 cache read token。
    #[must_use]
    pub fn render_cache_read_input(self) -> String {
        Self::render_token_count(self.cache_read_input_tokens)
    }

    /// 渲染 cache write token。
    #[must_use]
    pub fn render_cache_write_input(self) -> String {
        Self::render_token_count(self.cache_write_input_tokens)
    }

    /// 渲染输出 token。
    #[must_use]
    pub fn render_output(self) -> String {
        Self::render_token_count(self.output_tokens)
    }

    /// 渲染 reasoning token。
    #[must_use]
    pub fn render_reasoning_output(self) -> String {
        Self::render_token_count(self.reasoning_output_tokens)
    }

    /// 渲染任意 token 数值。
    #[must_use]
    pub fn render_token_count(value: u64) -> String {
        format_decimal_count(value)
    }
}

impl AgentTuiUsageLine {
    /// 创建带稳定 id 的 transcript usage part。
    #[must_use]
    pub fn with_id(id: impl Into<String>, usage: AgentTuiTokenUsage) -> Self {
        Self {
            id: id.into(),
            usage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AgentTuiTokenUsage;

    #[test]
    fn usage_text_uses_compact_units() {
        assert_eq!(
            AgentTuiTokenUsage::new(5_009, 0, 0, 104, 0).render_text(),
            "用量 5.11K"
        );
        assert_eq!(
            AgentTuiTokenUsage::new(1_000_000, 0, 0, 0, 0).render_total(),
            "1M"
        );
        assert_eq!(AgentTuiTokenUsage::render_token_count(1_000_000), "1M");
        assert_eq!(AgentTuiTokenUsage::render_token_count(1_000_000_000), "1B");
    }
}
