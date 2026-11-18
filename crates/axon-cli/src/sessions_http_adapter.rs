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
/// 会话清单的扫描上限：工作区过滤在内存里做，扫描窗口必须明显大于单页上限，
/// 否则先截断再过滤会让「该工作区有会话」在长列表尾部时返回空页。
const SESSION_SCAN_LIMIT: i64 = 4_096;
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
}

impl GatewayHttpRouteHandler for SessionsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        if path == SESSIONS_PATH {
            // 清单读取归工作台投影：桌面契约要求每条会话带上会话表面的渠道、目标与受众，
            // 那份投影与 /workspaces 同源；本适配器只保留写入（创建、置顶、归档）。
            // GET 在这里返回 None，由后续处理器接管，避免两个投影抢同一路由。
            return match method {
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
    ///
    /// 标识由服务端生成 ULID，类型与受众边界在创建时写死，此后不变。
    fn create(&self, body: &[u8]) -> GatewayHttpResponse {
        let Ok(payload) = serde_json::from_slice::<CreateSessionBody>(body) else {
            return GatewayHttpResponse::json(400, &json!({"error": "invalid_body"}));
        };
        // 适配器持有的是运行态根，不是数据目录：这里必须走同一份根到库的解析，
        // 否则会把库开到根目录上、在一个空库里做写入，现象是「会话不存在」。
        let store = crate::workbench_http_adapter::runtime_store(&self.runtime_root);
        if store.initialize().is_err() {
            return GatewayHttpResponse::json(500, &json!({"error": "runtime_unavailable"}));
        }
        let session_id = axon_session::new_session_id();
        let cwd = payload
            .cwd
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_default();
        let now_ms = axon_time::unix_now_ms();
        let title = payload
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let workspace_id = payload
            .workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let request = axon_session::NewSessionRequest {
            session_id: session_id.clone(),
            kind: axon_session::SessionKind::Primary,
            trust_scope: axon_session::SessionTrustScope::Private,
            workspace_id: workspace_id.clone(),
            root_session_id: None,
            agent_path: None,
            goal_id: None,
            execution_id: None,
            cwd: cwd.clone(),
            now_ms,
        };
        if store.create_session_with(&request).is_err() {
            return GatewayHttpResponse::json(500, &json!({"error": "create_failed"}));
        }
        if let Some(title) = title {
            let _ = store.with_read_connection(|connection| {
                connection
                    .execute(
                        "UPDATE sessions SET title=?1 WHERE session_id=?2",
                        rusqlite::params![title, session_id.as_str()],
                    )
                    .map(|_| ())
            });
        }
        GatewayHttpResponse::json(
            200,
            &json!({
                "sessionId": session_id.as_str(),
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
        // 适配器持有的是运行态根，不是数据目录：这里必须走同一份根到库的解析，
        // 否则会把库开到根目录上、在一个空库里做写入，现象是「会话不存在」。
        let store = crate::workbench_http_adapter::runtime_store(&self.runtime_root);
        if store.initialize().is_err() {
            return GatewayHttpResponse::json(500, &json!({"error": "runtime_unavailable"}));
        }
        let changed = store.with_read_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET pinned=COALESCE(?1, pinned), archived=COALESCE(?2, archived)
                     WHERE session_id=?3",
                rusqlite::params![
                    payload.pinned.map(|value| i64::from(u8::from(value))),
                    payload.archived.map(|value| i64::from(u8::from(value))),
                    session_id
                ],
            )
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

/// 一行会话列表数据。
///
/// `axon sessions list` 的唯一投影：字段与 SQL 只在这里定义一次，命令行与桌面左侧
/// 菜单各取所需字段，但都来自同一批运行态事实。
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
    /// 最近一轮的来源渠道（客户端优先，与 `origin_channel` 同口径）。
    pub(crate) channel: String,
    /// 最近一轮的回复去向。
    pub(crate) destination: String,
    /// 受众边界；由 `trust_scope` 映射成可展示标签。
    pub(crate) audience: String,
    /// 该会话已有的入站条数。
    pub(crate) interaction_count: u64,
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
            // 会话不存渠道与目标（渠道属于轮次）：这里给出的是最近一轮的来源与去向，
            // 以及该会话已有的入站条数，供桌面的会话条目浮层直接展示。
            "channel": self.channel,
            "destination": self.destination,
            "audience": self.audience,
            "interactionCount": self.interaction_count,
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
                (SELECT i.content FROM inbox AS i WHERE i.session_id = s.session_id
                 ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1),
                (SELECT i.scope_json FROM inbox AS i WHERE i.session_id = s.session_id
                 ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1),
                COALESCE((SELECT CASE WHEN TRIM(i.client) = '' THEN i.source_channel ELSE i.client END
                          FROM inbox AS i WHERE i.session_id = s.session_id
                          ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1), ''),
                COALESCE((SELECT json_extract(i.reply_route_json, '$.destination')
                          FROM inbox AS i WHERE i.session_id = s.session_id
                          ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1), ''),
                s.trust_scope,
                (SELECT COUNT(*) FROM inbox AS i WHERE i.session_id = s.session_id)
         FROM sessions AS s
         LEFT JOIN session_metrics AS m ON m.session_id = s.session_id
         WHERE s.kind = 'primary'
         ORDER BY s.pinned DESC, s.last_activity_at_ms DESC, s.session_id ASC
         LIMIT ?1",
    )?;
    let rows = statement.query_map([SESSION_SCAN_LIMIT], |row| {
        let cwd: String = row.get(1)?;
        let stored_workspace: Option<String> = row.get(5)?;
        let stored_title: Option<String> = row.get(6)?;
        let ttft_histogram_json: Option<String> = row.get(12)?;
        let first_content: Option<String> = row.get(13)?;
        let scope_json: Option<String> = row.get(14)?;
        let trust_scope: String = row.get(17)?;
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
            ttft_p50_ms: ttft_histogram_json.as_deref().and_then(median_of_histogram),
            channel: row.get(15)?,
            destination: row.get(16)?,
            audience: audience_label(&trust_scope),
            interaction_count: u64::try_from(row.get::<_, i64>(18)?).unwrap_or(0),
        })
    })?;
    rows.collect()
}

/// 受众边界的展示标签：表里存的是 `trust_scope` 取值域。
fn audience_label(trust_scope: &str) -> String {
    match trust_scope {
        "private" => "私密",
        "group_workspace" => "群聊工作区",
        "restricted" => "受限",
        other => other,
    }
    .to_owned()
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
