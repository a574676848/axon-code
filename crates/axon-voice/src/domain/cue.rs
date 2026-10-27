//! 人格化短反馈（cue）的类型。
//!
//! cue 的文本、频次与预算由应用层决定；本模块只冻结种类与规范记号，
//! 供协议帧与状态契约共用。

use crate::constants::CUE_TEXT_MAX_CHARS;

/// cue 种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CueKind {
    /// 收到，用户说完但模型还没出字。
    Ack,
    /// 思考中的轻声提示。
    Thinking,
    /// 被打断后的回执。
    InterruptedAck,
    /// 久别致意。
    Reunion,
    /// 长任务完成。
    Done,
    /// 回合失败的简短回执。
    ErrorAck,
}

impl CueKind {
    /// 全部种类。
    pub const ALL: [Self; 6] = [
        Self::Ack,
        Self::Thinking,
        Self::InterruptedAck,
        Self::Reunion,
        Self::Done,
        Self::ErrorAck,
    ];

    /// 规范记号，与协议 `cue.kind` 一致。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Ack => "ack",
            Self::Thinking => "thinking",
            Self::InterruptedAck => "interrupted_ack",
            Self::Reunion => "reunion",
            Self::Done => "done",
            Self::ErrorAck => "error_ack",
        }
    }

    /// 是否计入「单轮可听 cue ≤2」的预算。
    #[must_use]
    pub const fn counts_toward_budget(self) -> bool {
        !matches!(self, Self::Thinking)
    }

    /// 是否属于寒暄类，受用户情绪与会议模式约束。
    #[must_use]
    pub const fn is_greeting(self) -> bool {
        matches!(self, Self::Reunion)
    }
}

/// cue 文本长度是否合法。
#[must_use]
pub fn cue_text_within_limit(text: &str) -> bool {
    !text.trim().is_empty() && text.chars().count() <= CUE_TEXT_MAX_CHARS
}

#[cfg(test)]
mod tests {
    use super::{CueKind, cue_text_within_limit};
    use std::collections::HashSet;

    #[test]
    fn all_kinds_have_unique_tokens() {
        let tokens: HashSet<&str> = CueKind::ALL.iter().map(|kind| kind.as_token()).collect();
        assert_eq!(tokens.len(), 6);
    }

    #[test]
    fn budget_counts_every_kind_except_thinking() {
        let counted = CueKind::ALL
            .iter()
            .filter(|kind| kind.counts_toward_budget())
            .count();
        assert_eq!(counted, 5);
    }

    #[test]
    fn text_limit_is_enforced() {
        assert!(cue_text_within_limit("好"));
        assert!(!cue_text_within_limit("   "));
        assert!(cue_text_within_limit(&"好".repeat(24)));
        assert!(!cue_text_within_limit(&"好".repeat(25)));
    }
}
