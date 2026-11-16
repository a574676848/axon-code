//! Thread 尾部轮次视图：只读最近若干字节，回答「最新 N 轮」这一类问题。
//!
//! 每轮收口都要问一次「本轮轮序是多少」「最近几轮的显式信号」「上一轮的反思提示」，
//! 这些答案全在 thread 尾部。整条读取会让每轮代价随历史线性增长，因此这里统一走
//! 有字节预算的尾部读，并在尾部不足以给出可信答案时回退整条读。

use std::path::Path;

use axon_session::{JsonlThreadEventStore, ThreadEvent, ThreadEventRecord};

use super::run_session_store::resolved_axon_root;
use super::run_thread_store::ThreadTurnMetadata;
use super::run_thread_store::turn_metadata;

/// 尾部读取的默认预算：覆盖最近数轮的完整 item 与轮次边界。
const TAIL_BUDGET_BYTES: u64 = 16 * 1024 * 1024;

/// 尾部投影出的一轮事实。
#[derive(Debug, Clone)]
pub(super) struct TailTurn {
    pub(super) turn_id: String,
    pub(super) sequence: u64,
    pub(super) status: axon_session::TurnStatus,
    pub(super) started_at_ms: u64,
    pub(super) completed_at_ms: Option<u64>,
    pub(super) metadata: Option<ThreadTurnMetadata>,
    /// 本轮已落盘的历史 item（增量落盘时用来算 delta）。
    pub(super) history_items: Vec<axon_session::ThreadHistoryItem>,
    /// 本轮是否在尾部里从 `TurnStarted` 起完整可见。
    pub(super) complete: bool,
}

/// 一次尾部视图。
#[derive(Debug, Clone, Default)]
pub(super) struct ThreadTailView {
    pub(super) turns: Vec<TailTurn>,
    /// 尾部是否覆盖整条 thread。
    pub(super) complete: bool,
}

impl ThreadTailView {
    /// 尾部起点是轮次边界（或整条已读完）时，其中每一轮的计数与状态才可信。
    pub(super) fn reliable(&self) -> bool {
        self.complete
            || self
                .turns
                .first()
                .is_some_and(|turn| turn.complete && turn.sequence == 0)
    }

    pub(super) fn last_turn_with_usage(&self) -> Option<&TailTurn> {
        self.turns.iter().rev().find(|turn| {
            turn.metadata
                .as_ref()
                .is_some_and(|meta| meta.usage.is_some())
        })
    }
}

/// 读取尾部轮次视图；thread 不存在时返回空视图。
pub(super) fn tail_view(
    config_path: &Path,
    session_id: &str,
) -> Result<Option<ThreadTailView>, String> {
    let root = resolved_axon_root(config_path)?;
    let store = JsonlThreadEventStore;
    let tail = store
        .read_thread_tail(&root, session_id, TAIL_BUDGET_BYTES)
        .map_err(|error| format!("读取 thread 尾部失败：{error}"))?;
    if tail.records.is_empty() {
        return Ok(None);
    }
    Ok(Some(project_tail(&tail.records, tail.complete)))
}

fn project_tail(records: &[ThreadEventRecord], complete: bool) -> ThreadTailView {
    let mut turns = Vec::<TailTurn>::new();
    for record in records {
        match &record.event {
            ThreadEvent::TurnStarted { turn } => {
                turns.retain(|existing| existing.turn_id != turn.turn_id);
                turns.push(TailTurn {
                    turn_id: turn.turn_id.clone(),
                    sequence: turn.sequence,
                    status: turn.status,
                    started_at_ms: turn.started_at_ms,
                    completed_at_ms: turn.completed_at_ms,
                    metadata: turn_metadata(&turn.token_usage_json),
                    history_items: turn.history_items.clone(),
                    complete: true,
                });
            }
            ThreadEvent::TurnCompleted { turn } => {
                if let Some(existing) = turns.iter_mut().find(|item| item.turn_id == turn.turn_id) {
                    existing.status = turn.status;
                    existing.completed_at_ms = turn.completed_at_ms;
                    existing.metadata = turn_metadata(&turn.token_usage_json);
                    existing.history_items = turn.history_items.clone();
                } else {
                    turns.push(TailTurn {
                        turn_id: turn.turn_id.clone(),
                        sequence: turn.sequence,
                        status: turn.status,
                        started_at_ms: turn.started_at_ms,
                        completed_at_ms: turn.completed_at_ms,
                        metadata: turn_metadata(&turn.token_usage_json),
                        history_items: turn.history_items.clone(),
                        // 只有终态、没有起跑事件的轮次在尾部里是完整的：终态自带全量历史。
                        complete: true,
                    });
                }
            }
            ThreadEvent::TurnInterrupted { turn_id, .. } => {
                if let Some(existing) = turns.iter_mut().find(|item| item.turn_id == *turn_id) {
                    existing.status = axon_session::TurnStatus::Interrupted;
                }
            }
            ThreadEvent::TurnFailed { turn_id, .. } => {
                if let Some(existing) = turns.iter_mut().find(|item| item.turn_id == *turn_id) {
                    existing.status = axon_session::TurnStatus::Failed;
                }
            }
            ThreadEvent::ThreadCheckpoint { detail } => {
                // checkpoint 是整条 thread 的权威快照：尾部一旦包含它，轮次就以它为准。
                turns = detail
                    .turns
                    .iter()
                    .map(|turn| TailTurn {
                        turn_id: turn.turn_id.clone(),
                        sequence: turn.sequence,
                        status: turn.status,
                        started_at_ms: turn.started_at_ms,
                        completed_at_ms: turn.completed_at_ms,
                        metadata: turn_metadata(&turn.token_usage_json),
                        history_items: turn.history_items.clone(),
                        complete: true,
                    })
                    .collect();
            }
            ThreadEvent::TurnItemsAppended { turn_id, items } => {
                if let Some(existing) = turns.iter_mut().find(|item| item.turn_id == *turn_id) {
                    for item in items {
                        match existing
                            .history_items
                            .iter_mut()
                            .find(|current| current.identity() == item.identity())
                        {
                            Some(current) => *current = item.clone(),
                            None => existing.history_items.push(item.clone()),
                        }
                    }
                }
            }
            _ => {}
        }
    }
    ThreadTailView { turns, complete }
}

