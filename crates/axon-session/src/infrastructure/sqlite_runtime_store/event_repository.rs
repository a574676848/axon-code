//! Session Event repository 边界。
use super::SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES;
use super::SqliteRuntimeStore;
use super::helpers::{i64_value, parse_session_id, storage, u64_value};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, InteractionId};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
/// Event cursor repository 视图。
#[derive(Debug, Clone, Copy)]
pub struct EventRepository<'a, S> {
    /// 底层运行时事实源。
    pub store: &'a S,
}
impl<'a, S: RuntimeStore> EventRepository<'a, S> {
    /// 创建 repository 视图。
    #[must_use]
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }
    /// 读取 cursor 后的事件。
    pub fn replay(
        &self,
        session_id: &DaemonSessionId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<crate::domain::SessionEventRecord>, RuntimeStoreError> {
        self.store.replay_events(session_id, after, limit)
    }
}

/// 追加 Session 事件并推进事件序号。
pub(super) fn append_event(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    event_type: &str,
    payload_json: &str,
    now_ms: u64,
) -> Result<SessionEventRecord, RuntimeStoreError> {
    let event = (event_type, payload_json);
    let mut records = append_event_refs(
        store,
        session_id,
        host,
        std::iter::once(event),
        now_ms,
        None,
    )?;
    records
        .pop()
        .ok_or_else(|| RuntimeStoreError::Storage("单事件批次未返回持久化记录".to_owned()))
}

/// 在同一事务中按输入顺序追加一批 Session 事件。
pub(super) fn append_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    append_event_refs(
        store,
        session_id,
        host,
        events
            .iter()
            .map(|(event_type, payload_json)| (event_type.as_str(), payload_json.as_str())),
        now_ms,
        None,
    )
}

pub(super) fn append_interaction_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    interaction_id: &InteractionId,
    lease_token: &str,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    append_event_refs(
        store,
        session_id,
        host,
        events
            .iter()
            .map(|(event_type, payload_json)| (event_type.as_str(), payload_json.as_str())),
        now_ms,
        Some((interaction_id, lease_token)),
    )
}

