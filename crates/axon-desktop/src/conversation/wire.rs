//! 网关对话协议载荷。
//!
//! 这里的字段名与序列化形态**逐字段对齐后端源码**，不做任何本地改名：
//! 提交信封是 camelCase，而服务端 WebSocket 事件帧的 `event_type` /
//! `payload_json` 是 snake_case（后端 `EventMessage` 只给 `type` 做了重命名）。
//! 协议改名属于后端契约变更，客户端只能如实跟随。

use serde::{Deserialize, Serialize};

/// 交互事件提交路径。
pub const EVENTS_PATH: &str = "/gateway/v1/events";
/// 会话事件 WebSocket 路径。
pub const WS_PATH: &str = "/gateway/v1/ws";
/// 车道 Interaction 枚举路径前缀。
pub const LANES_PATH: &str = "/api/v1/lanes";
/// 工作台会话面前缀：按会话标识读取逐字稿。
pub const SESSIONS_PATH: &str = "/api/v1/sessions";
/// 工作台交互面前缀：按交互标识读取历史帧页。
pub const INTERACTIONS_PATH: &str = "/api/v1/interactions";

/// 提交信封。
///
/// 与后端 `HttpEventRequest` 一一对应：`workspaceId` 是显式工作区归属，
/// 缺省时后端会回落到它自己的默认工作区，因此桌面端必须填写，
/// 否则会话会落到与用户所选无关的工作区里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSubmission {
    /// 来源渠道。
    pub source_channel: String,
    /// 客户端生成的稳定幂等键。
    pub event_id: String,
    /// 事件所属 Workspace。
    pub workspace_id: String,
    /// 观测时间（毫秒）。
    pub observed_at_ms: u64,
    /// 失效时间（毫秒）。
    pub expires_at_ms: u64,
    /// 正文。
    pub content: String,
    /// 回复渠道，必须与来源渠道一致。
    pub reply_channel: String,
    /// 回复目标，即车道 destination。
    pub reply_destination: String,
    /// 已落地到磁盘的本地附件绝对路径。
    pub local_files: Vec<String>,
}

/// 提交回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionReceipt {
    /// Interaction 标识；客户端无法预知，只能取自回执。
    pub interaction_id: String,
    /// 是否成功写入或命中已有记录。
    #[serde(default)]
    pub accepted: bool,
    /// 是否未新增 durable 记录：同一幂等键重复提交，或输入被注入到同车道正在运行的回合。
    #[serde(default)]
    pub deduplicated: bool,
}

/// WebSocket 客户端请求帧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientFrame {
    /// 建立一条订阅。
    ///
    /// 变体级 `rename_all` 只作用于标签，结构体字段必须单独声明，
    /// 否则同一协议会出现两套命名。
    #[serde(rename_all = "camelCase")]
    Subscribe {
        /// 单连接内唯一的订阅标识；服务端在确认帧、事件帧与订阅级错误帧上原样回显。
        subscribe_id: String,
        /// 目标所属 Workspace，必须与目标的 scope 完全一致。
        workspace_id: String,
        /// 目标 Interaction；会话级订阅（子代理会话）时不带该字段。
        #[serde(skip_serializing_if = "Option::is_none")]
        interaction_id: Option<String>,
        /// 目标会话；给出时按**会话级订阅**处理，不看 Interaction。
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// 断点续订游标；缺省表示从序号 0 全量重放。
        #[serde(skip_serializing_if = "Option::is_none")]
        resume_cursor: Option<String>,
    },
    /// 取消一条订阅；只影响该标识对应的那一路。
    #[serde(rename_all = "camelCase")]
    Unsubscribe {
        /// 要取消的订阅标识。
        subscribe_id: String,
    },
    /// 保活：服务端不主动发心跳，长时间静默的连接由客户端自行维持。
    Ping {},
}

