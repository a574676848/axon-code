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
use axon_session::RuntimeStore as _;
use serde_json::{Value, json};

/// 会话路由前缀。
const SESSIONS_PATH: &str = "/api/v1/sessions";
/// 会话清单的扫描上限：工作区过滤在内存里做，扫描窗口必须明显大于单页上限，
/// 否则先截断再过滤会让「该工作区有会话」在长列表尾部时返回空页。
pub(crate) const SESSION_SCAN_LIMIT: i64 = 4_096;
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
///
/// 三个字段都可缺省，缺省即「这一项不改」：标题与置顶归档是同一张表上的三列，
/// 一次请求只写调用方真正改过的那几列。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchSessionBody {
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    archived: Option<bool>,
    /// 展示标题；桌面重命名写的就是这一列。
    #[serde(default)]
    title: Option<String>,
    /// 把 `--continue` 的恢复指针指向这条会话。
    ///
    /// 只有主会话会移动指针：cron、子代理与目标执行的会话不是 `--continue` 的落点，
    /// 这里按类型跳过并在响应里说明，不假装写成。
    #[serde(default)]
    resume: Option<bool>,
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

    /// 更新会话的置顶、归档与标题。
    ///
    /// 三列都走 COALESCE：缺省保持原值，因此置顶、归档与重命名可以各自单独下发，
    /// 不会因为并行改一项就把另一项写回旧值。
    fn patch(&self, session_id: &str, body: &[u8]) -> GatewayHttpResponse {
        let Ok(payload) = serde_json::from_slice::<PatchSessionBody>(body) else {
            return GatewayHttpResponse::json(400, &json!({"error": "invalid_body"}));
        };
        if payload.pinned.is_none()
            && payload.archived.is_none()
            && payload.title.is_none()
            && payload.resume.is_none()
        {
            return GatewayHttpResponse::json(400, &json!({"error": "empty_patch"}));
        }
        let title = payload.title.as_deref().map(str::trim);
        if title.is_some_and(str::is_empty) {
            // 空标题会把车道退化成没有名字：拒绝比静默接受更诚实。
            return GatewayHttpResponse::json(400, &json!({"error": "empty_title"}));
        }
        // 适配器持有的是运行态根，不是数据目录：这里必须走同一份根到库的解析，
        // 否则会把库开到根目录上、在一个空库里做写入，现象是「会话不存在」。
        let store = crate::workbench_http_adapter::runtime_store(&self.runtime_root);
        if store.initialize().is_err() {
            return GatewayHttpResponse::json(500, &json!({"error": "runtime_unavailable"}));
        }
        let changed = store.with_read_connection(|connection| {
            connection.execute(
                "UPDATE sessions
                    SET pinned=COALESCE(?1, pinned), archived=COALESCE(?2, archived),
                        title=COALESCE(?3, title)
                  WHERE session_id=?4",
                rusqlite::params![
                    payload.pinned.map(|value| i64::from(u8::from(value))),
                    payload.archived.map(|value| i64::from(u8::from(value))),
                    title,
                    session_id
                ],
            )
        });
        match changed {
            Ok(0) => GatewayHttpResponse::json(404, &json!({"error": "session_not_found"})),
            Ok(_) => {
                let resume = match payload.resume {
                    Some(true) => match self.move_resume_pointer(&store, session_id) {
                        Ok(state) => Some(state),
                        Err(response) => return response,
                    },
                    _ => None,
                };
                GatewayHttpResponse::json(
                    200,
                    &json!({
                        "sessionId": session_id,
                        "pinned": payload.pinned,
                        "archived": payload.archived,
                        "title": title,
                        "resume": resume,
                    }),
                )
            }
            Err(_) => GatewayHttpResponse::json(500, &json!({"error": "update_failed"})),
        }
    }

    /// 把 `--continue` 的恢复指针指向这条会话；非主会话如实回报「跳过」。
    ///
    /// 指针是控制面的单行状态，写失败必须让调用方看见：静默失败会让 `--continue`
    /// 悄悄落到上一条会话，比报错更难排查。
    fn move_resume_pointer(
        &self,
        store: &axon_session::SqliteRuntimeStore,
        session_id: &str,
    ) -> Result<&'static str, GatewayHttpResponse> {
        let kind = store.with_read_connection(|connection| {
            connection.query_row(
                "SELECT kind FROM sessions WHERE session_id=?1",
                rusqlite::params![session_id],
                |row| row.get::<_, String>(0),
            )
        });
        match kind {
            Ok(kind) if kind != "primary" => Ok("skipped_non_primary"),
            Ok(_) => {
                let Ok(session) = axon_domain::DaemonSessionId::new(session_id.to_owned()) else {
                    return Err(GatewayHttpResponse::json(
                        400,
                        &json!({"error": "invalid_session_id"}),
                    ));
                };
                match axon_session::ResumePointerStore::write_resume_pointer(
                    store,
                    &session,
                    axon_time::unix_now_ms(),
                ) {
                    Ok(()) => Ok("written"),
                    Err(_) => Err(GatewayHttpResponse::json(
                        500,
                        &json!({"error": "resume_pointer_failed"}),
                    )),
                }
            }
            Err(_) => Err(GatewayHttpResponse::json(
                500,
                &json!({"error": "session_kind_unreadable"}),
            )),
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
    /// 会话表面渠道（首条入站，客户端优先）。
    ///
    /// 这是会话归属的那一端，界面据此判定是否独立会话（Cron / 子代理 / 目标执行只读）；
    /// 最近一轮是谁发的不进这一列，避免同一会话被别端续聊后界面把它当成另一种会话。
    pub(crate) channel: String,
    /// 会话表面去向（首条入站的回复路由）。
    pub(crate) destination: String,
    /// 最近一轮的来源渠道；只用来说明「最后一次是谁说的」。
    pub(crate) last_channel: String,
    /// 受众边界；由 `trust_scope` 映射成可展示标签。
    pub(crate) audience: String,
    /// 该会话已有的入站条数。
    pub(crate) interaction_count: u64,
    /// 最近一轮实际使用的 Provider；从未跑过轮次的会话为空。
    pub(crate) provider: String,
    /// 最近一轮实际观测到的上游模型 id。
    pub(crate) model: String,
    /// 用户当时选择的模型别名；展示与恢复优先用它。
    pub(crate) model_name: Option<String>,
    /// 是否有一条 Interaction 正在处理中。
    ///
    /// 与领域口径同源：`session_query_service` 用「`work_items.status = 'running'` 的轮次
    /// 计数大于零」定义运行态。桌面左侧菜单靠这一位决定会话标题前的状态槽画不画转圈，
    /// 因此它必须随清单一起下发，而不是留给客户端按 `status` 去猜：`sessions.status`
    /// 在运行期不会被改写，按它判会把所有会话永远读成「没在跑」。
    pub(crate) running: bool,
    /// 当前待答复的提问数。
    ///
    /// `user_questions` 落在 outbox 库，与这份清单不是同一个文件，因此这一列只能在
    /// 清单之外补一次聚合（见 `read_session_rows`）；补不到时按 0 处理，角标不亮。
    pub(crate) pending_question_count: u64,
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
            // 会话不存渠道与目标（渠道属于轮次）：这里给出会话表面的那一端（首条入站）
            // 与最近一轮的来源，供桌面的会话条目浮层与会话视图分别使用。
            "channel": self.channel,
            "destination": self.destination,
            "lastChannel": self.last_channel,
            "audience": self.audience,
            "interactionCount": self.interaction_count,
            // 会话最近一轮实际使用的模型：会话行只记最近一次，不是每轮的模型历史。
            "provider": self.provider,
            "model": self.model,
            "modelName": self.model_name,
            // 运行态：桌面标题前的状态槽与「进行中」浮层都读这一位。
            "running": self.running,
            // 待答提问：桌面标题前的琥珀角标读这一位。
            "pendingQuestionCount": self.pending_question_count,
        })
    }
}

