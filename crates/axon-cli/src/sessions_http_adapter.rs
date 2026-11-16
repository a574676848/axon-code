//! 会话列表 HTTP Gateway 适配器。
//!
//! 只读：`GET /api/v1/sessions` 返回当前运行态里的会话清单，供桌面左侧菜单与
//! TUI `/sessions` 共用。字段直接来自 `sessions JOIN session_metrics`，标题回落到
//! 该车道第一条入站消息，不扫事件流、不扫 usage 库。
//!
//! 排序固定为「置顶优先、再按最近活动倒序」，与三端展示顺序一致；`include_archived`
//! 默认关闭，归档会话只有显式请求才出现。

use std::collections::BTreeMap;
use std::path::PathBuf;

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_session::{RuntimeStore as _, SqliteRuntimeStore};
use serde_json::{Value, json};

/// 会话路由前缀。
const SESSIONS_PATH: &str = "/api/v1/sessions";
/// 单次返回的会话上限：运行态会话数量由用户实际使用决定，这里只防止异常放大。
const SESSION_LIST_LIMIT: i64 = 500;
/// TTFT 直方图桶上界（毫秒），与 `session_metrics` 的投影口径保持一致。
const TTFT_BOUNDS_MS: [u64; 15] = [
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000,
];
/// 直方图桶总数：上界桶 + 一个溢出桶。
const TTFT_BUCKETS: usize = 16;

/// 会话列表网关适配器。
pub(crate) struct SessionsHttpAdapter {
    runtime_root: PathBuf,
}

impl SessionsHttpAdapter {
    /// 以运行时根目录构造适配器。
    pub(crate) fn new(runtime_root: PathBuf) -> Self {
        Self { runtime_root }
    }

    fn list(&self, query: Option<&str>) -> Value {
        let filters = QueryFilters::parse(query);
        let store = SqliteRuntimeStore::from_root(&self.runtime_root);
        let rows = match read_session_rows(&store) {
            Ok(rows) => rows,
            // 库还没建出来、被锁或没有表：返回空列表而不是 500，调用方据此展示空态。
            Err(_) => return json!([]),
        };
        let sessions = rows
            .into_iter()
            .filter(|row| filters.matches(row))
            .take(usize::try_from(SESSION_LIST_LIMIT).unwrap_or(usize::MAX))
            .map(|row| row.to_json())
            .collect::<Vec<_>>();
        json!(sessions)
    }
}

impl GatewayHttpRouteHandler for SessionsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        if path == SESSIONS_PATH {
            return match method {
                "GET" => Some(GatewayHttpResponse::json(200, &self.list(query))),
                "POST" => Some(self.create(body)),
                _ => None,
            };
        }
        // 单会话写入：`/api/v1/sessions/<id>`。
        let session_id = path
            .strip_prefix(SESSIONS_PATH)?
            .strip_prefix('/')
            .filter(|rest| !rest.is_empty() && !rest.contains('/'))?;
        match method {
            "PATCH" => Some(self.patch(session_id, body)),
            _ => None,
        }
    }
}

/// `POST /api/v1/sessions` 的报文体。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionBody {
    /// 归属 Workspace；缺省时不绑定。
    #[serde(default)]
    workspace_id: Option<String>,
    /// 工作目录；缺省时沿用 Axon 根目录。
    #[serde(default)]
    cwd: Option<String>,
    /// 展示标题；缺省时由第一条入站消息回落。
    #[serde(default)]
    title: Option<String>,
}

/// `PATCH /api/v1/sessions/<id>` 的报文体。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchSessionBody {
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    archived: Option<bool>,
}

impl SessionsHttpAdapter {
    /// 新建一个空会话：只登记运行态行，第一条消息到来时才产生事件。
    fn create(&self, body: &[u8]) -> GatewayHttpResponse {
        let Ok(payload) = serde_json::from_slice::<CreateSessionBody>(body) else {
            return GatewayHttpResponse::json(400, &json!({"error": "invalid_body"}));
        };
        let store = SqliteRuntimeStore::from_root(&self.runtime_root);
        if store.initialize().is_err() {
            return GatewayHttpResponse::json(500, &json!({"error": "runtime_unavailable"}));
        }
        let session_id = format!("session-{}", uuid::Uuid::new_v4());
        let Ok(parsed) = axon_domain::DaemonSessionId::new(session_id.clone()) else {
            return GatewayHttpResponse::json(500, &json!({"error": "invalid_session_id"}));
        };
        let cwd = payload
            .cwd
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_default();
        let now_ms = axon_time::unix_now_ms();
        if store.create_session(&parsed, &cwd, now_ms).is_err() {
            return GatewayHttpResponse::json(500, &json!({"error": "create_failed"}));
        }
        let title = payload
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let workspace_id = payload
            .workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if title.is_some() || workspace_id.is_some() {
            let _ = store.with_read_connection(|connection| {
                connection
                    .execute(
                        "UPDATE sessions SET title=COALESCE(?1, title),
                             workspace_id=COALESCE(?2, workspace_id)
                         WHERE session_id=?3",
                        rusqlite::params![title, workspace_id, session_id.as_str()],
                    )
                    .map(|_| ())
            });
        }
        GatewayHttpResponse::json(
            200,
            &json!({
                "sessionId": session_id,
                "workspaceId": workspace_id,
                "createdAtMs": now_ms,
            }),
        )
    }