/// WebSocket 服务端帧。
///
/// 只给 `subscribeId` 单独改名：服务端事件帧的 `event_type` / `payload_json`
/// 是既有 snake_case，协议改名属于后端契约变更，客户端只能如实跟随。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
pub enum ServerFrame {
    /// 订阅已建立，随后开始重放与实时转发。
    #[serde(rename = "subscribed")]
    Subscribed {
        /// 被确认的订阅标识；服务端一律回显。
        #[serde(default, rename = "subscribeId")]
        subscribe_id: Option<String>,
    },
    /// 订阅流上的一条事件。
    #[serde(rename = "event")]
    Event {
        /// 该帧属于哪一路订阅；服务端一律回显。
        #[serde(default, rename = "subscribeId")]
        subscribe_id: Option<String>,
        /// 事件类型：durable 会话事件名，或 `interaction.status` / `runtime_observation`。
        event_type: String,
        /// 事件载荷：服务端直接内嵌的 JSON 对象。
        payload: serde_json::Value,
        /// durable 游标，用于断点续订；非 durable 事件为 null。
        #[serde(default)]
        cursor: Option<String>,
        /// 历史回放标记；旧网关缺省时保守地关闭通知。
        #[serde(default = "default_replay")]
        replay: bool,
    },
    /// 取消订阅已完成回执。
    #[serde(rename = "unsubscribed")]
    Unsubscribed {
        /// 被取消的订阅标识。
        #[serde(default, rename = "subscribeId")]
        subscribe_id: Option<String>,
    },
    /// 保活应答。
    #[serde(rename = "pong")]
    Pong,
    /// 服务端错误帧。
    #[serde(rename = "error")]
    Error {
        /// 错误码。
        error: String,
        /// 订阅级错误会带上出错的订阅标识；连接级错误（例如 `invalid_json`）不带。
        #[serde(default, rename = "subscribeId")]
        subscribe_id: Option<String>,
    },
    /// 其余帧类型（例如提交回执）在订阅连接上不参与投影。
    #[serde(other)]
    Other,
}

/// 推送给前端的会话流消息。
///
/// 一条 Tauri 事件通道承载全部订阅的帧，因此每条都带 `subscribeId`：
/// 前端按它过滤分发，属于别的目标的帧直接丢弃。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StreamEvent {
    /// 订阅已建立。
    #[serde(rename_all = "camelCase")]
    Subscribed {
        /// 被确认的订阅标识。
        subscribe_id: String,
    },
    /// 订阅流上的一条事件。
    #[serde(rename_all = "camelCase")]
    Frame {
        /// 该帧属于哪一路订阅。
        subscribe_id: String,
        /// 事件类型。
        event_type: String,
        /// 事件载荷，JSON 文本。
        payload_json: String,
        /// durable 游标。
        cursor: Option<String>,
        /// 是否来自历史回放。
        replay: bool,
    },
    /// 订阅失败或断流。
    #[serde(rename_all = "camelCase")]
    Failure {
        /// 出问题的订阅标识。
        subscribe_id: String,
        /// 可读原因。
        message: String,
        /// 是否属于可重试的传输故障。
        retryable: bool,
    },
}

