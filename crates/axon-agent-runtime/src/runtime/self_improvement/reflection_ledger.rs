//! 反思链路的幂等台账。
//!
//! 两张小账本，都落在运行态目录下，都不是事实源：
//!
//! - `processed`：记「这一轮已经跑过反思」。治理队列对失败 payload 会重试，重试会让
//!   语义评估与反思子代理整条重跑；同一批候选被写两遍就是这里漏掉的。
//! - `inflight`：记「这一批已经出池、还没跑完」。进程在反思之前被杀时，下一次入池
//!   把它们放回队列，卡片不会静默丢。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ReflectionPoolEntry;

/// 已处理轮次的台账文件名。
pub const REFLECTION_PROCESSED_FILE: &str = "reflection_processed.jsonl";
/// 出池未完成的批次文件名。
pub const REFLECTION_INFLIGHT_FILE: &str = "reflection_pool_inflight.json";
/// 批次已写候选的台账文件名。按「批次键 + 候选指纹」去重，挡住半批失败后的重试重写。
pub const REFLECTION_WRITTEN_FILE: &str = "reflection_written.jsonl";

/// 台账文件位置与读写。
pub struct ReflectionLedgerRuntime;

impl ReflectionLedgerRuntime {
    /// 已处理轮次台账路径。
    #[must_use]
    pub fn processed_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref()
            .join("data")
            .join("runtime")
            .join(REFLECTION_PROCESSED_FILE)
    }

    /// 出池未完成批次路径。
    #[must_use]
    pub fn inflight_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref()
            .join("data")
            .join("runtime")
            .join(REFLECTION_INFLIGHT_FILE)
    }

    /// 批次已写候选台账路径。
    #[must_use]
    pub fn written_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref()
            .join("data")
            .join("runtime")
            .join(REFLECTION_WRITTEN_FILE)
    }

    /// 这一轮的反思是否已经**跑完**。只有 `done` 才算数：`started` 表示「跑过但没跑完」，
    /// 重试时允许再跑（重复写入由 `written` 台账的候选指纹挡住）。
    #[must_use]
    pub fn is_done(path: &Path, turn_key: &str) -> bool {
        let Ok(body) = std::fs::read_to_string(path) else {
            return false;
        };
        body.lines().any(|line| {
            serde_json::from_str::<ProcessedTurn>(line)
                .map(|record| {
                    record.turn_key == turn_key
                        // 空状态是旧版写的「已跑完」，按 done 兼容。
                        && (record.state.is_empty() || record.state == PROCESSED_DONE)
                })
                .unwrap_or(false)
        })
    }

    /// 记下「这一轮开始跑了」。写失败只返回错误，由调用方决定是否告警。
    pub fn mark_started(path: &Path, turn_key: &str, now_ms: u64) -> Result<(), String> {
        Self::upsert_state(path, turn_key, PROCESSED_STARTED, now_ms)
    }

    /// 记下「这一轮跑完了」。只有跑完的轮次才会在重试时被整条跳过。
    pub fn mark_done(path: &Path, turn_key: &str, now_ms: u64) -> Result<(), String> {
        Self::upsert_state(path, turn_key, PROCESSED_DONE, now_ms)
    }

    /// 覆盖式写入某个 key 的状态：先剔掉旧行再追加，保证同一 key 只有一行。
    fn upsert_state(path: &Path, turn_key: &str, state: &str, now_ms: u64) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{error:?}"))?;
        }
        let current = std::fs::read_to_string(path).unwrap_or_default();
        let mut kept: Vec<&str> = current
            .lines()
            .filter(|line| {
                !serde_json::from_str::<ProcessedTurn>(line)
                    .map(|record| record.turn_key == turn_key)
                    .unwrap_or(false)
            })
            .collect();
        let line = serde_json::to_string(&ProcessedTurn {
            turn_key: turn_key.to_owned(),
            state: state.to_owned(),
            updated_at_ms: now_ms,
        })
        .map_err(|error| format!("{error:?}"))?;
        kept.push(&line);
        let mut body = kept.join("\n");
        body.push('\n');
        std::fs::write(path, body).map_err(|error| format!("{error:?}"))
    }

    /// 这个候选在该批次里是否已经写过。半批失败后的重试靠它跳过已写条目。
    #[must_use]
    pub fn is_written(path: &Path, batch_key: &str, digest: &str) -> bool {
        let Ok(body) = std::fs::read_to_string(path) else {
            return false;
        };
        body.lines().any(|line| {
            serde_json::from_str::<WrittenCandidate>(line)
                .map(|record| record.batch_key == batch_key && record.digest == digest)
                .unwrap_or(false)
        })
    }

    /// 记下「这个候选已经在本次批次里写进库了」。失败只返回错误，不阻断主流程。
    pub fn mark_written(
        path: &Path,
        batch_key: &str,
        digest: &str,
        now_ms: u64,
    ) -> Result<(), String> {
        if Self::is_written(path, batch_key, digest) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{error:?}"))?;
        }
        let line = serde_json::to_string(&WrittenCandidate {
            batch_key: batch_key.to_owned(),
            digest: digest.to_owned(),
            written_at_ms: now_ms,
        })
        .map_err(|error| format!("{error:?}"))?;
        let mut body = std::fs::read_to_string(path).unwrap_or_default();
        body.push_str(&line);
        body.push('\n');
        std::fs::write(path, body).map_err(|error| format!("{error:?}"))
    }

    /// 记下「这一批已出池」。写失败不阻断主流程：最坏情况是崩溃后这批卡片丢失。
    pub fn write_inflight(path: &Path, entries: &[ReflectionPoolEntry]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{error:?}"))?;
        }
        let body = serde_json::to_string(entries).map_err(|error| format!("{error:?}"))?;
        std::fs::write(path, body).map_err(|error| format!("{error:?}"))
    }

    /// 取走未完成的批次：读到就删除文件，保证只恢复一次。
    #[must_use]
    pub fn take_inflight(path: &Path) -> Vec<ReflectionPoolEntry> {
        let Ok(body) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let _ = std::fs::remove_file(path);
        serde_json::from_str(&body).unwrap_or_default()
    }

    /// 清掉未完成标记：只在 inflight 里确实含有这一轮的卡片时才删。
    ///
    /// 不加条件地删会误清另一个 worker 的在途批次，每个治理 payload 各起一个
    /// 进程，两个 worker 的批次互不相干，谁跑完谁清自己的。
    pub fn clear_inflight(path: &Path, turn_id: &str) {
        let Ok(body) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(entries) = serde_json::from_str::<Vec<ReflectionPoolEntry>>(&body) else {
            return;
        };
        if entries.iter().any(|entry| entry.turn_id == turn_id) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 轮次状态：开始跑（未必跑完）。
pub const PROCESSED_STARTED: &str = "started";
/// 轮次状态：跑完了。只有这个状态会在重试时整条跳过。
pub const PROCESSED_DONE: &str = "done";

/// 轮次台账的一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessedTurn {
    turn_key: String,
    /// 空串是旧版遗留，按 `done` 兼容。
    #[serde(default)]
    state: String,
    updated_at_ms: u64,
}

