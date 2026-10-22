//! TUI 待决问答作答的 Daemon 发送路径（UQ-205）。
//!
//! 作答不是一轮新任务：它必须走统一答案入口 `DaemonCommand::AnswerQuestion`，
//! 由服务端校验、CAS 结算并投递，TUI 不自造 `[问答回执]` 正文。
//!
//! 发送复用 `run_commands::daemon_interaction_submission` 的同一条 IPC 通道：
//! 连接方式、往返超时与错误映射全端只有一处实现，作答不会被另起一套超时口径。

use std::path::Path;

use axon_protocol::DaemonCommand;
use axon_tui_runtime::AgentTuiQuestionAnswer;

use super::TUI_CHANNEL;
use crate::run_commands::daemon_interaction_submission::send_command;
use crate::runtime_paths::{DEFAULT_DAEMON_ENDPOINT, resolve_daemon_endpoint};

/// 把一条待决问答作答发往统一答案入口。
pub(super) fn submit_question_answer(
    answer: &AgentTuiQuestionAnswer,
    config_path: Option<&Path>,
) -> Result<serde_json::Value, String> {
    let command = question_answer_command(answer)?;
    let endpoint = resolve_daemon_endpoint(DEFAULT_DAEMON_ENDPOINT, config_path)?;
    send_command(&endpoint, command)
}

/// 构造作答命令。
///
/// 作答标识与条目在本地先校验：空标识或空条目送到服务端只会换回一条
/// `UNKNOWN_CALL` / `BAD_ANSWER`，不如当场给出可执行的说明。
fn question_answer_command(answer: &AgentTuiQuestionAnswer) -> Result<DaemonCommand, String> {
    if answer.call_id.trim().is_empty() {
        return Err("问答作答缺少问题调用标识".to_owned());
    }
    if answer.answers.is_empty() {
        return Err("问答作答条目不能为空".to_owned());
    }
    Ok(DaemonCommand::AnswerQuestion {
        lane_id: answer.lane_id.clone(),
        call_id: answer.call_id.clone(),
        answers: answer.answers.clone(),
        channel: TUI_CHANNEL.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use axon_domain::UserAnswerItem;

    use super::*;

    fn answer(selected: Vec<String>) -> AgentTuiQuestionAnswer {
        AgentTuiQuestionAnswer {
            lane_id: "lane-main".to_owned(),
            call_id: "call-question-1".to_owned(),
            answers: vec![UserAnswerItem {
                id: "q1".to_owned(),
                selected,
                custom: None,
            }],
        }
    }

    #[test]
    fn question_answer_uses_the_dedicated_daemon_command() {
        let command = question_answer_command(&answer(vec!["继续部署".to_owned()]))
            .expect("合法作答应构造出命令");

        let DaemonCommand::AnswerQuestion {
            lane_id,
            call_id,
            answers,
            channel,
        } = command
        else {
            panic!("作答必须走 DaemonCommand::AnswerQuestion，而不是普通消息提交");
        };
        assert_eq!(lane_id, "lane-main");
        assert_eq!(call_id, "call-question-1");
        assert_eq!(answers[0].selected, vec!["继续部署".to_owned()]);
        assert_eq!(channel, TUI_CHANNEL);
    }

    #[test]
    fn explicit_skip_is_encoded_as_empty_selection() {
        let command = question_answer_command(&answer(Vec::new())).expect("跳过应可提交");

        let DaemonCommand::AnswerQuestion { answers, .. } = command else {
            panic!("跳过同样走统一答案入口");
        };
        assert!(answers[0].selected.is_empty());
        assert_eq!(answers[0].custom, None);
    }

    #[test]
    fn empty_call_id_or_answers_are_rejected_before_any_ipc() {
        let mut missing_call = answer(vec!["继续部署".to_owned()]);
        missing_call.call_id = "  ".to_owned();
        assert!(question_answer_command(&missing_call).is_err());

        let mut empty_answers = answer(vec!["继续部署".to_owned()]);
        empty_answers.answers.clear();
        assert!(question_answer_command(&empty_answers).is_err());
    }
}
