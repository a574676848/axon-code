//! 子代理名册与来源标注：父会话关系、状态与标题全部取自 thread 事件面。
//!
//! 名册不再有第二份投影库：`ThreadSummary` 直接给出父会话、标题、模型、状态与末轮
//! 时间，一次投影回答名册的全部问题。

use std::path::Path;

use axon_session::{
    AnnotateChildOriginCommand, JsonlThreadEventStore, ThreadListQuery, ThreadService, TurnStatus,
};

use super::run_session_store::resolved_axon_root;

/// 子代理会话来源：用户可见的委派。
pub(crate) const CHILD_SESSION_ORIGIN_DELEGATED: &str = "delegated";
/// 子代理会话来源：运行侧内务（自迭代、反思、语义显著度）。
///
/// 这类会话由轮后治理自己派出去，不是用户要的子代理：名册不把它们列成会话的
/// 运行时动作，否则每个跑过一轮的会话都会长出一条并不存在的「子代理」。
pub(crate) const CHILD_SESSION_ORIGIN_INTERNAL: &str = "internal";

/// 名册一次投影的 thread 上限：名册只服务单条车道的子会话。
const CHILD_SESSION_SCAN_LIMIT: usize = 4096;

/// 子代理会话在名册里的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildSessionRow {
    pub(crate) session_id: String,
    /// 线程标题；尚未命名的会话为空。
    pub(crate) title: Option<String>,
    pub(crate) provider: String,
    pub(crate) model: String,
    /// 末轮状态：active / completed / interrupted / failed；没有轮次时为空。
    pub(crate) status: Option<String>,
    pub(crate) updated_at_ms: u64,
    /// 末轮记录时间；没有轮次时为空。
    pub(crate) last_turn_at_ms: Option<u64>,
}

/// 记录子会话来源：子代理起跑时写一次，重复标注以最后一次为准。
pub(super) fn record_child_origin(
    config_path: &Path,
    session_id: &str,
    origin: &str,
) -> Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_owned());
    }
    let root = resolved_axon_root(config_path)?;
    ThreadService::new(JsonlThreadEventStore).annotate_child_origin(
        &root,
        AnnotateChildOriginCommand {
            thread_id: session_id.to_owned(),
            recorded_at_ms: super::run_execution::now_ms(),
            origin: origin.to_owned(),
        },
    )
}

/// 某条会话直接派生的子代理名册；按最近活动倒序。
///
/// 只投影 thread 摘要，不逐条读会话正文：代价与子会话历史体量无关。
pub(crate) fn child_session_rows(
    root: &Path,
    parent_session_id: &str,
    limit: usize,
) -> Result<Vec<ChildSessionRow>, String> {
    let parent_session_id = parent_session_id.trim();
    if parent_session_id.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let page = ThreadService::new(JsonlThreadEventStore).list_thread_page(
        root,
        &ThreadListQuery {
            limit: CHILD_SESSION_SCAN_LIMIT,
            ..ThreadListQuery::default()
        },
    )?;
    let mut rows = page
        .threads
        .into_iter()
        .filter(|summary| summary.parent_thread_id.as_deref() == Some(parent_session_id))
        .filter(|summary| summary.child_origin.as_deref() != Some(CHILD_SESSION_ORIGIN_INTERNAL))
        .map(|summary| ChildSessionRow {
            session_id: summary.thread_id,
            title: summary.name,
            provider: summary.provider,
            model: summary.model,
            status: summary.last_turn_status.map(turn_status_text),
            updated_at_ms: summary.updated_at_ms,
            last_turn_at_ms: summary.last_turn_at_ms,
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at_ms));
    rows.truncate(limit);
    Ok(rows)
}

/// 末轮状态回到运行侧口径的字符串。
fn turn_status_text(status: TurnStatus) -> String {
    match status {
        TurnStatus::Active => "active",
        TurnStatus::Completed => "completed",
        TurnStatus::Interrupted => "interrupted",
        TurnStatus::Failed => "failed",
    }
    .to_owned()
}

#[cfg(test)]
#[path = "run_thread_children_tests.rs"]
mod tests;
