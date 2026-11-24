//! 会话事件日志的正文分级：旧交互的大载荷帧正文搬到归档，日志里只留标记。
//!
//! 分级按「交互」而不是按时间：保留期是整条会话的生死线，活跃长会话永远不会过期，
//! 而它的历史正文早就不是翻页要的东西，答案、工具名与读数已经落在 `turn_activity`
//! 投影里，正文可以离开日志、留一份可按需回读的归档。
//!
//! 两个阶段共用同一套判定（[`tiered_interactions`]）：dry-run 只算账、不落盘；执行阶段
//! 在日志写锁之外读快照与写归档，锁内只做「尾部未变」的乐观校验后重写日志。记录条数与
//! 序号在分级前后完全一致，因此这是一次纯粹的内容替换。

use std::collections::HashMap;
use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_eventlog::{EventLogReader, RuntimeEventRecord, rewrite_log_if_unchanged};

use super::SqliteRuntimeStore;
use crate::infrastructure::SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES;
use crate::infrastructure::session_event_body_archive::{
    body_archive_ref, body_marker_record, by_sequence, is_body_marker, is_body_record,
    payload_bytes, read_body_archive, reclaimed_placeholder, write_body_archive,
};
use crate::ports::RuntimeStoreError;

/// 事件正文分级的计划读数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventBodyTieringPlan {
    /// 将被分级的交互数。
    pub tiered_interactions: usize,
    /// 将被换成标记的帧数。
    pub tiered_frames: usize,
    /// 将离开日志的正文载荷字节数。
    pub tiered_bytes: u64,
    /// 分级前的日志字节数。
    pub log_bytes: u64,
}

/// 事件正文分级执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventBodyTieringReport {
    /// 已分级的交互数。
    pub tiered_interactions: usize,
    /// 已换成标记的帧数。
    pub tiered_frames: usize,
    /// 已写入归档的正文载荷字节数。
    pub tiered_bytes: u64,
    /// 归档实际落盘字节（压缩后）。
    pub archive_bytes: u64,
    /// 分级前的日志字节数。
    pub log_bytes_before: u64,
    /// 分级并重写后的日志字节数。
    pub log_bytes_after: u64,
    /// 归档写入失败、正文保持内联的交互数：下一轮重试。
    pub skipped_interactions: usize,
}

impl EventBodyTieringReport {
    /// 日志侧回收的字节数；重写后可能反而更大（帧边界变化），此时按 0 计。
    #[must_use]
    pub fn reclaimed_bytes(&self) -> u64 {
        self.log_bytes_before.saturating_sub(self.log_bytes_after)
    }

    /// 净回收字节数：日志减少量扣掉归档新增量。
    ///
    /// 归档是压缩后的副本，正常远小于原文，因此净值为正；把归档算进来是因为「日志变小、
    /// 磁盘变大」不该被记成回收。
    #[must_use]
    pub fn net_reclaimed_bytes(&self) -> u64 {
        self.reclaimed_bytes().saturating_sub(self.archive_bytes)
    }
}

/// 一条待分级交互：交互标识与它那些正文帧在记录集里的下标。
struct TieredInteraction {
    interaction_id: String,
    body_indices: Vec<usize>,
}

impl SqliteRuntimeStore {
    /// 只算账不落盘：返回 None 表示这条会话没有可分级的历史正文。
    pub fn plan_event_body_tiering(
        &self,
        session_id: &DaemonSessionId,
        keep_interactions: usize,
    ) -> Result<Option<EventBodyTieringPlan>, RuntimeStoreError> {
        let path = self.event_log(session_id).path();
        if !path.is_file() {
            return Ok(None);
        }
        let reader = EventLogReader::open(&path).map_err(log_error)?;
        let records = reader.records();
        let tiered = tiered_interactions(records, keep_interactions);
        if tiered.is_empty() {
            return Ok(None);
        }
        let frames = tiered.iter().fold(0usize, |sum, item| {
            sum.saturating_add(item.body_indices.len())
        });
        let bytes = tiered.iter().fold(0u64, |sum, item| {
            item.body_indices.iter().fold(sum, |inner, index| {
                inner.saturating_add(records.get(*index).map_or(0, payload_bytes) as u64)
            })
        });
        Ok(Some(EventBodyTieringPlan {
            tiered_interactions: tiered.len(),
            tiered_frames: frames,
            tiered_bytes: bytes,
            log_bytes: std::fs::metadata(&path).map_or(0, |meta| meta.len()),
        }))
    }

