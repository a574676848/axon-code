//! 会话相关 IPC 命令。
//!
//! 三个入口分别对应三种真实动作：提交一条消息、读取一条车道的历史索引、
//! 订阅一条 Interaction 的事件流。它们都不在本地留存可写的会话副本。

use tauri::State;

use crate::AppState;
use crate::conversation::answer_question::{self, AnswerQuestionReceipt};
use crate::conversation::context_snapshot::{self, SessionContextSnapshot};
use crate::conversation::history::{self, SessionInteractions};
use crate::conversation::interrupt::{self, InterruptReceipt, SettleSilentTurnReceipt};
use crate::conversation::job_output::{self, JobOutputStream};
use crate::conversation::job_stop::{self, JobStopReceipt};
use crate::conversation::question_sync::{self, QuestionSnapshot};
use crate::conversation::session_jobs::{self, SessionJobsRoster};
use crate::conversation::session_live::{self, SessionLive};
use crate::conversation::session_read::{self, SessionSubagents};
use crate::conversation::stream::SubscribeTarget;
use crate::conversation::subagent_message::{self, SubagentMessageReceipt};
use crate::conversation::submit;
use crate::conversation::wire::{SubmissionReceipt, UserAnswerItem};

/// Tauri IPC 命令：提交一条用户消息。
///
/// 返回服务端回执；调用方必须按回执里的 Interaction 标识建立订阅，
/// 不能假定它是本次提交新开的那一条（同一车道有活跃回合时服务端会复用）。
#[tauri::command(async)]
pub fn conversation_submit(
    workspace_id: String,
    session_id: Option<String>,
    destination: String,
    content: String,
    attachments: Option<Vec<String>>,
    model: Option<String>,
    reasoning_effort: Option<String>,
) -> Result<SubmissionReceipt, String> {
    submit::submit_message(
        &workspace_id,
        session_id.as_deref(),
        &destination,
        &content,
        attachments.as_deref().unwrap_or_default(),
        model.as_deref(),
        reasoning_effort.as_deref(),
    )
}

/// Tauri IPC 命令：提交待决提问的答案。
#[tauri::command(async)]
pub fn conversation_answer_question(
    session_id: String,
    call_id: String,
    answers: Vec<UserAnswerItem>,
) -> Result<AnswerQuestionReceipt, String> {
    answer_question::answer_question(&session_id, &call_id, &answers)
}

/// Tauri IPC 命令：列出某条车道下的历史 Interaction。
///
/// `before_ms` 与 `before_id` 一起构成反向分页游标：给出时返回它**之前**的一条，
/// 界面据此逐条向前翻，而不是一次把整条车道的历史读进来再全量渲染。
/// 回执同时带上整条车道的累计 token 用量：会话读数要的是整条会话，与翻到第几页无关。
#[tauri::command(async)]
pub fn conversation_history(
    session_id: String,
    limit: Option<u32>,
    before_ms: Option<u64>,
    before_id: Option<String>,
) -> Result<SessionInteractions, String> {
    let before = match (before_ms, before_id) {
        (Some(received_at_ms), Some(interaction_id)) if !interaction_id.trim().is_empty() => {
            Some(history::HistoryCursor {
                received_at_ms,
                interaction_id,
            })
        }
        _ => None,
    };
    history::session_interactions(&session_id, limit, before.as_ref())
}

/// Tauri IPC 命令：读取一条 Interaction 的一段历史帧。
///
/// 历史不再逐帧穿过事件通道：客户端读完这一段后从 `nextAfter` 起订阅实时尾，
/// 事件通道上只出现真正的增量。
#[tauri::command(async)]
pub fn conversation_history_events(
    interaction_id: String,
    after: Option<u64>,
    limit: Option<u32>,
    tail: Option<bool>,
) -> Result<history::InteractionHistoryPage, String> {
    history::interaction_events(&interaction_id, after, limit, tail.unwrap_or(false))
}

/// Tauri IPC 命令：按网关权威记录对账某条车道下的问答状态。
///
/// 返回的每一条都已经完成字段适配（`state` 翻译成界面状态、作答内容取 `answer`、
/// 缺失的 partId 按 callId 派生），前端直接消费即可，不必自己解释网关协议。
/// 失败时返回原因且不产生任何本地副作用：调用方保留本地事实。
///
/// 不带状态过滤：对账必须看到全部状态，只查其一会把另一种误判成已结算。
#[tauri::command(async)]
pub fn conversation_session_questions(
    session_id: String,
    limit: Option<u32>,
) -> Result<Vec<QuestionSnapshot>, String> {
    question_sync::session_questions(&session_id, limit)
}

/// Tauri IPC 命令：中断某条车道上正在运行的主 turn。
///
/// 回执里的 `interrupted` 是服务端的真实结论：没有活跃轮次时返回 false，
/// 调用方不得据此假装中断成功。
#[tauri::command(async)]
pub fn conversation_interrupt(session_id: String) -> Result<InterruptReceipt, String> {
    interrupt::interrupt_session(&session_id)
}

/// Tauri IPC 命令：请服务端显式收口一条静默轮次。
///
/// 索引仍说在途、服务端却已没有运行轮次时，这是唯一的收口路径：界面不得自己
/// 改状态。回执里的 `verdict` 是服务端判据，文案由调用方按它给。
#[tauri::command(async)]
pub fn conversation_settle_silent_turn(
    session_id: String,
    interaction_id: String,
) -> Result<SettleSilentTurnReceipt, String> {
    interrupt::settle_silent_turn(&session_id, &interaction_id)
}

