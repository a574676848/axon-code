//! 会话级上下文快照读数：TUI 所有上下文读数的唯一来源。
//!
//! 与服务端一致的口径：快照由事件日志的两类帧投影而来，用量帧给出当前占用与窗口，
//! 轮转生命周期帧给出换窗前后读数。TUI 只消费这一份读数，不再自己按用量帧另算一套，
//! 也不再拿「上一次非零值」顶替当前帧的未知（INV-6）。
//!
//! 所有字段可空是刻意的：帧里没有的值必须留空，界面据此显示未知；`None` 与 `0` 不是
//! 同一件事，`0` 只是「观测到零」，`None` 才是「没有观测」。

use crate::r#loop::agent_tui::{AgentTuiTokenUsage, display_units};

/// 换窗读数的成对语义：只有前后两侧都有值才是一条可读的换窗记录。
const ROLLOVER_ARROW: &str = "→";

/// 会话级上下文快照读数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct AgentTuiContextSnapshot {
    /// 当前上下文占用 token；未知时为 `None`。
    pub current_context_tokens: Option<u64>,
    /// 模型上下文窗口；帧里带有时才有值。
    pub context_window: Option<u64>,
    /// 最近一次换窗前的上下文读数。
    pub rollover_before_tokens: Option<u64>,
    /// 最近一次换窗后的上下文读数。
    pub rollover_after_tokens: Option<u64>,
    /// 快照最后一次写入的时刻（Unix 毫秒）。
    pub updated_at_ms: Option<u64>,
    /// 写入这份读数的帧类别。
    pub source: Option<String>,
}

impl AgentTuiContextSnapshot {
    /// 创建一份读数快照。
    #[must_use]
    pub const fn new(
        current_context_tokens: Option<u64>,
        context_window: Option<u64>,
        rollover_before_tokens: Option<u64>,
        rollover_after_tokens: Option<u64>,
        updated_at_ms: Option<u64>,
        source: Option<String>,
    ) -> Self {
        Self {
            current_context_tokens,
            context_window,
            rollover_before_tokens,
            rollover_after_tokens,
            updated_at_ms,
            source,
        }
    }

    /// 是否读到过当前占用；没有读数时界面显示未知，不显示 `0`。
    #[must_use]
    pub const fn has_context_reading(&self) -> bool {
        self.current_context_tokens.is_some()
    }

    /// 生效的上下文窗口：快照里的窗口优先，缺失时回落到会话模型的窗口。
    #[must_use]
    pub fn effective_window(&self, model_window: Option<u64>) -> Option<u64> {
        self.context_window
            .or(model_window)
            .filter(|window| *window > 0)
    }

    /// 头部占用段文案：`上下文 136K/200K`。
    ///
    /// 读不到当前占用时返回 `None`，调用方据此整段不显示；窗口缺失时只显示已用值。
    /// `cache_read` / `cache_write` 是本轮最近一次请求的构成，仍由用量帧给出，不属于
    /// 快照读数，因此作为可选后缀拼接。
    #[must_use]
    pub fn context_text(
        &self,
        model_window: Option<u64>,
        cache_read_input_tokens: u64,
        cache_write_input_tokens: u64,
    ) -> Option<String> {
        let used = self.current_context_tokens?;
        let mut text = match self.effective_window(model_window) {
            Some(window) => format!(
                "上下文 {}/{}",
                AgentTuiTokenUsage::render_token_count(used),
                AgentTuiTokenUsage::render_token_count(window)
            ),
            None => format!("上下文 {}", AgentTuiTokenUsage::render_token_count(used)),
        };
        if cache_read_input_tokens > 0 {
            text.push_str(" 缓存 ");
            text.push_str(&AgentTuiTokenUsage::render_token_count(
                cache_read_input_tokens,
            ));
        }
        if cache_write_input_tokens > 0 {
            text.push_str(" 写入 ");
            text.push_str(&AgentTuiTokenUsage::render_token_count(
                cache_write_input_tokens,
            ));
        }
        Some(text)
    }

    /// 头部换窗段文案：`换窗 200K→12K`。
    ///
    /// 换窗读数是成对事实：缺任一侧就返回 `None`，不拿当前占用凑另一半，也不显示只有一个
    /// 数字的箭头。头部与换窗行都从这里取数，两处因此不可能各说一套。
    #[must_use]
    pub fn rollover_text(&self) -> Option<String> {
        let before = self.rollover_before_tokens?;
        let after = self.rollover_after_tokens?;
        Some(format!(
            "换窗 {}{}{}",
            display_units::format_decimal_count(before),
            ROLLOVER_ARROW,
            display_units::format_decimal_count(after)
        ))
    }

    /// 上下文压力百分比；缺少窗口时无法判定，返回 `None`。
    #[must_use]
    pub fn context_pressure_percent(&self, model_window: Option<u64>) -> Option<u64> {
        let used = self.current_context_tokens?;
        let window = self.effective_window(model_window)?;
        Some(used.saturating_mul(100) / window.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::AgentTuiContextSnapshot;

    #[test]
    fn unknown_reading_stays_unknown_instead_of_zero() {
        let snapshot = AgentTuiContextSnapshot::default();
        assert!(!snapshot.has_context_reading());
        assert_eq!(snapshot.context_text(Some(200_000), 0, 0), None);
        assert_eq!(snapshot.context_pressure_percent(Some(200_000)), None);
    }

    #[test]
    fn window_falls_back_to_session_model_only_when_snapshot_lacks_it() {
        let snapshot = AgentTuiContextSnapshot {
            current_context_tokens: Some(135_917),
            ..AgentTuiContextSnapshot::default()
        };
        assert_eq!(snapshot.effective_window(Some(200_000)), Some(200_000));
        assert_eq!(snapshot.effective_window(None), None);
        assert_eq!(
            snapshot.context_text(Some(200_000), 0, 0).as_deref(),
            Some("上下文 136K/200K")
        );
        assert_eq!(
            snapshot.context_text(None, 0, 0).as_deref(),
            Some("上下文 136K")
        );
    }

    #[test]
    fn rollover_reading_requires_both_sides() {
        let before_only = AgentTuiContextSnapshot {
            rollover_before_tokens: Some(200_000),
            ..AgentTuiContextSnapshot::default()
        };
        assert_eq!(before_only.rollover_text(), None);
        let paired = AgentTuiContextSnapshot {
            rollover_before_tokens: Some(200_000),
            rollover_after_tokens: Some(12_345),
            ..AgentTuiContextSnapshot::default()
        };
        assert_eq!(paired.rollover_text().as_deref(), Some("换窗 200K→12.3K"));
    }
}
