//! TUI `/sessions` 选中会话后的历史读取。
//!
//! 会话目录来自 thread 存储，历史也必须来自同一份数据：这里读取目标 thread 的
//! turns，把用户轮与助手轮投影成 TUI 历史回放载荷，交给与冷启动同一条
//! `restore_historical_turn` 路径渲染。工具回执与 reasoning 不属于可见转录，
//! 因此不进入投影。

use std::path::Path;

use axon_session::{JsonlThreadEventStore, ThreadDetail, ThreadHistoryItem, ThreadService};
use axon_tui_runtime::{
    AgentTuiMessagePart, AgentTuiRunLine, AgentTuiRunOutput, AgentTuiRunSessionInfo,
};

/// 读取指定会话的历史回放载荷；会话不存在或没有可见对话时返回 None。
pub(super) fn load_session_history(
    config_path: Option<&Path>,
    session_id: &str,
) -> Option<AgentTuiRunOutput> {
    let root = crate::run_commands::run_session_store::resolved_axon_root(config_path?).ok()?;
    let detail = ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, session_id)
        .ok()?;
    let output = session_history_output(&detail);
    (!output.is_empty()).then_some(output)
}

/// 把 thread 历史投影成 TUI 历史回放载荷。
fn session_history_output(detail: &ThreadDetail) -> AgentTuiRunOutput {
    let messages = detail
        .turns
        .iter()
        .flat_map(|turn| turn.history_items.iter())
        .filter_map(history_message)
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return AgentTuiRunOutput::empty();
    }
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Session(AgentTuiRunSessionInfo::new(
        detail.thread.thread_id.clone(),
        None,
    )));
    for message in messages {
        output.push(AgentTuiRunLine::Message(message));
    }
    output
}

fn history_message(item: &ThreadHistoryItem) -> Option<AgentTuiMessagePart> {
    match item {
        ThreadHistoryItem::UserMessage {
            part_id, content, ..
        } => Some(AgentTuiMessagePart::user(part_id.clone(), content.clone())),
        ThreadHistoryItem::AssistantMessage {
            part_id, content, ..
        } => Some(AgentTuiMessagePart::assistant(
            part_id.clone(),
            content.clone(),
        )),
        ThreadHistoryItem::Reasoning { .. }
        | ThreadHistoryItem::ToolCall { .. }
        | ThreadHistoryItem::ToolResult { .. }
        | ThreadHistoryItem::InjectedContext { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use axon_session::{
        Thread, ThreadDetail, ThreadHistoryItem, ThreadSettings, ThreadSource, ThreadStatus, Turn,
        TurnStatus,
    };
    use axon_tui_runtime::{AgentTuiMessagePartRole, AgentTuiRunLine};

    #[test]
    fn session_history_projects_only_visible_user_and_assistant_turns() {
        let detail = ThreadDetail {
            thread: thread_fixture(),
            turns: vec![Turn {
                turn_id: "turn-1".to_owned(),
                sequence: 1,
                status: TurnStatus::Completed,
                history_items: vec![
                    ThreadHistoryItem::UserMessage {
                        sequence: 1,
                        part_id: "part:user:1".to_owned(),
                        content: "上一轮提问".to_owned(),
                        local_images: Vec::new(),
                        local_files: Vec::new(),
                        provider_metadata: None,
                    },
                    ThreadHistoryItem::Reasoning {
                        sequence: 2,
                        provider_turn_id: 1,
                        part_id: "part:reasoning:1".to_owned(),
                        reasoning_id: "reasoning-1".to_owned(),
                        content: "内部推理".to_owned(),
                        provider_metadata: None,
                    },
                    ThreadHistoryItem::AssistantMessage {
                        sequence: 3,
                        part_id: "part:assistant:1".to_owned(),
                        content: "上一轮回答".to_owned(),
                        provider_metadata: None,
                    },
                ],
                token_usage_json: None,
                started_at_ms: 1,
                completed_at_ms: Some(2),
                interrupted_at_ms: None,
                failure: None,
            }],
            compacted_history: None,
        };

        let output = super::session_history_output(&detail);
        let messages = output
            .lines()
            .iter()
            .filter_map(|line| match line {
                AgentTuiRunLine::Message(message) => Some((message.role, message.text.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            messages,
            vec![
                (AgentTuiMessagePartRole::User, "上一轮提问".to_owned()),
                (AgentTuiMessagePartRole::Assistant, "上一轮回答".to_owned()),
            ]
        );
        assert_eq!(
            output
                .lines()
                .first()
                .map(|line| matches!(line, AgentTuiRunLine::Session(_))),
            Some(true),
            "历史回放必须先带会话身份行"
        );
    }

    #[test]
    fn session_history_without_visible_messages_is_empty() {
        let detail = ThreadDetail {
            thread: thread_fixture(),
            turns: Vec::new(),
            compacted_history: None,
        };

        assert!(super::session_history_output(&detail).is_empty());
    }

    fn thread_fixture() -> Thread {
        Thread {
            thread_id: "session-1".to_owned(),
            session_root_id: "session-1".to_owned(),
            parent_thread_id: None,
            forked_from_thread_id: None,
            name: Some("历史会话".to_owned()),
            status: ThreadStatus::Idle,
            source: ThreadSource::Tui,
            ephemeral: false,
            settings: ThreadSettings {
                cwd: ".".to_owned(),
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                selected_skill_name: None,
            },
            created_at_ms: 1,
            updated_at_ms: 2,
            archived_at_ms: None,
        }
    }
}
