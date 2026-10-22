//! Daemon IPC 的稳定命令、事件和 JSON envelope。

use crate::goal::{GoalStartRequest, GoalToolRequest};
use crate::interaction::{EnqueueDeliveryRequest, SubmitInteractionRequest};
use axon_domain::{ActivityQuery, ConsumerCursor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 当前 Daemon IPC 协议版本。
pub const DAEMON_PROTOCOL_VERSION: u16 = 1;

/// 客户端发送给 Daemon 的命令。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "command", content = "payload")]
pub enum DaemonCommand {
    /// 提交统一自然语言 Interaction。
    SubmitInteraction(SubmitInteractionRequest),
    /// 查询 Interaction 的 durable 当前状态。
    GetInteraction {
        /// 目标 Interaction ID。
        interaction_id: String,
    },
    /// 查询作用域内的活动工作快照（Activity）。
    QueryActivity(ActivityQuery),
    /// 执行主 turn Goal 工具命令；查询与写入统一由 Daemon 在权威事实源完成。
    GoalTool(GoalToolRequest),
    /// 启动已有 Goal；只允许受控应用入口调用。
    StartGoal(GoalStartRequest),
    /// attach 并请求 snapshot/replay。
    Attach {
        /// 目标 Session ID。
        session_id: String,
        /// 上次 attach 的 cursor。
        resume_cursor: Option<ConsumerCursor>,
        /// 客户端能力列表。
        capabilities: Vec<String>,
    },
    /// 订阅 Session 的实时事件；Daemon 会先注册订阅，再从 cursor 补放 durable event。
    SubscribeEvents {
        /// 目标 Session ID。
        session_id: String,
        /// 用于观察 durable 终态的 Interaction ID。
        interaction_id: String,
        /// 客户端最后确认的 durable cursor。
        resume_cursor: Option<ConsumerCursor>,
    },
    /// 释放连接，不影响 Worker。
    Detach {
        /// 目标 Session ID。
        session_id: String,
    },
    /// 创建或更新一个 interval schedule。
    UpsertSchedule {
        /// schedule 稳定 ID。
        schedule_id: String,
        /// 可选目标 Session。
        session_id: Option<String>,
        /// 调度 Action 类型。
        kind: String,
        /// 触发间隔（毫秒）。
        interval_ms: u64,
        /// 投递模式。
        delivery_mode: String,
        /// 调度 Action JSON payload。
        payload: Value,
    },
    /// 获取当前 Session 状态。
    GetState {
        /// 目标 Session ID。
        session_id: String,
    },
    /// 查询 Daemon 内部健康指标；不需要 Session ID。
    GetHealth,
    /// 注册客户端投递 presence，声明该客户端可接收的渠道与目标。
    RegisterClient {
        /// 稳定客户端 ID。
        client_id: String,
        /// 目标 Workspace ID。
        workspace_id: String,
        /// 投递渠道，例如 `tui`。
        channel: String,
        /// 客户端在其渠道内的目标标识。
        destination: String,
    },
    /// 回答指定待决提问交互。
    AnswerQuestion {
        /// 所属车道 ID。
        lane_id: String,
        /// 问题 tool_call_id 或 question_id。
        call_id: String,
        /// 用户作答条目列表。
        answers: Vec<axon_domain::UserAnswerItem>,
        /// 作答渠道（例如 "tui" / "desktop" / "feishu"）。
        channel: String,
    },
    /// 注销客户端 presence；后续完成通知不再投递到该客户端。
    UnregisterClient {
        /// 稳定客户端 ID。
        client_id: String,
    },
    /// 拉取该客户端的待投递完成通知，并顺带续期 presence。
    PollNotifications {
        /// 稳定客户端 ID。
        client_id: String,
        /// 单次拉取上限。
        limit: u32,
    },
    /// 确认完成通知已送达客户端，允许 Daemon 确认对应 Result。
    AcknowledgeNotifications {
        /// 稳定客户端 ID。
        client_id: String,
        /// 已确认送达的通知 ID。
        notification_ids: Vec<String>,
    },
    /// 提交一次完成通知派发请求。
    EnqueueDelivery(EnqueueDeliveryRequest),
    /// 请求 Host 进入排空并 handoff。
    Drain {
        /// 排空原因。
        reason: String,
        /// replacement 目标版本。
        target_version: String,
    },
    /// 强制重新探测 MCP 能力：清除健康负缓存并作废、重建该 Workspace 的启动快照。
    ///
    /// 由 Host 运行器的 IPC 层直接处理：MCP 探测与启动快照缓存都是宿主进程资源，
    /// 不进入 `DaemonHost` 的命令分发。
    RefreshMcpCapabilities {
        /// 目标 Workspace 工作目录。
        cwd: String,
    },
}

/// Daemon 推送给客户端的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event", content = "payload")]
pub enum DaemonEvent {
    /// Session 已 attach。
    SessionAttached {
        /// 目标 Session ID。
        session_id: String,
    },
    /// Session 状态变化。
    SessionState {
        /// 目标 Session ID。
        session_id: String,
        /// 新状态。
        status: String,
    },
    /// Agent turn 增量输出。
    TurnDelta {
        /// 目标 Session ID。
        session_id: String,
        /// 增量文本。
        delta: String,
    },
    /// Snapshot 开始。
    SnapshotBegin {
        /// 目标 Session ID。
        session_id: String,
        /// 分块总数。
        total_chunks: u32,
    },
    /// Snapshot 分块。
    SnapshotChunk {
        /// 目标 Session ID。
        session_id: String,
        /// 分块序号。
        index: u32,
        /// 分块 payload。
        payload: Value,
    },
    /// Snapshot 结束并返回最新 cursor。
    SnapshotEnd {
        /// 目标 Session ID。
        session_id: String,
        /// 最新事件 cursor。
        cursor: ConsumerCursor,
    },
    /// 客户端已 detach。
    SessionDetached {
        /// 目标 Session ID。
        session_id: String,
    },
    /// Session 已关闭。
    SessionClosed {
        /// 目标 Session ID。
        session_id: String,
        /// 关闭原因。
        reason: String,
    },
    /// cursor 不可 replay，客户端必须重新 snapshot。
    SnapshotRequired {
        /// 目标 Session ID。
        session_id: String,
        /// 需要重新 snapshot 的原因。
        reason: String,
    },
    /// Host 因升级关闭，客户端应自动重连。
    DaemonClosing {
        /// 关闭原因。
        reason: String,
        /// 是否应自动重连。
        reconnect: bool,
    },
}

/// JSONL 请求 envelope。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonRequest {
    /// envelope 类型。
    pub message_type: String,
    /// 请求幂等 ID。
    pub request_id: String,
    /// 协议版本。
    pub protocol_version: u16,
    /// 客户端 ID。
    pub client_id: String,
    /// 关联 Session。
    pub session_id: Option<String>,
    /// 命令正文。
    pub command: DaemonCommand,
}

