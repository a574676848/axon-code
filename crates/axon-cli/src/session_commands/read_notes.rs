//! 追读口径：`uncertain` 轮次与无归属子代理帧必须给出可读结论。
//!
//! 两种现场的共同点是「正文看起来是空的」，但空正文的原因完全不同：
//!
//! - **结论未知**：这一轮的执行被中断，外部结果没有确认。落库状态由执行账本的
//!   `ExecutionStatus::Uncertain` 表达（存储层统一定义落库取值），这里只读它，不另造判定。
//! - **没有归属**：子代理会话的帧没有写归属列。归属能不能回填复用存量修复的同一套判定
//!   （子代理会话 + 事件日志 `interaction_id` 为空 + 派发 join），能唯一确定就指向
//!   `axon debug session repair --attribute`，否则如实给出「无法确定」的原因。
//!
//! 两条结论都只用一句话讲清「这不是没有内容」，不把空正文当成正常终态。

use axon_domain::{DaemonSessionId, ExecutionStatus};
use axon_session::{ExecutionStore as _, SqliteRuntimeStore};

use super::session_repair::{
    DispatchResolution, is_subagent_session, read_log_records, resolve_dispatch,
};

/// 追读备注的前缀：文本输出逐行打印。
pub(super) const READ_NOTE_PREFIX: &str = "- 备注: ";
/// 无归属帧的统一收尾：这一句必须出现，否则空正文会被读成「本来就没有过程」。
const UNATTRIBUTED_FOOTER: &str = "该轮次的过程仍在事件库里，无法归位不等于这一轮没有过程";
/// 诊断摘要里最多点名多少条会话；超出只报总数。
const NOTE_SAMPLE_LIMIT: usize = 3;

/// 一条会话的追读备注。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReadNotes {
    /// 可读结论，按「结论未知」在前、「没有归属」在后。
    pub(super) lines: Vec<String>,
    /// 结论未知的轮次数。
    pub(super) uncertain_turns: usize,
    /// 没有归属的帧数。
    pub(super) unattributed_frames: usize,
}

impl ReadNotes {
    /// 文本输出的备注行。
    #[cfg(test)]
    pub(super) fn text_lines(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|line| format!("{READ_NOTE_PREFIX}{line}"))
            .collect()
    }
}

/// 结论未知的轮次说明；状态不是 `uncertain` 时返回 `None`。
///
/// 只读执行账本的状态：`uncertain` 是「外部结果未知」，与「这一轮没有内容」是两种事实，
/// 因此文案必须点明「不是没有内容」。
pub(super) fn uncertain_turn_note(execution_id: &str, status: ExecutionStatus) -> Option<String> {
    (status == ExecutionStatus::Uncertain).then(|| {
        format!(
            "本轮（execution {execution_id}）结论未知（uncertain）：执行被中断且外部结果未确认，不是「没有内容」"
        )
    })
}

/// 无归属子代理帧的说明；没有帧时返回 `None`。
pub(super) fn unattributed_frames_note(
    unattributed_frames: usize,
    resolution: &DispatchResolution,
) -> Option<String> {
    if unattributed_frames == 0 {
        return None;
    }
    let reason = match resolution {
        DispatchResolution::Unique(interaction_id) => format!(
            "派发轮次可唯一确定为 {}，但这些帧没有写归属列，可用 `axon debug session repair --attribute` 回填",
            interaction_id.as_str()
        ),
        DispatchResolution::Undetermined(reason) => format!("无法唯一确定派发轮次：{reason}"),
    };
    Some(format!(
        "该子代理会话有 {unattributed_frames} 帧没有归属（{reason}）。{UNATTRIBUTED_FOOTER}"
    ))
}