fn append_event_refs<'a>(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: impl ExactSizeIterator<Item = (&'a str, &'a str)>,
    now_ms: u64,
    interaction_lease: Option<(&InteractionId, &str)>,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    if let Some((interaction_id, lease_token)) = interaction_lease {
        let active = transaction
            .query_row(
                "SELECT 1 FROM jarvis_interactions WHERE interaction_id=?1 AND status='processing' AND lease_token=?2 AND lease_expires_at_ms>?3",
                params![interaction_id.as_str(), lease_token, i64_value(now_ms)?],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage)?;
        if active.is_none() {
            return Err(RuntimeStoreError::Fenced);
        }
    }
    let previous = transaction
        .query_row(
            "SELECT last_event_sequence FROM session_runtimes WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| RuntimeStoreError::NotFound(session_id.to_string()))?;
    let mut next_sequence = u64::try_from(previous)
        .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?;
    let emitted_at_ms = i64_value(now_ms)?;
    let mut records = Vec::with_capacity(events.len());
    let archive_root = store.event_archive_root();
    /*
     * 工具动作摘要：历史列表不读事件流，顺手把这一批里的工具名记进该轮记录。
     *
     * 只有带 Interaction 租约的写入才有归属，会话级事件不参与。名字先在本批内去重，
     * 落库那一条 SQL 再按列内已有内容判重，所以一轮里反复用同一个工具只会记一次。
     */
    let mut tool_names: Vec<String> = Vec::new();
    {
        let mut statement = transaction
            .prepare_cached(
                "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(storage)?;
        for (event_type, payload_json) in events {
            next_sequence = next_sequence
                .checked_add(1)
                .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
            let payload_ref =
                persist_event_payload(&archive_root, session_id, next_sequence, payload_json)?;
            // 正文已外置的行把内联列清空：正文只有一份真相，读侧一律按引用取回。
            let inline_payload = if payload_ref.is_some() {
                ""
            } else {
                payload_json
            };
            statement
                .execute(params![
                    session_id.as_str(),
                    i64_value(next_sequence)?,
                    host.generation,
                    event_type,
                    inline_payload,
                    emitted_at_ms,
                    payload_ref
                ])
                .map_err(storage)?;
            records.push(SessionEventRecord {
                session_id: session_id.clone(),
                sequence: next_sequence,
                generation: host.generation.clone(),
                event_type: event_type.to_owned(),
                payload_json: payload_json.to_owned(),
                emitted_at_ms: now_ms,
            });
            // 只在这一轮持有交互租约时记工具名：工具名来自事件载荷，不落库就没有可聚合的列。
            if interaction_lease.is_some()
                && let Some(name) = tool_call_name(event_type, payload_json)
                && !tool_names.iter().any(|known| known == &name)
            {
                tool_names.push(name);
            }
        }
    }
    transaction
        .execute(
            "UPDATE session_runtimes SET last_event_sequence=?1,last_activity_at_ms=?2,owner_host_id=?3,owner_generation=?4 WHERE session_id=?5",
            params![
                i64_value(next_sequence)?,
                emitted_at_ms,
                host.host_id.as_str(),
                host.generation,
                session_id.as_str()
            ],
        )
        .map_err(storage)?;
    if let Some((interaction_id, _)) = interaction_lease {
        for name in &tool_names {
            transaction
                .execute(
                    "UPDATE jarvis_interactions SET tool_summary = CASE \
                        WHEN tool_summary = '' THEN ?1 \
                        WHEN instr(',' || tool_summary || ',', ',' || ?1 || ',') > 0 THEN tool_summary \
                        ELSE tool_summary || ',' || ?1 END \
                     WHERE interaction_id = ?2",
                    params![name, interaction_id.as_str()],
                )
                .map_err(storage)?;
        }
    }
    transaction.commit().map_err(storage)?;
    Ok(records)
}

/// 从一帧 wire 事件里取工具名；只认 `toolCall`，其余一律 None。
///
/// 先用子串做廉价筛选：绝大多数事件不是工具调用，不该为它们付一次 JSON 解析。
/// 判据必须带上字段名：`toolCallId` 里也含 `toolCall`，只搜裸词会把每一条工具
/// 结果帧都拖进 JSON 解析（实测一条长轮次里工具结果帧是工具调用的十倍）。
fn tool_call_name(event_type: &str, payload_json: &str) -> Option<String> {
    if event_type != "wire_event" || !payload_json.contains("\"type\":\"toolCall\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let kind = value.get("kind")?;
    if kind.get("type")?.as_str()? != "toolCall" {
        return None;
    }
    kind.get("payload")?
        .get("toolName")?
        .as_str()
        .map(str::to_owned)
}

/// 把超限的事件正文写到归档文件，返回相对引用。
///
/// 返回 `None` 表示正文留在库里：写文件失败时宁可多占一点数据库空间，也不能让一条
/// 已经发生的运行事件落不了库。
fn persist_event_payload(
    archive_root: &std::path::Path,
    session_id: &DaemonSessionId,
    sequence: u64,
    payload_json: &str,
) -> Result<Option<String>, RuntimeStoreError> {
    if payload_json.len() <= SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES {
        return Ok(None);
    }
    let relative = format!("{}/{sequence}.json", session_id.as_str());
    let directory = archive_root.join(session_id.as_str());
    if std::fs::create_dir_all(&directory).is_err() {
        return Ok(None);
    }
    match std::fs::write(
        directory.join(format!("{sequence}.json")),
        payload_json.as_bytes(),
    ) {
        Ok(()) => Ok(Some(relative)),
        Err(_) => Ok(None),
    }
}

/// 读取 cursor 之后按序号升序排列的 Session 事件。
pub(super) fn replay_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    after_sequence: u64,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    // 先把整页读进内存再逐条取正文：档案读取是文件 I/O，不能让它占着数据库连接与语句。
    let raw = {
        let connection = store.connection()?;
        let mut statement = connection.prepare("SELECT session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref FROM session_events WHERE session_id=?1 AND sequence>?2 ORDER BY sequence ASC LIMIT ?3").map_err(storage)?;
        let rows = statement
            .query_map(
                params![
                    session_id.as_str(),
                    i64_value(after_sequence)?,
                    i64::from(limit)
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };
    let archive_root = store.event_archive_root();
    let mut records = Vec::with_capacity(raw.len());
    for (
        raw_session_id,
        sequence,
        generation,
        event_type,
        inline_payload,
        emitted_at_ms,
        payload_ref,
    ) in raw
    {
        let payload_json = match payload_ref {
            Some(reference) => {
                std::fs::read_to_string(archive_root.join(&reference)).map_err(|_| {
                    RuntimeStoreError::Storage(format!("事件正文归档不可读：{reference}"))
                })?
            }
            None => inline_payload,
        };
        records.push(SessionEventRecord {
            session_id: parse_session_id(raw_session_id).map_err(storage)?,
            sequence: u64_value(sequence).map_err(storage)?,
            generation,
            event_type,
            payload_json,
            emitted_at_ms: u64_value(emitted_at_ms).map_err(storage)?,
        });
    }
    Ok(records)
}