    /// 更新会话的置顶与归档状态。
    fn patch(&self, session_id: &str, body: &[u8]) -> GatewayHttpResponse {
        let Ok(payload) = serde_json::from_slice::<PatchSessionBody>(body) else {
            return GatewayHttpResponse::json(400, &json!({"error": "invalid_body"}));
        };
        if payload.pinned.is_none() && payload.archived.is_none() {
            return GatewayHttpResponse::json(400, &json!({"error": "empty_patch"}));
        }
        let store = SqliteRuntimeStore::from_root(&self.runtime_root);
        if store.initialize().is_err() {
            return GatewayHttpResponse::json(500, &json!({"error": "runtime_unavailable"}));
        }
        let changed = store.with_read_connection(|connection| {
            connection
                .execute(
                    "UPDATE sessions SET pinned=COALESCE(?1, pinned), archived=COALESCE(?2, archived)
                     WHERE session_id=?3",
                    rusqlite::params![
                        payload.pinned.map(|value| i64::from(u8::from(value))),
                        payload.archived.map(|value| i64::from(u8::from(value))),
                        session_id
                    ],
                )
                .map(|changed| changed)
        });
        match changed {
            Ok(0) => GatewayHttpResponse::json(404, &json!({"error": "session_not_found"})),
            Ok(_) => GatewayHttpResponse::json(
                200,
                &json!({
                    "sessionId": session_id,
                    "pinned": payload.pinned,
                    "archived": payload.archived,
                }),
            ),
            Err(_) => GatewayHttpResponse::json(500, &json!({"error": "update_failed"})),
        }
    }
}
/// 查询过滤条件：`workspace` 精确匹配，`include_archived` 决定是否包含归档会话。
#[derive(Debug, Default)]
struct QueryFilters {
    workspace_id: Option<String>,
    include_archived: bool,
}

impl QueryFilters {
    fn parse(query: Option<&str>) -> Self {
        let mut filters = Self::default();
        let Some(query) = query else {
            return filters;
        };
        for pair in query.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or_default();
            let value = parts.next().unwrap_or_default();
            match key {
                "workspace" if !value.is_empty() => {
                    filters.workspace_id = Some(percent_decode(value));
                }
                "include_archived" => {
                    filters.include_archived = matches!(value, "1" | "true" | "yes");
                }
                _ => {}
            }
        }
        filters
    }

    fn matches(&self, row: &SessionListRow) -> bool {
        if row.archived && !self.include_archived {
            return false;
        }
        match &self.workspace_id {
            Some(workspace) => row.workspace_id.as_deref() == Some(workspace.as_str()),
            None => true,
        }
    }
}

/// 最小百分号解码：只处理 `%XX` 与 `+`，不做字符集转换。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or_default();
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        decoded.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        decoded.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_owned())
}

/// 一行会话列表数据。
///
/// HTTP 路由与 `axon sessions list` 共用这一份投影：字段与 SQL 只在这里定义一次，
/// 避免两个出口对同一份运行态事实给出不同口径。
pub(crate) struct SessionListRow {
    pub(crate) session_id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) pinned: bool,
    pub(crate) archived: bool,
    pub(crate) workspace_id: Option<String>,
    pub(crate) last_activity_at_ms: u64,
    pub(crate) turn_count: u64,
    pub(crate) total_active_ms: u64,
    pub(crate) ttft_p50_ms: Option<u64>,
    pub(crate) ttft_max_ms: u64,
    pub(crate) open_job_count: u64,
}

impl SessionListRow {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "title": self.title,
            "status": self.status,
            "pinned": self.pinned,
            "archived": self.archived,
            "workspaceId": self.workspace_id,
            "lastActivityAtMs": self.last_activity_at_ms,
            "turnCount": self.turn_count,
            "totalActiveMs": self.total_active_ms,
            "ttftP50Ms": self.ttft_p50_ms,
            "ttftMaxMs": self.ttft_max_ms,
            "openJobCount": self.open_job_count,
        })
    }
}