const fn default_replay() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submission_envelope_uses_camel_case_fields() {
        let value = serde_json::to_value(EventSubmission {
            source_channel: "desktop".to_owned(),
            event_id: "event-1".to_owned(),
            workspace_id: "workspace-a".to_owned(),
            observed_at_ms: 10,
            expires_at_ms: 20,
            content: "你好".to_owned(),
            reply_channel: "desktop".to_owned(),
            reply_destination: "workbench-1".to_owned(),
            local_files: Vec::new(),
        })
        .expect("提交信封应可序列化");
        assert_eq!(value["sourceChannel"], "desktop");
        assert_eq!(value["workspaceId"], "workspace-a");
        assert_eq!(value["replyDestination"], "workbench-1");
        assert!(value["localFiles"].is_array(), "附件字段必须存在且为数组");
    }

    #[test]
    fn subscribe_frame_is_camel_case_and_omits_absent_cursor() {
        let frame = ClientFrame::Subscribe {
            subscribe_id: "interaction:interaction-1".to_owned(),
            workspace_id: "workspace-a".to_owned(),
            interaction_id: Some("interaction-1".to_owned()),
            session_id: None,
            resume_cursor: None,
        };
        let value = serde_json::to_value(frame).expect("订阅帧应可序列化");
        assert_eq!(value["type"], "subscribe");
        assert_eq!(value["subscribeId"], "interaction:interaction-1");
        assert_eq!(value["workspaceId"], "workspace-a");
        assert_eq!(value["interactionId"], "interaction-1");
        assert!(value.get("sessionId").is_none(), "缺省会话不得占位");
        assert!(value.get("resumeCursor").is_none(), "缺省游标不得占位");
    }

    #[test]
    fn lane_subscribe_identifies_the_interaction_never_the_lane() {
        // 车道 API 的 `laneId` 与 Interaction 的 `interactionId` 是两个不同的串，
        // 而订阅与事件回显用的都是后者。曾经前端拿车道标识当订阅标识去过滤帧，
        // 结果是帧全部到达、又被整批丢掉，界面永远停在「生成回复中」。
        let frame = ClientFrame::Subscribe {
            subscribe_id: "interaction:external-interaction-a212f001".to_owned(),
            workspace_id: "workspace-66867af015c61de4".to_owned(),
            interaction_id: Some("external-interaction-a212f001".to_owned()),
            session_id: None,
            resume_cursor: None,
        };
        let value = serde_json::to_value(frame).expect("订阅帧应可序列化");
        assert_eq!(
            value["subscribeId"],
            "interaction:external-interaction-a212f001"
        );
        assert_eq!(value["interactionId"], "external-interaction-a212f001");
        assert!(
            !value["subscribeId"]
                .as_str()
                .unwrap_or_default()
                .contains("session-lane-"),
            "订阅标识不得由车道标识派生：服务端回显的是交互标识，两者不相等就会把帧全丢掉"
        );
    }

    #[test]
    fn unsubscribe_frame_carries_only_the_subscribe_id() {
        let frame = ClientFrame::Unsubscribe {
            subscribe_id: "session:parent--subagent--root".to_owned(),
        };
        let value = serde_json::to_value(frame).expect("取消订阅帧应可序列化");
        assert_eq!(value["type"], "unsubscribe");
        assert_eq!(value["subscribeId"], "session:parent--subagent--root");
        assert_eq!(
            value.as_object().map(serde_json::Map::len),
            Some(2),
            "取消订阅只交标识，不带任何目标字段"
        );
    }

    #[test]
    fn session_subscribe_frame_carries_session_id_and_no_interaction() {
        let frame = ClientFrame::Subscribe {
            subscribe_id: "session:parent--subagent--root_delegate_fast_1".to_owned(),
            workspace_id: "workspace-a".to_owned(),
            interaction_id: None,
            session_id: Some("parent--subagent--root_delegate_fast_1".to_owned()),
            resume_cursor: Some("{\"generation\":\"g1\",\"sequence\":4}".to_owned()),
        };
        let value = serde_json::to_value(frame).expect("订阅帧应可序列化");
        assert_eq!(value["sessionId"], "parent--subagent--root_delegate_fast_1");
        assert!(
            value.get("interactionId").is_none(),
            "会话级订阅不得带 Interaction 占位"
        );
        assert_eq!(
            value["resumeCursor"],
            "{\"generation\":\"g1\",\"sequence\":4}"
        );
    }

    #[test]
    fn server_event_frame_carries_the_payload_as_a_nested_object() {
        let frame = serde_json::from_str::<ServerFrame>(
            r#"{"type":"event","subscribeId":"s1","event_type":"wire_event","payload":{"text":"ok"},"cursor":"c1"}"#,
        )
        .expect("服务端事件帧必须按真实字段名解析");
        assert_eq!(
            frame,
            ServerFrame::Event {
                subscribe_id: Some("s1".to_owned()),
                event_type: "wire_event".to_owned(),
                payload: serde_json::json!({ "text": "ok" }),
                cursor: Some("c1".to_owned()),
                replay: true,
            }
        );
    }

    #[test]
    fn server_subscription_frames_echo_the_subscribe_id() {
        assert_eq!(
            serde_json::from_str::<ServerFrame>(r#"{"type":"subscribed","subscribeId":"s1"}"#)
                .expect("确认帧应可解析"),
            ServerFrame::Subscribed {
                subscribe_id: Some("s1".to_owned())
            }
        );
        assert_eq!(
            serde_json::from_str::<ServerFrame>(
                r#"{"type":"error","error":"subscribe_duplicate_id","subscribeId":"s1"}"#
            )
            .expect("订阅级错误帧应可解析"),
            ServerFrame::Error {
                error: "subscribe_duplicate_id".to_owned(),
                subscribe_id: Some("s1".to_owned()),
            }
        );
        assert_eq!(
            serde_json::from_str::<ServerFrame>(r#"{"type":"error","error":"invalid_json"}"#)
                .expect("连接级错误帧应可解析"),
            ServerFrame::Error {
                error: "invalid_json".to_owned(),
                subscribe_id: None,
            }
        );
    }

    #[test]
    fn unknown_server_frame_tags_do_not_break_the_stream() {
        assert_eq!(
            serde_json::from_str::<ServerFrame>(r#"{"type":"accepted","receipt":{}}"#)
                .expect("未知帧类型应被吸收"),
            ServerFrame::Other
        );
    }
}

/// 用户问题选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionOption {
    /// 选项标签。
    pub label: String,
    /// 选项描述。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 用户问题条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionItem {
    /// 问题 ID。
    pub id: String,
    /// 问题内容。
    pub question: String,
    /// 标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// 细节说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 选项列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<UserQuestionOption>,
    /// 是否允许多选。
    #[serde(default)]
    pub multi_select: bool,
}

/// 单项问题作答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAnswerItem {
    /// 问题 ID。
    pub id: String,
    /// 选中的选项。
    #[serde(default)]
    pub selected: Vec<String>,
    /// 自定义回答。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// 用户问答交互事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireUserQuestion {
    /// 稳定 part ID。
    pub part_id: String,
    /// 工具调用 ID。
    pub call_id: String,
    /// 问答状态。
    pub status: String,
    /// 提出的问题列表。
    pub questions: Vec<UserQuestionItem>,
    /// 作答列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<UserAnswerItem>,
    /// 挂起交接文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// 截止时间毫秒时间戳。
    pub deadline_at_ms: i64,
}