/// Tauri IPC 命令：读取一条会话的上下文快照。
///
/// 头部占用与剩余量读这一份；`snapshot` 为空表示还没有任何可用读数，
/// 与会话不存在（读取失败）严格区分。
#[tauri::command(async)]
pub fn conversation_context_snapshot(session_id: String) -> Result<SessionContextSnapshot, String> {
    context_snapshot::read_context_snapshot(&session_id)
}

/// Tauri IPC 命令：读取一条会话的运行态证据。
///
/// `running` 只认带租约且未过期的在跑轮次；读不到时返回错误，界面据此降级，
/// 不拿本地推算顶替。
#[tauri::command(async)]
pub fn conversation_session_live(session_id: String) -> Result<SessionLive, String> {
    session_live::read_session_live(&session_id)
}

/// Tauri IPC 命令：用户手动停止一条车道上的后台作业。
///
/// 归属与终态原因都由服务端裁决；回执里的 `stopped` 是真实结论，
/// 失败时返回的是服务端给出的拒绝原因（例如归属不符）。
#[tauri::command(async)]
pub fn conversation_stop_job(session_id: String, job_id: String) -> Result<JobStopReceipt, String> {
    job_stop::stop_job(&session_id, &job_id)
}

/// Tauri IPC 命令：读取一条车道上仍在运行的后台作业名册。
///
/// `settled` 是界面点名要收口事实的那些作业：名册给出「还持有谁」，收口事实给出
/// 「那一条跑了多久、怎么收口的」，两者同源同一次往返。参数可省：不带点名时
/// 只回名册，旧调用方与调试桥因此不必跟着改。
#[tauri::command(async)]
pub fn conversation_session_jobs(
    session_id: String,
    settled: Option<Vec<String>>,
) -> Result<SessionJobsRoster, String> {
    session_jobs::session_jobs(&session_id, &settled.unwrap_or_default())
}

/// Tauri IPC 命令：读取一次后台作业的输出增量。
///
/// 参数是**字节游标**而不是行号：归档是持续追加的字节流，行在写入过程中还可能
/// 只写了一半，按字节续读既不丢内容也不会重复。
#[tauri::command(async)]
pub fn conversation_job_output(
    session_id: String,
    job_id: String,
    offset: u64,
) -> Result<JobOutputStream, String> {
    job_output::job_output(&session_id, &job_id, offset)
}

/// Tauri IPC 命令：读取一条车道下的子代理会话清单。
#[tauri::command(async)]
pub fn conversation_session_subagents(session_id: String) -> Result<SessionSubagents, String> {
    session_read::session_subagents(&session_id)
}

/// Tauri IPC 命令：向正在运行的子代理投递一条用户消息。
///
/// 消息只到达目标子代理，主代理上下文不受影响；可投递性由服务端裁决，
/// 拒绝时返回的是服务端给出的原因（例如「该子代理当前不在运行」）。
#[tauri::command(async)]
pub fn conversation_send_subagent_message(
    session_id: String,
    agent_id: String,
    content: String,
) -> Result<SubagentMessageReceipt, String> {
    subagent_message::send_subagent_message(&session_id, &agent_id, &content)
}

/// Tauri IPC 命令：登记当前会话，供系统通知使用。
///
/// 通知标题要带会话名，点击后要能回到这条会话；两者都需要一个「用户此刻在哪个
/// 会话里」的事实，而这份事实只有界面知道（会话切换发生在界面里）。车道的展示名
/// 也一并交上来：Rust 侧不重新派生标题，避免同一件事出现两种口径。
#[tauri::command(async)]
pub fn conversation_set_active_session(session_id: String, title: String) -> Result<(), String> {
    crate::session_context::set_active_session(&session_id, &title);
    /*
     * 续聊指针随切换一起前移：`--continue` 落在「人最后在看的那条会话」上。
     * 走后台线程，切换本身不等这一次网络往返；写失败只记录，不阻塞界面，
     * 因为指针是锦上添花，不是打开会话的前置条件。
     */
    let pointer_session = session_id.trim().to_owned();
    if !pointer_session.is_empty() {
        std::thread::spawn(move || {
            if let Err(error) = crate::session_menu::move_resume_pointer(&pointer_session) {
                eprintln!("WARNING: 续聊指针更新失败：{error}");
            }
        });
    }
    Ok(())
}

/// Tauri IPC 命令：订阅一条 Interaction 的事件流。
///
/// 订阅按目标在进程内唯一的订阅器上登记：车道流与子代理会话流可以并存，
/// 同一个目标重复订阅是幂等的。
#[tauri::command(async)]
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

/// Tauri IPC 命令：订阅一条会话的**轮次头**。
///
/// 只推「多了一轮」与「这一轮收口了」两类轻帧，不推正文。独立会话不在会话清单里，
/// 界面靠这一路订阅发现新轮，再回来读一次索引。
#[tauri::command(async)]
pub fn conversation_watch_turns(
    state: State<'_, AppState>,
    workspace_id: String,
    session_id: String,
) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("会话标识不能为空".to_string());
    }
    if workspace_id.trim().is_empty() {
        return Err("工作区标识不能为空".to_string());
    }
    state.conversation.watch(SubscribeTarget::SessionTurns {
        workspace_id,
        session_id,
    });
    Ok(())
}

/// Tauri IPC 命令：取消一条订阅。
///
/// 必须按**订阅标识**精确取消：每个订阅目标各持一条连接，按标识退订才不会误伤同视图
/// 的其他目标（子代理视图与车道视图正是这种并存关系）。
#[tauri::command(async)]
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
