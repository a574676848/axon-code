//! 会话相关 IPC 命令。
//!
//! 三个入口分别对应三种真实动作：提交一条消息、读取一条车道的历史索引、
//! 订阅一条 Interaction 的事件流。它们都不在本地留存可写的会话副本。

use tauri::State;

use crate::AppState;
use crate::conversation::answer_question::{self, AnswerQuestionReceipt};
use crate::conversation::history::{self, InteractionSummary};
use crate::conversation::interrupt::{self, InterruptReceipt};
use crate::conversation::job_stop::{self, JobStopReceipt};
use crate::conversation::lane_jobs::{self, LaneJobsRoster};
use crate::conversation::question_sync::{self, QuestionSnapshot};
use crate::conversation::session_read::{self, LaneSubagents};
use crate::conversation::stream::SubscribeTarget;
use crate::conversation::subagent_message::{self, SubagentMessageReceipt};
use crate::conversation::submit;
use crate::conversation::wire::{SubmissionReceipt, UserAnswerItem};

/// Tauri IPC 命令：提交一条用户消息。
///
/// 返回服务端回执；调用方必须按回执里的 Interaction 标识建立订阅，
/// 不能假定它是本次提交新开的那一条（同一车道有活跃回合时服务端会复用）。
#[tauri::command]
pub fn conversation_submit(
    workspace_id: String,
    destination: String,
    content: String,
    attachments: Option<Vec<String>>,
) -> Result<SubmissionReceipt, String> {
    submit::submit_message(
        &workspace_id,
        &destination,
        &content,
        attachments.as_deref().unwrap_or_default(),
    )
}

/// Tauri IPC 命令：提交待决提问的答案。
#[tauri::command]
pub fn conversation_answer_question(
    lane_id: String,
    call_id: String,
    answers: Vec<UserAnswerItem>,
) -> Result<AnswerQuestionReceipt, String> {
    answer_question::answer_question(&lane_id, &call_id, &answers)
}

/// Tauri IPC 命令：列出某条车道下的历史 Interaction。
///
/// `before_ms` 与 `before_id` 一起构成反向分页游标：给出时返回它**之前**的一条，
/// 界面据此逐条向前翻，而不是一次把整条车道的历史读进来再全量渲染。
#[tauri::command]
pub fn conversation_history(
    lane_id: String,
    limit: Option<u32>,
    before_ms: Option<u64>,
    before_id: Option<String>,
) -> Result<Vec<InteractionSummary>, String> {
    let before = match (before_ms, before_id) {
        (Some(received_at_ms), Some(interaction_id)) if !interaction_id.trim().is_empty() => {
            Some(history::HistoryCursor {
                received_at_ms,
                interaction_id,
            })
        }
        _ => None,
    };
    history::lane_interactions(&lane_id, limit, before.as_ref())
}

/// Tauri IPC 命令：读取一条 Interaction 的一段历史帧。
///
/// 历史不再逐帧穿过事件通道：客户端读完这一段后从 `nextAfter` 起订阅实时尾，
/// 事件通道上只出现真正的增量。
#[tauri::command]
pub fn conversation_history_events(
    interaction_id: String,
    after: Option<u64>,
    limit: Option<u32>,
) -> Result<history::InteractionHistoryPage, String> {
    history::interaction_events(&interaction_id, after, limit)
}

