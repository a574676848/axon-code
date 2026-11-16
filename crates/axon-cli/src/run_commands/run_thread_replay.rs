//! 会话回放读模型：resume 链、轮次摘要与工具归档输出，全部取自 thread 事件面。
//!
//! 回放不再有第二份事件库：thread 轮次就是一轮的终态事实，工具归档仍按内容寻址
//! 存放在运行态 `data` 根下，轮次里的回执自带归档引用。

use std::collections::HashSet;
use std::path::Path;

use axon_session::{JsonlThreadEventStore, ThreadService};

use super::run_session_store::resolved_axon_root;
use super::run_thread_history::turn_item_projection;

/// resume 链回放视图（可序列化）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplayChainView {
    /// 链中位置（0 为根）。
    pub(crate) depth: usize,
    /// 会话 ID。
    pub(crate) session_id: String,
    /// 父会话 ID。
    pub(crate) parent_session_id: Option<String>,
    /// 末轮创建时间，Unix 毫秒。
    pub(crate) created_at_ms: u64,
    /// provider 名称。
    pub(crate) provider: String,
    /// 模型 ID。
    pub(crate) model: String,
    /// 末轮 prompt 字符数。
    pub(crate) prompt_chars: usize,
    /// 末轮 response 字符数。
    pub(crate) response_chars: usize,
}

/// 轮次级回放摘要：一轮的终态、时间与结构化计数。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionTurnReplay {
    /// 会话内轮序。
    pub(crate) sequence: u64,
    /// 轮次状态：active / completed / interrupted / failed。
    pub(crate) status: String,
    /// 轮次记录时间，Unix 毫秒。
    pub(crate) recorded_at_ms: u64,
    /// 本轮 turn 事件条数。
    pub(crate) turn_event_count: usize,
    /// 本轮工具回执条数。
    pub(crate) tool_receipt_count: usize,
    /// 本轮的记忆反思提示；没有时为空。
    pub(crate) reflection_notice: Option<String>,
}

/// 回放整条 resume 链：从给定会话沿 parent 回溯到根，按时间正序返回。
pub(crate) fn replay_session_chain(
    config_path: &Path,
    session_id: &str,
) -> Result<Vec<ReplayChainView>, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("session id 不能为空".to_owned());
    }
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut chain = Vec::new();
    let mut current = Some(session_id.to_owned());
    let mut visited = HashSet::new();
    while let Some(thread_id) = current {
        if !visited.insert(thread_id.clone()) {
            return Err("thread resume 链存在环".to_owned());
        }
        let Ok(detail) = service.read_thread(&root, &thread_id) else {
            break;
        };
        chain.push(chain_view(&detail));
        current = detail.thread.parent_thread_id.clone();
    }
    if chain.is_empty() {
        return Err(format!("未找到会话：{session_id}"));
    }
    // 回溯得到的是「子→父」顺序，反转为按时间正序（根在前），深度从 0 开始。
    chain.reverse();
    Ok(chain
        .into_iter()
        .enumerate()
        .map(|(depth, mut view)| {
            view.depth = depth;
            view
        })
        .collect())
}

/// 单条会话的轮次摘要，按轮序升序。
pub(crate) fn replay_session_turns(
    config_path: &Path,
    session_id: &str,
) -> Result<Vec<SessionTurnReplay>, String> {
    let Some(detail) = read_detail(config_path, session_id) else {
        return Err(format!("未找到会话：{session_id}"));
    };
    Ok(detail
        .turns
        .iter()
        .map(|turn| {
            let metadata = turn_metadata(&turn.token_usage_json);
            SessionTurnReplay {
                sequence: turn.sequence,
                status: status_text(turn.status),
                recorded_at_ms: turn
                    .completed_at_ms
                    .or(turn.interrupted_at_ms)
                    .unwrap_or(turn.started_at_ms),
                turn_event_count: metadata.as_ref().map_or(0, |value| value.turn_event_count),
                tool_receipt_count: metadata
                    .as_ref()
                    .map_or(0, |value| value.tool_receipts.len()),
                reflection_notice: metadata.and_then(|value| value.reflection_notice),
            }
        })
        .collect())
}

/// 读取指定会话中某条 provider 回执关联的完整工具输出。
///
/// 回执必须唯一：同一会话出现多条同 provider 调用标识时拒绝读取，不猜是哪一条。
pub(crate) fn read_task_output(
    config_path: &Path,
    session_id: &str,
    provider_call_id: &str,
) -> Result<Vec<u8>, String> {
    let detail =
        read_detail(config_path, session_id).ok_or_else(|| format!("未找到会话：{session_id}"))?;
    let mut receipts = detail
        .turns
        .iter()
        .flat_map(|turn| turn.history_items.iter())
        .filter_map(|item| match item {
            axon_session::ThreadHistoryItem::ToolResult {
                provider_call_id: item_call_id,
                receipt,
                ..
            } => receipt
                .as_deref()
                .filter(|_| item_call_id == provider_call_id),
            _ => None,
        });
    let receipt = receipts
        .next()
        .ok_or_else(|| format!("session 中未找到 provider receipt：{provider_call_id}"))?;
    if receipts.next().is_some() {
        return Err(format!(
            "session 中 provider receipt 不唯一，拒绝读取：{provider_call_id}"
        ));
    }
    let persisted_ref = receipt
        .output_ref
        .as_ref()
        .ok_or_else(|| "provider receipt 未包含 OutputRef".to_owned())?;
    let archive_root = crate::runtime_paths::runtime_axon_data_path_static(config_path)?;
    axon_agent_runtime::ContentArchiveRuntime::default()
        .read_output(&archive_root, persisted_ref)
        .map_err(|error| format!("工具输出归档读取失败：{error}"))
}

fn read_detail(config_path: &Path, session_id: &str) -> Option<axon_session::ThreadDetail> {
    let root = resolved_axon_root(config_path).ok()?;
    ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, session_id.trim())
        .ok()
}

fn chain_view(detail: &axon_session::ThreadDetail) -> ReplayChainView {
    let latest = detail.turns.last();
    let (prompt_chars, response_chars) = latest.map_or((0, 0), |turn| {
        let projection = turn_item_projection(&turn.history_items);
        (
            projection.prompt.chars().count(),
            projection.response.chars().count(),
        )
    });
    ReplayChainView {
        depth: 0,
        session_id: detail.thread.thread_id.clone(),
        parent_session_id: detail.thread.parent_thread_id.clone(),
        created_at_ms: latest.map_or(detail.thread.created_at_ms, |turn| turn.started_at_ms),
        provider: detail.thread.settings.provider.clone(),
        model: detail.thread.settings.model.clone(),
        prompt_chars,
        response_chars,
    }
}

fn status_text(status: axon_session::TurnStatus) -> String {
    match status {
        axon_session::TurnStatus::Active => "active",
        axon_session::TurnStatus::Completed => "completed",
        axon_session::TurnStatus::Interrupted => "interrupted",
        axon_session::TurnStatus::Failed => "failed",
    }
    .to_owned()
}

/// 轮次 metadata 的只读解码。
fn turn_metadata(
    token_usage_json: &Option<String>,
) -> Option<super::run_thread_store::ThreadTurnMetadata> {
    serde_json::from_str(token_usage_json.as_deref()?).ok()
}