    /// 执行分级：写归档、重写日志、重建索引。
    ///
    /// 读快照与写归档都在日志写锁**之外**：把整条日志解压进内存可能持续数秒，按住主
    /// 写锁会让并发追加在锁超时（5 秒）后失败。锁内只做一次「末条序号是否仍是快照那
    /// 一条」的乐观校验，对不上就说明期间有人写过，本轮放弃、下一轮重来。
    ///
    /// 单会话体量上限见 [`MAX_TIERING_LOG_BYTES`]：超过就只告警不处理，避免一次治理把
    /// 进程内存打满。
    pub fn tier_event_body(
        &self,
        session_id: &DaemonSessionId,
        keep_interactions: usize,
    ) -> Result<Option<EventBodyTieringReport>, RuntimeStoreError> {
        let log = self.event_log(session_id);
        let log_path = log.path();
        if !log_path.is_file() {
            return Ok(None);
        }
        let log_bytes_before = std::fs::metadata(&log_path).map_or(0, |meta| meta.len());
        if log_bytes_before > MAX_TIERING_LOG_BYTES {
            axon_logger::warning(&format!(
                "会话事件日志超过分级上限，跳过本轮: session={} bytes={log_bytes_before} limit={MAX_TIERING_LOG_BYTES}",
                session_id.as_str()
            ));
            return Ok(None);
        }
        let reader = EventLogReader::open(&log_path).map_err(log_error)?;
        let header = reader.header().clone();
        let mut records = reader.records().to_vec();
        let Some(expected_tail) = records.last().map(|record| record.sequence) else {
            return Ok(None);
        };
        let archive_root = self.event_archive_root();
        let mut stats = TieringStats::default();
        if !apply_tiering(
            &mut records,
            session_id.as_str(),
            keep_interactions,
            &archive_root,
            &mut stats,
        ) {
            // 没有可改动的记录（或全部归档都写失败）：日志一字未动，但「跳过」要上报。
            if stats.skipped_interactions == 0 {
                return Ok(None);
            }
            return Ok(Some(EventBodyTieringReport {
                tiered_interactions: 0,
                tiered_frames: 0,
                tiered_bytes: 0,
                archive_bytes: 0,
                log_bytes_before,
                log_bytes_after: log_bytes_before,
                skipped_interactions: stats.skipped_interactions,
            }));
        }
        let Some(outcome) =
            rewrite_log_if_unchanged(&log, expected_tail, &header, &records).map_err(log_error)?
        else {
            // 读快照之后有人追加过：本次重写会盖掉那些帧，直接放弃。
            axon_logger::warning(&format!(
                "会话事件日志在分级期间被追加，本轮放弃: session={}",
                session_id.as_str()
            ));
            return Ok(None);
        };
        Ok(Some(EventBodyTieringReport {
            tiered_interactions: stats.tiered_interactions,
            tiered_frames: stats.tiered_frames,
            tiered_bytes: stats.tiered_bytes,
            archive_bytes: stats.archive_bytes,
            log_bytes_before,
            log_bytes_after: outcome.bytes,
            skipped_interactions: stats.skipped_interactions,
        }))
    }
}

/// 单条会话事件日志参与分级的体量上限。
///
/// 分级要把整条日志解压成记录集：几百 MiB 的日志对应 GiB 级内存，daemon 会在此时
/// 直接 abort。超过上限的会话本轮跳过并告警，宁可让它继续大，也不拿进程内存换。
const MAX_TIERING_LOG_BYTES: u64 = 256 * 1024 * 1024;

/// 执行阶段的累计读数。
#[derive(Debug, Clone, Copy, Default)]
struct TieringStats {
    tiered_interactions: usize,
    tiered_frames: usize,
    tiered_bytes: u64,
    /// 归档落盘字节（压缩后）：净回收要把这部分从日志减少量里扣掉。
    archive_bytes: u64,
    skipped_interactions: usize,
}

/// 就地把待分级交互的正文换成标记并写归档；返回是否真的改动过记录。
fn apply_tiering(
    records: &mut [RuntimeEventRecord],
    session_id: &str,
    keep_interactions: usize,
    archive_root: &Path,
    stats: &mut TieringStats,
) -> bool {
    let tiered = tiered_interactions(records, keep_interactions);
    let mut changed = false;
    for item in tiered {
        let bodies: Vec<RuntimeEventRecord> = item
            .body_indices
            .iter()
            .filter_map(|index| records.get(*index).cloned())
            .collect();
        let written =
            match write_body_archive(archive_root, session_id, &item.interaction_id, &bodies) {
                Ok(written) => written,
                Err(_) => {
                    // 归档没落盘就不能把正文从日志里拿走：保持内联，下一轮重来。
                    stats.skipped_interactions = stats.skipped_interactions.saturating_add(1);
                    continue;
                }
            };
        stats.archive_bytes = stats.archive_bytes.saturating_add(written);
        let archive_ref = body_archive_ref(session_id, &item.interaction_id);
        for (index, body) in item.body_indices.iter().zip(bodies.iter()) {
            if let Some(slot) = records.get_mut(*index) {
                *slot = body_marker_record(body, &archive_ref);
                changed = true;
            }
        }
        stats.tiered_interactions = stats.tiered_interactions.saturating_add(1);
        stats.tiered_frames = stats.tiered_frames.saturating_add(bodies.len());
        stats.tiered_bytes =
            stats
                .tiered_bytes
                .saturating_add(bodies.iter().fold(0u64, |sum, body| {
                    sum.saturating_add(payload_bytes(body) as u64)
                }));
    }
    changed
}