/// 读一条会话的追读备注。
///
/// 会话标识缺失时不留白：Thread 未登记会话本身就是「无法判定」的原因，必须写出来。
pub(super) fn read_notes(store: &SqliteRuntimeStore, session_id: Option<&str>) -> ReadNotes {
    let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) else {
        return ReadNotes {
            lines: vec![
                "该 Thread 未登记会话：无法判定轮次结论与帧归属，空正文不代表这一轮没有内容"
                    .to_owned(),
            ],
            ..ReadNotes::default()
        };
    };
    let Ok(session) = DaemonSessionId::new(session_id.to_owned()) else {
        return ReadNotes {
            lines: vec![format!("会话标识非法，无法判定轮次结论：{session_id}")],
            ..ReadNotes::default()
        };
    };
    let mut notes = ReadNotes::default();
    match store.execution_for_session(&session) {
        Ok(Some(execution)) => {
            if let Some(note) =
                uncertain_turn_note(execution.execution_id.as_str(), execution.status)
            {
                notes.uncertain_turns = notes.uncertain_turns.saturating_add(1);
                notes.lines.push(note);
            }
        }
        Ok(None) => {}
        Err(error) => notes
            .lines
            .push(format!("读取执行账本失败，无法判定轮次结论：{error}")),
    }
    if !is_subagent_session(store, session_id) {
        return notes;
    }
    let log_path = store.event_log(&session).path();
    // 日志文件还没落盘说明这条子会话一帧都还没写过：没有帧就没有归属问题，不加备注。
    if !log_path.is_file() {
        return notes;
    }
    match read_log_records(&log_path) {
        Ok(records) => {
            let unattributed = records
                .iter()
                .filter(|record| record.interaction_id.is_none())
                .count();
            let resolution = match resolve_dispatch(store, session_id) {
                Ok(resolution) => resolution,
                Err(message) => DispatchResolution::Undetermined(message),
            };
            if let Some(note) = unattributed_frames_note(unattributed, &resolution) {
                notes.unattributed_frames = unattributed;
                notes.lines.push(note);
            }
        }
        Err(message) => notes.lines.push(format!(
            "事件日志读不出来，无法判定帧归属：{message}。空正文不代表这一轮没有内容"
        )),
    }
    notes
}

/// doctor 的诊断结果：两类需要说明的现场。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReadNoteScan {
    /// 最近一轮结论未知的会话。
    pub(super) uncertain_sessions: Vec<String>,
    /// 有帧没有归属的子代理会话及其帧数。
    pub(super) unattributed_sessions: Vec<(String, usize)>,
}

impl ReadNoteScan {
    /// 两类现场都不存在。
    pub(super) fn is_clean(&self) -> bool {
        self.uncertain_sessions.is_empty() && self.unattributed_sessions.is_empty()
    }

    /// 一行可读结论：清干净时说明检查了什么，有问题时点名并写清原因。
    pub(super) fn summary(&self) -> String {
        if self.is_clean() {
            return "所有会话都有可判定的轮次结论与帧归属".to_owned();
        }
        let mut parts = Vec::new();
        if !self.uncertain_sessions.is_empty() {
            parts.push(format!(
                "结论未知（uncertain，外部结果未确认，不是没有内容）的会话 {} 条：{}",
                self.uncertain_sessions.len(),
                self.uncertain_sessions
                    .iter()
                    .take(NOTE_SAMPLE_LIMIT)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if !self.unattributed_sessions.is_empty() {
            parts.push(format!(
                "有帧没有归属（过程在库里但无法归位）的子代理会话 {} 条：{}",
                self.unattributed_sessions.len(),
                self.unattributed_sessions
                    .iter()
                    .take(NOTE_SAMPLE_LIMIT)
                    .map(|(session, frames)| format!("{session}（{frames} 帧）"))
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        parts.join("；")
    }
}

/// 扫一遍运行态会话，收集两类追读现场。
///
/// 只做只读查询：结论未知读执行账本，帧归属读子代理会话的事件日志，与
/// `axon debug session repair` 的判定同源。
pub(super) fn scan_read_notes(store: &SqliteRuntimeStore) -> Result<ReadNoteScan, String> {
    let rows = crate::sessions_http_adapter::read_session_rows(store)
        .map_err(|error| format!("读取会话清单失败：{error}"))?;
    let mut scan = ReadNoteScan::default();
    for row in rows {
        let notes = read_notes(store, Some(row.session_id.as_str()));
        if notes.uncertain_turns > 0 {
            scan.uncertain_sessions.push(row.session_id.clone());
        }
        if notes.unattributed_frames > 0 {
            scan.unattributed_sessions
                .push((row.session_id.clone(), notes.unattributed_frames));
        }
    }
    Ok(scan)
}

#[cfg(test)]
#[path = "read_notes_tests.rs"]
mod tests;
