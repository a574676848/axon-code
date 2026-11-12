//! 承诺检测与生命周期（M3-3）。
//!
//! 命中承诺句式 → 写入一条 `open` 承诺；同一线索已有 `open` 承诺时只更新原文（单例语义）。
//! 命中完成/取消句式 → 关闭该命名空间下的 `open` 承诺。
//! `enabled = false` 时不检测、不写入、不关闭，行为与基线一致。

use axon_domain::AxonResult;

use crate::MemorySqliteStore;
use crate::store_thread::COMMITMENT_STATUS_DONE;
use crate::thread::{ThreadScope, open_commitment};

/// 承诺句式标记词（中文）。
const COMMITMENT_OPEN_MARKERS_ZH: [&str; 8] = [
    "我明天提醒你",
    "提醒我",
    "回头给你",
    "我记下了",
    "我稍后",
    "明天提醒",
    "记得提醒",
    "我记一下",
];

/// 承诺句式标记词（英文）。
const COMMITMENT_OPEN_MARKERS_EN: [&str; 3] = ["remind me", "i will remind", "i'll remind"];

/// 完成/取消句式标记词。
const COMMITMENT_CLOSE_MARKERS: [&str; 11] = [
    "搞定",
    "好了",
    "不用了",
    "交了",
    "交完了",
    "完成了",
    "已经完成",
    "done",
    "finished",
    "never mind",
    "算了",
];

/// 关闭句式只在短句上生效，避免「好了我们开始下一件事」误关承诺。
const CLOSURE_MAX_CHARS: usize = 20;

/// 是否命中承诺句式。
#[must_use]
pub fn detects_commitment(text: &str) -> bool {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return false;
    }
    COMMITMENT_OPEN_MARKERS_ZH
        .iter()
        .chain(COMMITMENT_OPEN_MARKERS_EN.iter())
        .any(|marker| text.contains(marker))
}

/// 是否命中完成/取消句式。
#[must_use]
pub fn detects_closure(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > CLOSURE_MAX_CHARS {
        return false;
    }
    let lowered = trimmed.to_lowercase();
    COMMITMENT_CLOSE_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
}

/// 检测并记录承诺；返回写入的 `commitment_id`。
///
/// 未命中承诺句式或 `enabled = false` 时返回 `None` 且不产生任何写入。
pub fn record_commitment(
    store: &MemorySqliteStore,
    scope: &ThreadScope<'_>,
    thread_id: Option<&str>,
    text: &str,
    channel: &str,
    now_ms: i64,
    enabled: bool,
) -> AxonResult<Option<String>> {
    if !enabled || !detects_commitment(text) {
        return Ok(None);
    }
    let commitment = open_commitment(scope, thread_id, text, channel, now_ms);
    let commitment_id = commitment.commitment_id.clone();
    store.upsert_open_commitment(&commitment)?;
    Ok(Some(commitment_id))
}

/// 用户表示完成或取消时关闭该命名空间下的 `open` 承诺，返回关闭条数。
pub fn close_commitments(
    store: &MemorySqliteStore,
    scope: &ThreadScope<'_>,
    text: &str,
    now_ms: i64,
    enabled: bool,
) -> AxonResult<usize> {
    if !enabled || !detects_closure(text) {
        return Ok(0);
    }
    let open = store.open_commitments(scope.namespace_ref, scope.user_ref, usize::MAX)?;
    let mut closed = 0;
    for commitment in open {
        if store.close_commitment(&commitment.commitment_id, COMMITMENT_STATUS_DONE, now_ms)? {
            closed += 1;
        }
    }
    Ok(closed)
}

#[cfg(test)]
#[path = "commitment_tests.rs"]
mod tests;