/// Tauri IPC 命令：按网关权威记录对账某条车道下的问答状态。
///
/// 返回的每一条都已经完成字段适配（`state` 翻译成界面状态、作答内容取 `answer`、
/// 缺失的 partId 按 callId 派生），前端直接消费即可，不必自己解释网关协议。
/// 失败时返回原因且不产生任何本地副作用：调用方保留本地事实。
#[tauri::command]
pub fn conversation_lane_questions(
    lane_id: String,
    state: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<QuestionSnapshot>, String> {
    question_sync::lane_questions(&lane_id, state.as_deref(), limit)
}

/// Tauri IPC 命令：中断某条车道上正在运行的主 turn。
///
/// 回执里的 `interrupted` 是服务端的真实结论：没有活跃轮次时返回 false，
/// 调用方不得据此假装中断成功。
#[tauri::command]
pub fn conversation_interrupt(lane_id: String) -> Result<InterruptReceipt, String> {
    interrupt::interrupt_lane(&lane_id)
}

/// Tauri IPC 命令：用户手动停止一条车道上的后台作业。
///
/// 归属与终态原因都由服务端裁决；回执里的 `stopped` 是真实结论，
/// 失败时返回的是服务端给出的拒绝原因（例如归属不符）。
#[tauri::command]
pub fn conversation_stop_job(lane_id: String, job_id: String) -> Result<JobStopReceipt, String> {
    job_stop::stop_job(&lane_id, &job_id)
}

/// Tauri IPC 命令：读取一条车道上仍在运行的后台作业名册。
#[tauri::command]
pub fn conversation_lane_jobs(lane_id: String) -> Result<LaneJobsRoster, String> {
    lane_jobs::lane_jobs(&lane_id)
}

/// Tauri IPC 命令：读取一条车道下的子代理会话清单。
#[tauri::command]
pub fn conversation_lane_subagents(lane_id: String) -> Result<LaneSubagents, String> {
    session_read::lane_subagents(&lane_id)
}

/// Tauri IPC 命令：向正在运行的子代理投递一条用户消息。
///
/// 消息只到达目标子代理，主代理上下文不受影响；可投递性由服务端裁决，
/// 拒绝时返回的是服务端给出的原因（例如「该子代理当前不在运行」）。
#[tauri::command]
pub fn conversation_send_subagent_message(
    lane_id: String,
    agent_id: String,
    content: String,
) -> Result<SubagentMessageReceipt, String> {
    subagent_message::send_subagent_message(&lane_id, &agent_id, &content)
}

/// Tauri IPC 命令：登记当前会话，供系统通知使用。
///
/// 通知标题要带会话名，点击后要能回到这条会话；两者都需要一个「用户此刻在哪个
/// 会话里」的事实，而这份事实只有界面知道（会话切换发生在界面里）。车道的展示名
/// 也一并交上来：Rust 侧不重新派生标题，避免同一件事出现两种口径。
#[tauri::command]
pub fn conversation_set_active_lane(lane_id: String, title: String) -> Result<(), String> {
    crate::lane_context::set_active_lane(&lane_id, &title);
    Ok(())
}

/// Tauri IPC 命令：订阅一条 Interaction 的事件流。
///
/// 订阅按目标在进程内唯一的订阅器上登记：车道流与子代理会话流可以并存，
/// 同一个目标重复订阅是幂等的。
#[tauri::command]
pub fn conversation_watch(
    state: State<'_, AppState>,
    workspace_id: String,
    interaction_id: String,
    resume_cursor: Option<String>,
) -> Result<(), String> {
    if interaction_id.trim().is_empty() {
        return Err("Interaction 标识不能为空".to_string());
    }
    if workspace_id.trim().is_empty() {
        return Err("工作区标识不能为空".to_string());
    }
    state.conversation.watch(SubscribeTarget::Interaction {
        workspace_id,
        interaction_id,
        resume_cursor,
    });
    Ok(())
}

/// Tauri IPC 命令：订阅一条**会话**的事件流（子代理会话）。
///
/// 与车道订阅共用同一条连接上的不同订阅，互不干扰；同一个目标重复订阅是幂等的。
#[tauri::command]
pub fn conversation_watch_session(
    state: State<'_, AppState>,
    workspace_id: String,
    session_id: String,
    resume_cursor: Option<String>,
) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("会话标识不能为空".to_string());
    }
    if workspace_id.trim().is_empty() {
        return Err("工作区标识不能为空".to_string());
    }
    state.conversation.watch(SubscribeTarget::Session {
        workspace_id,
        session_id,
        resume_cursor,
    });
    Ok(())
}

/// Tauri IPC 命令：取消一条订阅。
///
/// 必须按**订阅标识**精确取消：一条连接上并存多路订阅，整体关闭会把仍在服务的
/// 那一路一起掐掉（子代理视图与车道视图正是这种并存关系）。
#[tauri::command]
pub fn conversation_unsubscribe(
    state: State<'_, AppState>,
    subscribe_id: String,
) -> Result<(), String> {
    if subscribe_id.trim().is_empty() {
        return Err("订阅标识不能为空".to_string());
    }
    state.conversation.unsubscribe(subscribe_id.trim());
    Ok(())
}