/// 读取会话清单：置顶优先、再按最近活动倒序。
///
/// 读模型本体在 `sessions_list_read_model`：一条会话主查询加一条批量聚合查询，两次
/// 往返覆盖整页会话。这里只把入口留在原处，`axon sessions list` 与工作台投影共用它。
pub(crate) use self::sessions_list_read_model::read_session_rows;

/// 会话标题：控制面登记优先，其次首条入站正文，最后回落到工作目录。
///
/// 清单与索引页共用同一口径，避免同一条会话在两个入口显示成不同的名字。
pub(crate) fn display_session_title(
    stored: Option<String>,
    first_content: Option<String>,
    cwd: &str,
) -> String {
    stored
        .filter(|value| !value.trim().is_empty())
        .or_else(|| first_content.map(|content| clip_title(&content)))
        .unwrap_or_else(|| cwd.to_owned())
}

/// 受众边界的展示标签：表里存的是 `trust_scope` 取值域。
pub(crate) fn audience_label(trust_scope: &str) -> String {
    match trust_scope {
        "private" => "私密",
        "group_workspace" => "群聊工作区",
        "restricted" => "受限",
        other => other,
    }
    .to_owned()
}

/// 从作用域 JSON 中取出 Workspace 标识；全局作用域返回 `None`。
pub(crate) fn workspace_of_scope(scope_json: &str) -> Option<String> {
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
pub(crate) fn median_of_histogram(histogram_json: &str) -> Option<u64> {
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

#[cfg(test)]
#[path = "sessions_list_read_model_equivalence_tests.rs"]
mod equivalence_tests;

/// 会话清单读模型：一条会话主查询加一条批量聚合查询。
#[path = "sessions_list_read_model.rs"]
mod sessions_list_read_model;
