//! 会话：主区域首屏、对话过程交互与右侧栏所消费的运行态事实。
//!
//! 与运行态的全部往来都走网关协议，桌面端不链接会话子系统、不直连数据库：
//!
//! - **提交**走网关 HTTP 的交互事件面（[`wire::EVENTS_PATH`]），
//!   `sourceChannel` 取 [`crate::lane_source::DESKTOP_LANE_CHANNEL`]；
//! - **流式**走网关 WebSocket 订阅（[`wire::WS_PATH`]），文本增量、工具过程与
//!   运行指标都从同一条订阅流上取，不用轮询替代；
//! - **历史**走工作台扩展面按车道枚举 Interaction，再逐条订阅重放。
//!
//! 事实源唯一：对话流的事实源是订阅流，桌面端只做投影，不维护第二份可写副本。

/// 待决问答提交作答。
pub mod answer_question;
/// 输入区附件：落地与预览。
pub mod attachment;
/// 订阅命令与流式推送的 IPC 边界。
pub mod commands;
/// 按车道枚举历史 Interaction。
pub mod history;
/// 中断正在运行的主 turn。
pub mod interrupt;
/// 用户手动停止后台作业。
pub mod job_stop;
/// 车道活作业名册：作业行的对账依据。
pub mod lane_jobs;
/// 车道待决提问的对账查询与字段适配。
pub mod question_sync;
/// 会话级读取：子代理清单与任意会话的逐字稿。
pub mod session_read;
/// 车道事件订阅：一条连接并行服务多路订阅，逐帧推送给前端。
pub mod stream;
/// 向正在运行的子代理投递用户消息。
pub mod subagent_message;
/// 交互事件提交。
pub mod submit;
/// 网关协议载荷与常量。
pub mod wire;

pub use attachment::{StagedAttachment, attachment_pick, attachment_preview, attachment_stage};
pub use commands::{
    conversation_answer_question, conversation_history, conversation_interrupt,
    conversation_lane_jobs, conversation_lane_questions, conversation_lane_subagents,
    conversation_send_subagent_message, conversation_set_active_lane, conversation_stop_job,
    conversation_submit, conversation_unsubscribe, conversation_watch, conversation_watch_session,
};
pub use history::InteractionSummary;
pub use interrupt::InterruptReceipt;
pub use job_stop::JobStopReceipt;
pub use lane_jobs::LaneJobsRoster;
pub use question_sync::QuestionSnapshot;
pub use session_read::LaneSubagents;
pub use stream::{ConversationStream, SubscribeTarget};
pub use subagent_message::SubagentMessageReceipt;
pub use wire::SubmissionReceipt;
