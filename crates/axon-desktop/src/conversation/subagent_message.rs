//! 向正在运行的子代理投递一条用户消息。
//!
//! 走工作台扩展面的子代理消息入口：服务端把消息送进目标子代理的 mailbox，
//! 只有那个子代理会在自己的 step 边界读到它，主代理上下文不受影响。
//! 归属与可投递性都由服务端裁决，桌面端只负责把请求送达并如实回报结论。

use serde::{Deserialize, Serialize};

use crate::conversation::wire::LANES_PATH;
use crate::gateway;

/// 投递回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentMessageReceipt {
    /// 目标车道。
    pub lane_id: String,
    /// 目标子代理。
    pub agent_id: String,
    /// 是否已进入目标 mailbox。
    pub delivered: bool,
    /// 目标当前状态标签。
    #[serde(default)]
    pub recipient_status: Option<String>,
}

/// 向某个子代理投递一条用户消息。
pub fn send_subagent_message(
    lane_id: &str,
    agent_id: &str,
    content: &str,
) -> Result<SubagentMessageReceipt, String> {
    let lane_id = lane_id.trim();
    if lane_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let agent_id = agent_id.trim();
    if agent_id.is_empty() {
        return Err("子代理标识不能为空".to_string());
    }
    if content.trim().is_empty() {
        return Err("消息内容不能为空".to_string());
    }
    let path = format!(
        "{LANES_PATH}/{lane_id}/subagents/{}/messages",
        agent_path_segment(agent_id)
    );
    let payload = gateway::post_json_at(&path, &serde_json::json!({ "content": content }))?;
    serde_json::from_value::<SubagentMessageReceipt>(payload)
        .map_err(|error| format!("解析子代理投递回执失败：{error}"))
}

/// 子代理标识在 URL 里的写法：`/root/x` 这类路径整体编成一个路径段。
fn agent_path_segment(agent_id: &str) -> String {
    agent_id
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace(':', "%3A")
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_inputs_are_rejected_before_any_request() {
        assert!(super::send_subagent_message("  ", "/root/a", "继续").is_err());
        assert!(super::send_subagent_message("session-lane-a", " ", "继续").is_err());
        assert!(super::send_subagent_message("session-lane-a", "/root/a", "   ").is_err());
    }

    #[test]
    fn agent_path_is_encoded_into_one_url_segment() {
        assert_eq!(
            super::agent_path_segment("subagent:/root/worker"),
            "subagent%3A%2Froot%2Fworker"
        );
        assert_eq!(
            super::agent_path_segment("/root/worker"),
            "%2Froot%2Fworker"
        );
    }
}