/// 决定哪些交互的正文离开日志：最近 `keep_interactions` 条之外、且还有正文可搬的那些。
fn tiered_interactions(
    records: &[RuntimeEventRecord],
    keep_interactions: usize,
) -> Vec<TieredInteraction> {
    let order = interaction_recentness(records);
    if order.len() <= keep_interactions {
        return Vec::new();
    }
    let mut tiered = Vec::new();
    for interaction_id in order.into_iter().skip(keep_interactions) {
        let body_indices: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record.interaction_id.as_deref() == Some(interaction_id.as_str())
                    && is_body_record(record, SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES)
            })
            .map(|(index, _)| index)
            .collect();
        if body_indices.is_empty() {
            continue;
        }
        tiered.push(TieredInteraction {
            interaction_id,
            body_indices,
        });
    }
    tiered
}

/// 交互按「最后一条事件时间」倒序：最近还在跑或刚跑完的交互排在前面。
///
/// 只有带归属的记录参与：会话级事件（`job_*`、`session_recovered`）不属于任何一轮，
/// 也不该因为排序被牵连。
fn interaction_recentness(records: &[RuntimeEventRecord]) -> Vec<String> {
    let mut latest: HashMap<&str, (u64, usize)> = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        let Some(interaction_id) = record
            .interaction_id
            .as_deref()
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let entry = latest.entry(interaction_id).or_insert((0, index));
        entry.0 = entry.0.max(record.emitted_at_ms);
        entry.1 = index;
    }
    let mut order: Vec<(&str, u64, usize)> = latest
        .into_iter()
        .map(|(interaction_id, (emitted, index))| (interaction_id, emitted, index))
        .collect();
    // 同一毫秒内按日志里最后一次出现的位置排序，保证同一份日志每次分级结论一致。
    order.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.2.cmp(&left.2)));
    order
        .into_iter()
        .map(|(interaction_id, _, _)| interaction_id.to_owned())
        .collect()
}

/// 把一段记录里被分级的正文还原成原文（按交互分组，每条交互的归档只读一次）。
///
/// 归档缺失（已被回收）时降级为占位记录：调用方据此如实上报「正文已回收」，而不是
/// 把这一轮当成没有正文。返回还原后的记录，顺序与入参一致。
#[must_use]
pub(crate) fn hydrate_archived_page(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    records: Vec<RuntimeEventRecord>,
) -> Vec<RuntimeEventRecord> {
    let is_marker: Vec<bool> = records.iter().map(is_body_marker).collect();
    if !is_marker.iter().any(|value| *value) {
        return records;
    }
    let archive_root = store.event_archive_root();
    let mut archived = HashMap::<String, Option<HashMap<u64, RuntimeEventRecord>>>::new();
    for (record, marker) in records.iter().zip(is_marker.iter()) {
        if !marker {
            continue;
        }
        let Some(interaction_id) = record.interaction_id.clone() else {
            continue;
        };
        archived.entry(interaction_id).or_insert_with_key(|owner| {
            match read_body_archive(&archive_root, session_id.as_str(), owner) {
                Ok(Some(bodies)) => Some(by_sequence(bodies)),
                Ok(None) => None,
                Err(error) => {
                    axon_logger::warning(&format!(
                        "会话正文归档不可读，回读按已回收处理: session={} interaction={owner} error={error}",
                        session_id.as_str()
                    ));
                    None
                }
            }
        });
    }
    let mut hydrated = Vec::with_capacity(records.len());
    for (record, marker) in records.into_iter().zip(is_marker) {
        if !marker {
            hydrated.push(record);
            continue;
        }
        let original = record
            .interaction_id
            .as_ref()
            .and_then(|owner| archived.get(owner))
            .and_then(|bodies| bodies.as_ref())
            .and_then(|bodies| bodies.get(&record.sequence))
            .cloned();
        match original {
            Some(original) => hydrated.push(original),
            None => {
                let archive_ref = record
                    .interaction_id
                    .as_deref()
                    .map(|owner| body_archive_ref(session_id.as_str(), owner))
                    .unwrap_or_default();
                hydrated.push(reclaimed_placeholder(&record, &archive_ref, "reclaimed"));
            }
        }
    }
    hydrated
}

/// 单条交互的记录集还原：分级标记只出现在带归属的内容帧上，按交互归并即可。
#[must_use]
pub(crate) fn hydrate_archived_records(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    _interaction_id: &str,
    records: Vec<RuntimeEventRecord>,
) -> Vec<RuntimeEventRecord> {
    hydrate_archived_page(store, session_id, records)
}

/// 事件面错误统一折进存储错误。
fn log_error(error: axon_eventlog::EventLogError) -> RuntimeStoreError {
    RuntimeStoreError::Storage(format!("会话事件日志读写失败：{error}"))
}
