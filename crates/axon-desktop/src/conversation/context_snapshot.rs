//! 会话级上下文快照读取。
//!
//! 头部占用与剩余量读的是同一份快照：它是运行侧带时间戳的唯一来源，未知字段是
//! `null` 而不是 0。桌面端只做适配与展示：剩余量按「窗口 − 占用」派生，窗口优先取
//! 快照实测，缺失时回退到当前生效模型在配置目录里的窗口。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::SESSIONS_PATH;
use crate::gateway;

/// 上下文构成估算：系统提示词、工具定义与对话消息三类 token。
///
/// 每个字段都可缺省：用量帧只报它自己带的那几类，缺的那类就是「未知」，不是 0。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextComposition {
    /// 系统提示词与上下文片段。
    #[serde(default)]
    pub system_prompt_tokens: Option<u64>,
    /// 模型可见的工具定义。
    #[serde(default)]
    pub tool_definition_tokens: Option<u64>,
    /// 真实对话消息（含工具回执）。
    #[serde(default)]
    pub conversation_tokens: Option<u64>,
}

/// 会话上下文快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    /// 当前上下文占用；帧里没有该字段时为空。
    #[serde(default)]
    pub current_context_tokens: Option<u64>,
    /// 模型上下文窗口；用量帧通常不带，因此常为空。
    #[serde(default)]
    pub context_window: Option<u64>,
    /// 上下文构成估算。
    #[serde(default)]
    pub composition: Option<ContextComposition>,
    /// 这份读数最后一次被写入的时刻（Unix 毫秒）。
    #[serde(default)]
    pub updated_at_ms: Option<u64>,
    /// 写入这份读数的帧类别（目前只有 `usage_frame`）。
    #[serde(default)]
    pub source: Option<String>,
}

/// 快照读取回执。
///
/// `snapshot` 为空只表示这条会话还没有任何可用读数，与会话不存在（HTTP 404）不同：
/// 后者由网关的错误模型表达，前端据此给出两种不同的说明。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextSnapshot {
    /// 目标会话。
    pub session_id: String,
    /// 快照；没有任何可用读数时为空。
    #[serde(default)]
    pub snapshot: Option<ContextSnapshot>,
}

/// 读取一条会话的上下文快照。
pub fn read_context_snapshot(session_id: &str) -> Result<SessionContextSnapshot, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_string());
    }
    let payload = gateway::get_json_at(&format!("{SESSIONS_PATH}/{session_id}/context-snapshot"))?;
    serde_json::from_value::<SessionContextSnapshot>(payload)
        .map_err(|error| format!("解析上下文快照失败：{error}"))
}

#[cfg(test)]
mod tests {
    /// 空标识在发请求之前就被拒绝。
    #[test]
    fn empty_session_id_is_rejected_before_any_request() {
        assert!(super::read_context_snapshot("  ").is_err());
    }

    /// 未知字段是空而不是 0：帧里没报的读数不得被读成「占用归零」。
    #[test]
    fn missing_fields_stay_unknown_instead_of_zero() {
        let payload = serde_json::json!({
            "sessionId": "session-1",
            "snapshot": {
                "currentContextTokens": 135_917,
                "contextWindow": serde_json::Value::Null,
                "composition": { "conversationTokens": 80_821 },
                "updatedAtMs": 1_760_000_000_000_u64,
                "source": "usage_frame",
            },
        });
        let reading =
            serde_json::from_value::<super::SessionContextSnapshot>(payload).expect("快照应可解析");
        let snapshot = reading.snapshot.expect("快照存在");
        assert_eq!(snapshot.current_context_tokens, Some(135_917));
        assert_eq!(snapshot.context_window, None, "帧里没有窗口就是未知");
        let composition = snapshot.composition.expect("构成存在");
        assert_eq!(composition.conversation_tokens, Some(80_821));
        assert_eq!(
            composition.system_prompt_tokens, None,
            "缺的构成项是未知，不是 0"
        );
    }
}
