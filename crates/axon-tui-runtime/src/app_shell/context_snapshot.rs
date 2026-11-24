//! 会话级上下文快照读数：TUI 所有上下文读数的唯一来源。
//!
//! 与服务端一致的口径：快照只由用量帧投影而来，给出当前占用与窗口。TUI 只消费这一份
//! 读数，不再自己按用量帧另算一套，也不再拿「上一次非零值」顶替当前帧的未知（INV-6）。
//!
//! 所有字段可空是刻意的：帧里没有的值必须留空，界面据此显示未知；`None` 与 `0` 不是
//! 同一件事，`0` 只是「观测到零」，`None` 才是「没有观测」。

use crate::r#loop::agent_tui::AgentTuiTokenUsage;

/// 会话级上下文快照读数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct AgentTuiContextSnapshot {
    /// 当前上下文占用 token；未知时为 `None`。
    pub current_context_tokens: Option<u64>,
    /// 模型上下文窗口；帧里带有时才有值。
    pub context_window: Option<u64>,
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
        updated_at_ms: Option<u64>,
        source: Option<String>,
    ) -> Self {
        Self {
            current_context_tokens,
            context_window,
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

    /// 头部剩余段文案：`剩余 748K`。
    ///
    /// 剩余 = 模型窗口减去 Provider 实测占用；占用或窗口任一未知就返回 `None`，
    /// 整段不显示，绝不拿估算值顶替。
    #[must_use]
    pub fn remaining_text(&self, model_window: Option<u64>) -> Option<String> {
        let remaining = self.remaining_tokens(model_window)?;
        Some(format!(
            "剩余 {}",
            AgentTuiTokenUsage::render_token_count(remaining)
        ))
    }

    /// 模型窗口剩余多少 token；占用或窗口未知时为 `None`。
    #[must_use]
    pub fn remaining_tokens(&self, model_window: Option<u64>) -> Option<u64> {
        let used = self.current_context_tokens?;
        let window = self.effective_window(model_window)?;
        Some(window.saturating_sub(used))
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
        assert_eq!(snapshot.remaining_text(Some(200_000)), None);
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
    fn remaining_is_window_minus_used() {
        let snapshot = AgentTuiContextSnapshot {
            current_context_tokens: Some(60_000),
            context_window: Some(200_000),
            ..AgentTuiContextSnapshot::default()
        };
        // 200K 窗口减去 60K 已用 = 140K。
        assert_eq!(snapshot.remaining_tokens(None), Some(140_000));
        assert_eq!(snapshot.remaining_text(None).as_deref(), Some("剩余 140K"));

        // 已用超过窗口时收敛到 0，不显示负数。
        let over = AgentTuiContextSnapshot {
            current_context_tokens: Some(220_000),
            context_window: Some(200_000),
            ..AgentTuiContextSnapshot::default()
        };
        assert_eq!(over.remaining_tokens(None), Some(0));
    }
}
