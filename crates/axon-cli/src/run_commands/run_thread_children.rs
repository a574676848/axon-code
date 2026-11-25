//! 子代理来源标注与内务判定。
//!
//! 名册本身已改由会话索引回答（见 `workbench_http_subagents`）：按 `(kind, root_session_id)`
//! 直接取本车道的子会话，不再把整份 thread 索引读出来过滤。这里只保留两件事：判定一条
//! agent 路径是否属于运行侧内务，以及在子代理起跑时把来源写进 thread 事件面。

use std::path::Path;

use axon_session::{AnnotateChildOriginCommand, JsonlThreadEventStore, ThreadService};

use super::run_session_store::resolved_axon_root;

/// 运行侧内务子代理的 agent 路径段前缀。
///
/// 自迭代、反思与语义显著度都按这个前缀派生子任务；它们不是用户要的子代理，
/// 名册与子代理视图都不得把它们当成可进入的委派。
pub(crate) const SELF_ITERATION_AGENT_PREFIX: &str = "self_iteration";

/// 该 agent 路径是否属于运行侧内务子代理。
pub(crate) fn is_internal_agent_path(agent_path: &str) -> bool {
    agent_path
        .split('/')
        .any(|segment| segment.starts_with(SELF_ITERATION_AGENT_PREFIX))
}

/// 子代理会话来源：用户可见的委派。
pub(crate) const CHILD_SESSION_ORIGIN_DELEGATED: &str = "delegated";
/// 子代理会话来源：运行侧内务（自迭代、反思、语义显著度）。
///
/// 这类会话由轮后治理自己派出去，不是用户要的子代理：名册不把它们列成会话的
/// 运行时动作，否则每个跑过一轮的会话都会长出一条并不存在的「子代理」。
pub(crate) const CHILD_SESSION_ORIGIN_INTERNAL: &str = "internal";

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

#[cfg(test)]
#[path = "run_thread_children_tests.rs"]
mod tests;
