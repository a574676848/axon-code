//! TUI `/sessions` 选中会话后的历史读取。
//!
//! 会话目录来自运行态会话表，历史也必须来自同一份数据：这里读取该会话的 durable
//! wire 事件，按轮次投影成与实时回合完全相同的渲染行，再交给与冷启动同一条
//! `restore_historical_turn` 路径渲染。这样切过去的会话看到的就是它当初的样子，
//! 不存在「目录里有、历史读不到」的第二套事实源。

use std::path::Path;

use axon_tui_runtime::{AgentTuiRunLine, AgentTuiRunOutput, AgentTuiRunSessionInfo};

use super::run_output_wire_sequence::wire_sequence_lines;

/// 读取指定会话的历史回放载荷；会话没有可回放的轮次时返回 None。
pub(super) fn load_session_history(
    config_path: Option<&Path>,
    session_id: &str,
) -> Option<AgentTuiRunOutput> {
    let turns =
        crate::run_commands::daemon_history_loader::load_session_turns(config_path, session_id)
            .ok()?;
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Session(AgentTuiRunSessionInfo::new(
        session_id.to_owned(),
        None,
    )));
    for turn in turns {
        for line in wire_sequence_lines(&turn.wire_events, &[]) {
            output.push(line);
        }
    }
    (output.lines().len() > 1).then_some(output)
}