/// 轮序：turn_id 由 `{session}-turn-{assistant_part_id}` 组成，按后缀点查。
pub(super) fn turn_sequence_at_tail(
    config_path: &Path,
    session_id: &str,
    assistant_part_id: &str,
) -> Result<Option<u64>, String> {
    let Some(view) = tail_view(config_path, session_id)? else {
        return Ok(None);
    };
    let suffix = format!("-turn-{assistant_part_id}");
    if let Some(turn) = view
        .turns
        .iter()
        .find(|turn| turn.turn_id.ends_with(&suffix))
    {
        return Ok(Some(turn.sequence));
    }
    if view.complete {
        return Ok(None);
    }
    super::run_thread_store::turn_sequence(config_path, session_id, assistant_part_id)
}

/// 最近一轮的 token 用量合计。
pub(super) fn latest_turn_usage_tokens_at_tail(
    config_path: &Path,
    session_id: &str,
) -> Result<Option<u64>, String> {
    let Some(view) = tail_view(config_path, session_id)? else {
        return Ok(None);
    };
    if let Some(turn) = view.last_turn_with_usage() {
        return Ok(turn
            .metadata
            .as_ref()
            .and_then(|meta| meta.usage)
            .map(|usage| usage.total_tokens()));
    }
    if view.complete || view.reliable() {
        return Ok(None);
    }
    super::run_thread_store::latest_turn_usage_tokens_at_root(config_path, session_id)
}

/// 上一轮的记忆反思提示：取当前轮之前最近一条带提示的轮次。
pub(super) fn previous_reflection_notice_at_tail(
    config_path: &Path,
    session_id: &str,
    current_part_id: &str,
) -> Option<String> {
    let view = tail_view(config_path, session_id).ok()??;
    let current_suffix = format!("-turn-{current_part_id}");
    let position = view
        .turns
        .iter()
        .position(|turn| turn.turn_id.ends_with(&current_suffix));
    let candidates = match position {
        Some(position) => &view.turns[..position],
        None => view.turns.as_slice(),
    };
    if let Some(notice) = candidates
        .iter()
        .rev()
        .filter_map(|turn| {
            turn.metadata
                .as_ref()
                .and_then(|meta| meta.reflection_notice.clone())
        })
        .next()
    {
        return Some(notice);
    }
    // 当前轮已在尾部：它之前的轮次也都在尾部，答案就是「没有提示」，不必回退。
    if position.is_some() || view.complete {
        return None;
    }
    super::run_thread_store::previous_reflection_notice(config_path, session_id, current_part_id)
}

/// 最近若干轮的事实摘要；尾部不够覆盖时回退整条读。
pub(super) fn recent_turn_facts_at_tail(
    config_path: &Path,
    session_id: &str,
    limit: u32,
) -> Result<Vec<super::run_thread_store::RecentTurnFact>, String> {
    let Some(view) = tail_view(config_path, session_id)? else {
        return Ok(Vec::new());
    };
    let want = usize::try_from(limit).unwrap_or(usize::MAX);
    let enough = view.turns.len() >= want;
    if !enough && !view.complete {
        return super::run_thread_store::recent_turn_facts(config_path, session_id, limit);
    }
    let start = view.turns.len().saturating_sub(want);
    Ok(view.turns[start..]
        .iter()
        .map(|turn| super::run_thread_store::RecentTurnFact {
            sequence: turn.sequence,
            recorded_at_ms: turn.started_at_ms,
            prompt_chars: turn
                .metadata
                .as_ref()
                .map_or(0, |metadata| metadata.prompt_chars),
            failed: turn.status == axon_session::TurnStatus::Failed
                || turn
                    .metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.failed),
            affect_sequence: turn
                .metadata
                .as_ref()
                .map(|metadata| metadata.affect_sequence.clone())
                .unwrap_or_default(),
        })
        .collect())
}

/// 当前轮已落盘的历史 item；尾部没覆盖到本轮的起跑事件时返回 `None`。
pub(super) fn active_turn_items(
    config_path: &Path,
    session_id: &str,
    turn_id: &str,
) -> Result<Option<Vec<axon_session::ThreadHistoryItem>>, String> {
    let root = resolved_axon_root(config_path)?;
    let store = JsonlThreadEventStore;
    let tail = store
        .read_thread_tail(&root, session_id, TAIL_BUDGET_BYTES)
        .map_err(|error| format!("读取 thread 尾部失败：{error}"))?;
    if tail.records.is_empty() {
        return Ok(None);
    }
    let view = project_tail(&tail.records, tail.complete);
    let Some(turn) = view.turns.iter().find(|turn| turn.turn_id == turn_id) else {
        return Ok(None);
    };
    if !turn.complete && !view.complete {
        return Ok(None);
    }
    Ok(Some(turn.history_items.clone()))
}