/// 读取会话清单：置顶优先、再按最近活动倒序。
pub(crate) fn read_session_rows(
    store: &SqliteRuntimeStore,
) -> Result<Vec<SessionListRow>, axon_session::RuntimeStoreError> {
    store.with_read_connection(read_sessions)
}

fn read_sessions(connection: &rusqlite::Connection) -> rusqlite::Result<Vec<SessionListRow>> {
    let mut statement = connection.prepare(
        "SELECT s.session_id, s.cwd, s.status, s.pinned, s.archived, s.workspace_id, s.title,
                s.last_activity_at_ms,
                COALESCE(m.turn_count, 0), COALESCE(m.total_active_ms, 0),
                COALESCE(m.ttft_max_ms, 0), COALESCE(m.jobs_running, 0),
                m.ttft_histogram_json,
                (SELECT i.content FROM inbox AS i WHERE i.lane_id = s.session_id
                 ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1),
                (SELECT i.scope_json FROM inbox AS i WHERE i.lane_id = s.session_id
                 ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1)
         FROM sessions AS s
         LEFT JOIN session_metrics AS m ON m.session_id = s.session_id
         ORDER BY s.pinned DESC, s.last_activity_at_ms DESC, s.session_id ASC
         LIMIT ?1",
    )?;
    let rows = statement.query_map([SESSION_LIST_LIMIT], |row| {
        let cwd: String = row.get(1)?;
        let stored_workspace: Option<String> = row.get(5)?;
        let stored_title: Option<String> = row.get(6)?;
        let ttft_histogram_json: Option<String> = row.get(12)?;
        let first_content: Option<String> = row.get(13)?;
        let scope_json: Option<String> = row.get(14)?;
        let workspace_id = stored_workspace
            .filter(|value| !value.is_empty())
            .or_else(|| scope_json.as_deref().and_then(workspace_of_scope));
        Ok(SessionListRow {
            session_id: row.get(0)?,
            title: stored_title
                .filter(|value| !value.trim().is_empty())
                .or_else(|| first_content.map(|content| clip_title(&content)))
                .unwrap_or_else(|| cwd.clone()),
            status: row.get(2)?,
            pinned: row.get::<_, i64>(3)? != 0,
            archived: row.get::<_, i64>(4)? != 0,
            workspace_id,
            last_activity_at_ms: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
            turn_count: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
            total_active_ms: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
            ttft_max_ms: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
            open_job_count: u64::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
            ttft_p50_ms: ttft_histogram_json
                .as_deref()
                .and_then(median_of_histogram),
        })
    })?;
    rows.collect()
}

/// 从作用域 JSON 中取出 Workspace 标识；全局作用域返回 `None`。
fn workspace_of_scope(scope_json: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(scope_json).ok()?;
    match &parsed {
        serde_json::Value::Object(map) => map
            .get("workspace")
            .or_else(|| map.get("Workspace"))
            .and_then(|value| match value {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Object(inner) => inner
                    .get("workspace_id")
                    .or_else(|| inner.get("workspaceId"))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                _ => None,
            }),
        _ => None,
    }
}

/// 标题取第一行、压掉多余空白并截断到 60 个字符。
fn clip_title(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = collapsed.chars().take(60).collect::<String>();
    if collapsed.chars().count() > 60 {
        title.push('…');
    }
    title
}

/// 由直方图求 TTFT 中位数：取累计样本过半所在桶的中点。
fn median_of_histogram(histogram_json: &str) -> Option<u64> {
    let counts: Vec<u64> = serde_json::from_str(histogram_json).ok()?;
    let buckets: BTreeMap<usize, u64> = counts
        .into_iter()
        .take(TTFT_BUCKETS)
        .enumerate()
        .filter(|(_, count)| *count > 0)
        .collect();
    let total = buckets.values().sum::<u64>();
    if total == 0 {
        return None;
    }
    let target = total.div_ceil(2);
    let mut cumulative = 0_u64;
    for (index, count) in buckets {
        cumulative = cumulative.saturating_add(count);
        if cumulative >= target {
            return Some(bucket_midpoint(index));
        }
    }
    None
}

fn bucket_midpoint(index: usize) -> u64 {
    match (index, TTFT_BOUNDS_MS.get(index)) {
        (_, Some(upper)) => {
            let lower = if index == 0 {
                0
            } else {
                TTFT_BOUNDS_MS[index - 1]
            };
            lower + (upper.saturating_sub(lower)) / 2
        }
        // 溢出桶：以最后一个上界为下界，按两倍上界估中点。
        (_, None) => {
            let lower = TTFT_BOUNDS_MS[TTFT_BOUNDS_MS.len() - 1];
            lower + lower / 2
        }
    }
}

#[cfg(test)]
#[path = "sessions_http_adapter_tests.rs"]
mod tests;