/// 批次里已经写进库的候选：批次键 + 内容指纹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WrittenCandidate {
    batch_key: String,
    digest: String,
    written_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "axon-reflection-ledger-{}-{name}.json",
            std::process::id()
        ))
    }

    #[test]
    fn processed_turns_are_idempotent_by_key() {
        let path = scratch("processed");
        let _ = std::fs::remove_file(&path);

        assert!(!ReflectionLedgerRuntime::is_done(&path, "s:t1"));
        ReflectionLedgerRuntime::mark_started(&path, "s:t1", 1).expect("首写");
        // 「开跑」不算「跑完」：崩在子代理中途的轮次必须允许重跑。
        assert!(
            !ReflectionLedgerRuntime::is_done(&path, "s:t1"),
            "started 不得被当成 done"
        );
        ReflectionLedgerRuntime::mark_started(&path, "s:t1", 2).expect("重复写");

        let body = std::fs::read_to_string(&path).expect("台账可读");
        assert_eq!(body.lines().count(), 1, "同一个 key 只能留一行");

        ReflectionLedgerRuntime::mark_done(&path, "s:t1", 3).expect("收口");
        assert!(ReflectionLedgerRuntime::is_done(&path, "s:t1"));
        assert!(!ReflectionLedgerRuntime::is_done(&path, "s:t2"));
        let body = std::fs::read_to_string(&path).expect("台账可读");
        assert_eq!(body.lines().count(), 1, "done 必须覆盖 started，不留两行");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn inflight_batch_is_taken_exactly_once() {
        let path = scratch("inflight");
        let _ = std::fs::remove_file(&path);
        let entry = crate::ReflectionPoolEntry {
            turn_id: "turn-1".to_owned(),
            trace_id: "trace:s:0".to_owned(),
            enqueued_at_ms: 1,
            card: crate::ReflectionCard {
                card_id: "card:trace:s:0:0".to_owned(),
                trace_id: "trace:s:0".to_owned(),
                segment_index: 0,
                segment_count: 1,
                when: crate::WhenBlock {
                    started_at_ms: 1,
                    duration_ms: 0,
                    hour_bucket: crate::HourBucket::Afternoon,
                },
                location: crate::WhereBlock {
                    cwd_ref: "E:/a".to_owned(),
                    workspace_kind: "project".to_owned(),
                },
                what: crate::WhatBlock {
                    intent_ref: "message:user:1".to_owned(),
                    intent_chars: 0,
                    intent_digest: String::new(),
                    outcome_ref: "message:assistant:1".to_owned(),
                    outcome_chars: 0,
                    outcome_digest: String::new(),
                    final_status: "completed".to_owned(),
                },
                evidence_range: crate::EvidenceRange {
                    first_ref: String::new(),
                    last_ref: String::new(),
                    count: 0,
                    verified_count: 0,
                    rejected_count: 0,
                },
                top_evidence: Vec::new(),
                signals: crate::SalienceVector::default(),
                batch: crate::BatchBlock {
                    window_started_at_ms: 1,
                    window_ended_at_ms: 1,
                    card_count: 1,
                    accumulated_salience: 0,
                },
                hints: Vec::new(),
            },
            bucket: "E:/a".to_owned(),
            correction_bp: 0,
        };

        ReflectionLedgerRuntime::write_inflight(&path, std::slice::from_ref(&entry))
            .expect("写批次");
        assert_eq!(ReflectionLedgerRuntime::take_inflight(&path), vec![entry]);
        assert!(
            ReflectionLedgerRuntime::take_inflight(&path).is_empty(),
            "未完成批次只能恢复一次"
        );
    }
}
