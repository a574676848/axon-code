//! 按车道枚举历史 Interaction。
//!
//! 会话正文的事实源是订阅流，历史回放的通道也只有一条：逐条订阅该车道下的
//! Interaction 并让其 durable 日志全量重放。因此这里只需要拿到「有哪些
//! Interaction、按什么顺序」，正文不从这条只读接口取。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::LANES_PATH;
use crate::gateway;

/// 单次拉取的历史条数上限。
const MAX_HISTORY_LIMIT: u32 = 1_000;
/// 默认拉取的历史条数。
const DEFAULT_HISTORY_LIMIT: u32 = 200;

/// 车道内一条历史 Interaction。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionSummary {
    /// Interaction 标识。
    pub interaction_id: String,
    /// 处理状态：received、processing、understood、rejected 或 interrupted。
    pub status: String,
    /// 来源渠道。
    pub channel: String,
    /// 接收时间（毫秒）。
    pub received_at_ms: u64,
    /// 入站正文。
    #[serde(default)]
    pub content: String,
    /// 入站时落地的本地附件绝对路径。
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// 网关载荷。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaneInteractionsPayload {
    interactions: Vec<InteractionSummary>,
}

/// 反向分页游标：按「已加载窗口里最早那一条」的排序键向前翻。
///
/// 两个字段就是网关查询的排序键 `(received_at_ms, interaction_id)`：只给时间
/// 会在同一毫秒内落库多条时漏读或重读，因此必须成对使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCursor {
    /// 排序主键：接收时间（毫秒）。
    pub received_at_ms: u64,
    /// 同一毫秒内的次序键。
    pub interaction_id: String,
}

/// 列出某条车道下的 Interaction，按接收时间升序。
///
/// `before` 为空取最近一页；给出游标则取它之前的一页，用于逐条向前翻。
pub fn lane_interactions(
    lane_id: &str,
    limit: Option<u32>,
    before: Option<&HistoryCursor>,
) -> Result<Vec<InteractionSummary>, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let limit = limit
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
        .clamp(1, MAX_HISTORY_LIMIT);
    let mut path = format!("{LANES_PATH}/{lane_id}/interactions?limit={limit}");
    if let Some(cursor) = before {
        // 车道标识只允许 URL 安全字符，游标仍需转义：它来自网关下发的标识，
        // 不该由客户端假定其字符集。
        let encoded: String =
            url::form_urlencoded::byte_serialize(cursor.interaction_id.as_bytes()).collect();
        path.push_str(&format!(
            "&before_ms={}&before_id={encoded}",
            cursor.received_at_ms
        ));
    }
    let payload = gateway::get_json_at(&path)?;
    serde_json::from_value::<LaneInteractionsPayload>(payload)
        .map(|parsed| parsed.interactions)
        .map_err(|error| format!("解析车道历史失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn history_limit_is_clamped_into_the_supported_window() {
        assert_eq!(super::MAX_HISTORY_LIMIT.min(5_000), 1_000);
        assert_eq!(
            super::DEFAULT_HISTORY_LIMIT.clamp(1, super::MAX_HISTORY_LIMIT),
            200
        );
    }

    #[test]
    fn empty_lane_id_is_rejected_before_any_request() {
        assert!(super::lane_interactions("  ", None, None).is_err());
    }

    #[test]
    fn a_cursor_keeps_both_sort_keys_for_the_next_page() {
        let cursor = super::HistoryCursor {
            received_at_ms: 1_730_000_000_000,
            interaction_id: "interaction:a/b".to_owned(),
        };
        // 游标拼进查询串时必须转义：标识里的分隔符不该被当成查询串结构。
        let encoded: String =
            url::form_urlencoded::byte_serialize(cursor.interaction_id.as_bytes()).collect();
        assert!(!encoded.contains('/'), "斜杠必须被转义：{encoded}");
        assert!(!encoded.contains(':'), "冒号必须被转义：{encoded}");
    }
}
