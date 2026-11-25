//! 会话清单的读模型：一条会话主查询加一条批量聚合查询。
//!
//! 清单原先在一条 SQL 里挂七个逐会话相关子查询：首条正文、首末作用域、首末渠道、
//! 入站条数与在跑轮次数。其中两个 `COUNT(*)` 与在跑轮次那条要按会话把整个 `inbox`
//! 分区扫一遍，首条正文还要读全文列，代价随每条会话的消息数线性增长，而清单本身
//! 只需要每条会话的少量标量。
//!
//! 现在拆成两次往返：主查询只读 `sessions` 与其累计读数；聚合查询按会话列表取入站事实，
//! 每条会话的首末行与条数走 `inbox_session_recent` 的前缀索引，在跑轮次改为一次聚合，
//! 由 `work_items` 的在跑部分索引驱动。正文只在首条那一行读，不随消息数放大。

use std::collections::HashMap;

use axon_session::{RuntimeStoreError, SqliteRuntimeStore};

use super::{
    SESSION_SCAN_LIMIT, SessionListRow, audience_label, display_session_title, median_of_histogram,
    workspace_of_scope,
};

/// 一条会话的入站聚合事实；没有入站消息的会话不出现在结果里。
#[derive(Debug, Default)]
struct InboxFacts {
    interaction_count: u64,
    /// 首条入站正文；标题回落的原料。
    first_content: Option<String>,
    /// 首条入站的表面渠道。
    first_channel: Option<String>,
    /// 首条入站的回复去向。
    first_destination: Option<String>,
    /// 末条入站的表面渠道。
    last_channel: Option<String>,
    /// 末条入站的作用域 JSON；Workspace 回落的原料。
    last_scope_json: Option<String>,
    /// 该会话名下仍在跑的轮次数。
    running_count: u64,
}

/// 主查询读出的一行原始列。
struct SessionBaseRow {
    session_id: String,
    cwd: String,
    status: String,
    pinned: bool,
    archived: bool,
    workspace_id: Option<String>,
    stored_title: Option<String>,
    last_activity_at_ms: u64,
    turn_count: u64,
    total_active_ms: u64,
    ttft_max_ms: u64,
    open_job_count: u64,
    ttft_p50_ms: Option<u64>,
    trust_scope: String,
    provider: String,
    model: String,
    model_name: Option<String>,
}

impl SessionBaseRow {
    /// 把主查询的一行与它的入站聚合合并成一条清单行。
    fn into_row(self, facts: InboxFacts) -> SessionListRow {
        let workspace_id = self
            .workspace_id
            .filter(|value| !value.is_empty())
            .or_else(|| {
                facts
                    .last_scope_json
                    .as_deref()
                    .and_then(workspace_of_scope)
            });
        SessionListRow {
            title: display_session_title(self.stored_title, facts.first_content, &self.cwd),
            session_id: self.session_id,
            status: self.status,
            pinned: self.pinned,
            archived: self.archived,
            workspace_id,
            last_activity_at_ms: self.last_activity_at_ms,
            turn_count: self.turn_count,
            total_active_ms: self.total_active_ms,
            ttft_p50_ms: self.ttft_p50_ms,
            ttft_max_ms: self.ttft_max_ms,
            open_job_count: self.open_job_count,
            channel: facts.first_channel.unwrap_or_default(),
            destination: facts.first_destination.unwrap_or_default(),
            last_channel: facts.last_channel.unwrap_or_default(),
            audience: audience_label(&self.trust_scope),
            interaction_count: facts.interaction_count,
            provider: self.provider,
            model: self.model,
            model_name: self.model_name,
            running: facts.running_count > 0,
            pending_question_count: 0,
        }
    }
}

/// 读取会话清单：置顶优先、再按最近活动倒序。
///
/// 待答提问数在这里补：`user_questions` 落在 outbox 库，与清单不是同一个文件，并不进
/// 那两条查询。整份清单只多一次聚合，不随会话数增长；读失败按「没有待答」处理，角标
/// 缺失比整份清单不可用轻。
pub(crate) fn read_session_rows(
    store: &SqliteRuntimeStore,
) -> Result<Vec<SessionListRow>, RuntimeStoreError> {
    let mut rows = store.with_read_connection(|connection| {
        let base = read_session_base(connection)?;
        if base.is_empty() {
            return Ok(Vec::new());
        }
        let session_ids = base
            .iter()
            .map(|row| row.session_id.as_str())
            .collect::<Vec<_>>();
        let facts = read_inbox_facts(connection, &session_ids)?;
        Ok(base
            .into_iter()
            .map(|row| {
                let merged = facts
                    .get(&row.session_id)
                    .map_or_else(InboxFacts::default, |facts| InboxFacts {
                        interaction_count: facts.interaction_count,
                        first_content: facts.first_content.clone(),
                        first_channel: facts.first_channel.clone(),
                        first_destination: facts.first_destination.clone(),
                        last_channel: facts.last_channel.clone(),
                        last_scope_json: facts.last_scope_json.clone(),
                        running_count: facts.running_count,
                    });
                row.into_row(merged)
            })
            .collect::<Vec<_>>())
    })?;
    let pending = store.pending_question_counts().unwrap_or_default();
    for row in &mut rows {
        row.pending_question_count = pending.get(&row.session_id).copied().unwrap_or(0);
    }
    Ok(rows)
}