/// JSONL 响应 envelope。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonResponse {
    /// 请求幂等 ID。
    pub request_id: String,
    /// 协议版本。
    pub protocol_version: u16,
    /// 是否成功。
    pub ok: bool,
    /// 事件或错误正文。
    pub body: Value,
    /// 最新 cursor。
    pub cursor: Option<ConsumerCursor>,
}

impl DaemonResponse {
    /// 创建成功响应。
    pub fn success(request_id: impl Into<String>, body: Value) -> Self {
        Self {
            request_id: request_id.into(),
            protocol_version: DAEMON_PROTOCOL_VERSION,
            ok: true,
            body,
            cursor: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_INTERVAL_MS: u64 = 60_000;

    #[test]
    fn upsert_schedule_command_round_trips_through_json() {
        let command = DaemonCommand::UpsertSchedule {
            schedule_id: "heartbeat-session-a".to_owned(),
            session_id: Some("session-a".to_owned()),
            kind: "heartbeat".to_owned(),
            interval_ms: TEST_INTERVAL_MS,
            delivery_mode: "follow_up".to_owned(),
            payload: serde_json::json!({"prompt": "检查进度"}),
        };
        let json = serde_json::to_string(&command).expect("命令应可序列化");
        let decoded: DaemonCommand = serde_json::from_str(&json).expect("命令应可反序列化");
        assert!(matches!(
            decoded,
            DaemonCommand::UpsertSchedule {
                interval_ms: TEST_INTERVAL_MS,
                ..
            }
        ));
    }

    #[test]
    fn subscribe_events_command_round_trips_with_resume_cursor() {
        let command = DaemonCommand::SubscribeEvents {
            session_id: "session-a".to_owned(),
            interaction_id: "interaction-a".to_owned(),
            resume_cursor: Some(ConsumerCursor::new("generation-a", 7)),
        };

        let json = serde_json::to_string(&command).expect("命令应可序列化");
        let decoded: DaemonCommand = serde_json::from_str(&json).expect("命令应可反序列化");

        assert!(matches!(
            decoded,
            DaemonCommand::SubscribeEvents {
                resume_cursor: Some(ConsumerCursor { sequence: 7, .. }),
                ..
            }
        ));
    }

    #[test]
    fn refresh_mcp_capabilities_command_round_trips_through_json() {
        let command = DaemonCommand::RefreshMcpCapabilities {
            cwd: "E:\\workspace".to_owned(),
        };

        let json = serde_json::to_string(&command).expect("命令应可序列化");
        let decoded: DaemonCommand = serde_json::from_str(&json).expect("命令应可反序列化");

        assert!(matches!(
            decoded,
            DaemonCommand::RefreshMcpCapabilities { cwd } if cwd == "E:\\workspace"
        ));
    }

    #[test]
    fn query_activity_command_round_trips_through_json() {
        let command = DaemonCommand::QueryActivity(ActivityQuery {
            scope: axon_domain::ScopeRef::workspace("workspace-a").expect("Scope 有效"),
            goal_limit: 10,
            result_limit: 5,
            event_after_sequence: 12,
            event_limit: 20,
        });

        let json = serde_json::to_string(&command).expect("命令应可序列化");
        let decoded: DaemonCommand = serde_json::from_str(&json).expect("命令应可反序列化");

        assert!(matches!(
            decoded,
            DaemonCommand::QueryActivity(ActivityQuery {
                goal_limit: 10,
                result_limit: 5,
                event_after_sequence: 12,
                event_limit: 20,
                ..
            })
        ));
    }

    #[test]
    fn goal_tool_command_round_trips_through_json() {
        use crate::goal::{GoalToolCommand, GoalToolRequest};
        use axon_domain::{DisclosurePolicy, GoalListQuery, GoalQueryScope};

        let command = DaemonCommand::GoalTool(GoalToolRequest {
            actor: "tui".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            idempotency_key: "call-1".to_owned(),
            source: Some(crate::goal::GoalSource::OneOff),
            disclosure_policy: Some(DisclosurePolicy::PrivateOnly),
            transport_context: None,
            command: GoalToolCommand::List(GoalListQuery {
                scope: GoalQueryScope::CurrentContext,
                statuses: Vec::new(),
                limit: 20,
                cursor: None,
            }),
        });

        let json = serde_json::to_string(&command).expect("命令应可序列化");
        let decoded: DaemonCommand = serde_json::from_str(&json).expect("命令应可反序列化");

        match decoded {
            DaemonCommand::GoalTool(request) => {
                assert_eq!(request.workspace_id.as_deref(), Some("workspace-a"));
                assert!(matches!(
                    request.command,
                    GoalToolCommand::List(GoalListQuery {
                        scope: GoalQueryScope::CurrentContext,
                        limit: 20,
                        ..
                    })
                ));
            }
            other => panic!("期望 GoalTool 命令，实际为 {other:?}"),
        }
    }
}
