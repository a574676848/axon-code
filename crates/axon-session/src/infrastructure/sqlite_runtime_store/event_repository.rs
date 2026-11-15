//! Session Event repository 边界。
use super::SESSION_EVENT_MISSING_ARCHIVE_TYPE;
use super::SqliteRuntimeStore;
use super::event_payload_store;
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
    // 交互归属：带租约的写入把 Interaction 标识落到列上，历史帧查询据此定向取帧。
    // 会话级事件没有归属，列值保持空串，读取侧自动回退整会话扫描。
    let interaction_column = interaction_lease.map(|(id, _)| id.as_str()).unwrap_or("");
    {
        let mut statement = transaction
            .prepare_cached(
                "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref,interaction_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(storage)?;
        for (event_type, payload_json) in events {
            next_sequence = next_sequence
                .checked_add(1)
                .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
            let payload_ref = event_payload_store::persist(
                &archive_root,
                session_id,
                next_sequence,
                event_type,
                payload_json,
            );
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
                    payload_ref,
                    interaction_column
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

/// 一页原始事件行：`(session_id, sequence, generation, event_type, payload_json, emitted_at_ms, payload_ref)`。
type RawEventRow = (String, i64, String, String, String, i64, Option<String>);

/// 一页原始行的列读取顺序，三个查询共用。
fn raw_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEventRow> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, i64>(1)?,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, String>(4)?,
        row.get::<_, i64>(5)?,
        row.get::<_, Option<String>>(6)?,
    ))
}

/// 把一页原始行展开成带正文的事件记录。
///
/// 调用方必须先把整页读进内存再交进来：档案读取是文件 I/O，占着数据库连接与
/// 语句做文件读取会让同一库的其他读者按 `busy_timeout` 排队。
fn read_event_records(
    store: &SqliteRuntimeStore,
    raw: Vec<RawEventRow>,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
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
        let (event_type, payload_json) = match payload_ref {
            Some(reference) => match event_payload_store::read(&archive_root, &reference) {
                Ok(content) => (event_type, content),
                /*
                 * 任何读取失败都只对这一帧降级为占位记录：保留序号与游标推进，
                 * 避免单个外置正文缺失把同一车道的其他交互乃至整条会话一起拖垮。
                 *
                 * 之前只认 `NotFound`，其余错误（verbatim 路径下的非法名、权限、
                 * 编码等）直接让整页历史帧失败，表现为网关返回 404、界面报
                 * 「网关不含该接口，后台服务版本可能过旧」。外置正文属于可重建的
                 * 附属事实，不值得用它拖垮主历史。
                 */
                Err(error) => (
                    SESSION_EVENT_MISSING_ARCHIVE_TYPE.to_owned(),
                    serde_json::json!({
                        "archiveMissing": true,
                        "payloadRef": reference,
                        "originalEventType": event_type,
                        "archiveError": {
                            "kind": format!("{:?}", error.kind()),
                            "message": error.to_string(),
                        },
                    })
                    .to_string(),
                ),
            },
            None => (event_type, inline_payload),
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

/// 读取 cursor 之后按序号升序排列的 Session 事件。
pub(super) fn replay_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    after_sequence: u64,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let raw = {
        let connection = store.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref \
                 FROM session_events \
                 WHERE session_id=?1 AND sequence>?2 \
                 ORDER BY sequence ASC LIMIT ?3",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(
                params![
                    session_id.as_str(),
                    i64_value(after_sequence)?,
                    i64::from(limit)
                ],
                raw_event_row,
            )
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };
    read_event_records(store, raw)
}

/// 读取某条 Interaction 自身的尾窗帧，按序号升序。
///
/// 从尾往前取：首屏一次就拿到可渲染的尾部窗口，不必先探整条会话的尾序号再算
/// 窗口起点。老交互的帧序号远小于整条会话的尾序号，先探再算会把整个老交互截空，
/// 这条路径从根上避免那个问题。
pub(super) fn interaction_tail_window(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &InteractionId,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let raw = {
        let connection = store.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref \
                 FROM ( \
                     SELECT session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref \
                     FROM session_events \
                     WHERE session_id=?1 AND interaction_id=?2 \
                     ORDER BY sequence DESC LIMIT ?3 \
                 ) \
                 ORDER BY sequence ASC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(
                params![
                    session_id.as_str(),
                    interaction_id.as_str(),
                    i64::from(limit)
                ],
                raw_event_row,
            )
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };
    read_event_records(store, raw)
}