/// 主查询：会话行与它的累计读数。
///
/// 只读两张按主键连接的表，不再逐会话回查入站事实；排序与上限保持原口径，
/// 因此工作区过滤仍在内存里做。
fn read_session_base(connection: &rusqlite::Connection) -> rusqlite::Result<Vec<SessionBaseRow>> {
    let mut statement = connection.prepare(
        "SELECT s.session_id, s.cwd, s.status, s.pinned, s.archived, s.workspace_id, s.title,
                s.last_activity_at_ms,
                COALESCE(m.turn_count, 0), COALESCE(m.total_active_ms, 0),
                COALESCE(m.ttft_max_ms, 0), COALESCE(m.jobs_running, 0),
                m.ttft_histogram_json,
                s.trust_scope, s.provider, s.model, s.model_name
           FROM sessions AS s
           LEFT JOIN session_metrics AS m ON m.session_id = s.session_id
          WHERE s.kind = 'primary'
          ORDER BY s.pinned DESC, s.last_activity_at_ms DESC, s.session_id ASC
          LIMIT ?1",
    )?;
    let rows = statement.query_map([SESSION_SCAN_LIMIT], |row| {
        let ttft_histogram_json: Option<String> = row.get(12)?;
        Ok(SessionBaseRow {
            session_id: row.get(0)?,
            cwd: row.get(1)?,
            status: row.get(2)?,
            pinned: row.get::<_, i64>(3)? != 0,
            archived: row.get::<_, i64>(4)? != 0,
            workspace_id: row.get(5)?,
            stored_title: row.get(6)?,
            last_activity_at_ms: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
            turn_count: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
            total_active_ms: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
            ttft_max_ms: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
            open_job_count: u64::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
            ttft_p50_ms: ttft_histogram_json.as_deref().and_then(median_of_histogram),
            trust_scope: row.get(13)?,
            provider: row.get::<_, Option<String>>(14)?.unwrap_or_default(),
            model: row.get::<_, Option<String>>(15)?.unwrap_or_default(),
            model_name: row.get(16)?,
        })
    })?;
    rows.collect()
}

/// 聚合查询：一次覆盖整页会话的入站事实。
///
/// 语句按会话列表驱动，每个会话的三类事实各走一次索引：
/// 入站条数与首末行都命中 `inbox_session_recent(session_id, received_at_ms, interaction_id)`，
/// 首末行的排序键与原相关子查询完全一致（时间相同时按交互标识升序或降序），因此取到的
/// 仍是同两条记录；正文只在首条那一行读取，不参与任何计数扫描。
///
/// 在跑轮次是这里唯一被批量化的部分：它原先按会话把 `inbox` 分区扫一遍再逐行回表
/// `work_items`，成本等于该会话的消息数。这里改成一次聚合，由
/// `work_items_stale_running(status='running')` 部分索引驱动，再按主键回表 `inbox` 取
/// 会话并限制在整页会话之内，成本是在跑轮次数，不是消息数。`LEFT JOIN` 与回表连接一起
/// 保住了原口径：只有挂着入站事实的轮次才算在跑，目标执行的工作项不算。
///
/// 试过把首末行也改成窗口函数批量取，SQLite 会把整段入站事实物化再排序：同样 200 会话
/// 各 1000 条入站事实下，窗口写法约 1.2 秒，按会话点查约 10 毫秒，因此首末行保留索引点查。
fn read_inbox_facts(
    connection: &rusqlite::Connection,
    session_ids: &[&str],
) -> rusqlite::Result<HashMap<String, InboxFacts>> {
    if session_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = (1..=session_ids.len())
        .map(|index| format!("(?{index})"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "WITH scoped(session_id) AS (VALUES {placeholders}),
              runs AS (
                  SELECT i.session_id, COUNT(*) AS running_count
                    FROM work_items AS w
                    JOIN inbox AS i ON i.interaction_id = w.inbox_id
                   WHERE w.status = 'running'
                     AND i.session_id IN (SELECT session_id FROM scoped)
                   GROUP BY i.session_id
              )
         SELECT sc.session_id,
                (SELECT COUNT(*) FROM inbox AS i WHERE i.session_id = sc.session_id),
                (SELECT CASE WHEN TRIM(i.client) = '' THEN i.source_channel ELSE i.client END
                   FROM inbox AS i WHERE i.session_id = sc.session_id
                  ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1),
                (SELECT json_extract(i.reply_route_json, '$.destination')
                   FROM inbox AS i WHERE i.session_id = sc.session_id
                  ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1),
                (SELECT CASE WHEN TRIM(i.client) = '' THEN i.source_channel ELSE i.client END
                   FROM inbox AS i WHERE i.session_id = sc.session_id
                  ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1),
                (SELECT i.scope_json FROM inbox AS i WHERE i.session_id = sc.session_id
                  ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1),
                (SELECT i.content FROM inbox AS i WHERE i.session_id = sc.session_id
                  ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1),
                COALESCE(r.running_count, 0)
           FROM scoped AS sc
           LEFT JOIN runs AS r ON r.session_id = sc.session_id"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            InboxFacts {
                interaction_count: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                first_channel: row.get(2)?,
                first_destination: row.get(3)?,
                last_channel: row.get(4)?,
                last_scope_json: row.get(5)?,
                first_content: row.get(6)?,
                running_count: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
            },
        ))
    })?;
    let mut facts = HashMap::with_capacity(session_ids.len());
    for row in rows {
        let (session_id, row_facts) = row?;
        facts.insert(session_id, row_facts);
    }
    Ok(facts)
}
