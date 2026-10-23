//! 工作台路由：把工作区与车道以 HTTP 协议暴露给外部客户端。
//!
//! 两个网关面共用同一份载荷定义与读取逻辑：
//! `axon serve` 的 `/v1/*` 路由族，以及常驻 daemon 网关的 `/api/v1/*` 扩展路由。
//! 载荷在这里显式声明字段名并使用小驼峰，与既有协议保持一致；
//! 刻意不复用领域类型的序列化，避免为了协议改名而破坏落库数据。
//!
//! 工作区与车道都属于用户数据，因此**不得**加入任何免鉴权白名单：
//! daemon 网关在委派扩展路由前已经完成鉴权，`serve` 侧也只放行健康端点。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use axon_daemon::infrastructure::gateway_replay::interaction_history_page;
use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
};
use axon_session::{
    AXON_RUNTIME_DATA_DIR, InteractionCursor, InteractionQueryStore, LaneQueryService,
    LaneSnapshot, RuntimeStore, RuntimeStoreError, SqliteRuntimeStore, WorkspaceRecord,
    WorkspaceStore, session_lane_id,
};
use serde::Serialize;

use crate::run_commands::run_steer::{ActivePrimaryTurnRegistry, SubagentSendOutcome};

/// daemon 网关上的扩展路由前缀。
pub const WORKBENCH_PATH_PREFIX: &str = "/api/v1";
/// 协议版本；新增路由或改动字段语义时必须递增，供客户端协商。
pub const WORKBENCH_PROTOCOL_VERSION: u32 = 5;
/// 单次车道 Interaction 回放查询的默认条数上限。
const DEFAULT_LANE_INTERACTION_LIMIT: u32 = 200;
/// 单次车道 Interaction 回放查询的条数硬上限。
const MAX_LANE_INTERACTION_LIMIT: u32 = 1_000;
/// 历史帧页的默认帧数：一次取够一屏正文，又不至于让单次响应过大。
const DEFAULT_INTERACTION_EVENTS_LIMIT: u32 = 800;
/// 单页帧数硬上限。
const MAX_INTERACTION_EVENTS_LIMIT: u32 = 2_000;
/// 单条发往子代理的用户消息字符上限；与主会话输入保持同一量级。
const MAX_SUBAGENT_MESSAGE_CHARS: usize = 8_000;

#[path = "workbench_http_views.rs"]
mod workbench_http_views;

pub use workbench_http_views::{InteractionView, LaneView, WorkspaceView};

#[path = "workbench_http_targets.rs"]
mod workbench_http_targets;

#[cfg(test)]
pub(crate) use workbench_http_targets::owner_matches_lane;
use workbench_http_targets::{
    interaction_events_target, lane_interactions_target, lane_interrupt_target,
    lane_job_stop_target, lane_jobs_target, lane_question_answer_target, lane_questions_target,
    lane_subagent_message_target, lane_subagents_target, query_param,
};

#[path = "workbench_http_subagents_jobs.rs"]
mod workbench_http_subagents_jobs;

use workbench_http_subagents_jobs::{lane_jobs_payload, lane_subagents_payload, stop_lane_job};

#[cfg(test)]
#[path = "workbench_http_adapter_tests.rs"]
mod workbench_http_adapter_tests;

/// 车道内 Interaction 列表载荷。
///
/// `before` 是反向分页游标：为空取最近一页，给出则取它之前的一页。
/// 客户端按「一次一条」向前翻，因此这里不需要额外的页号或总量。
pub fn lane_interactions_payload(
    axon_root: &Path,
    lane_id: &str,
    limit: u32,
    before: Option<&InteractionCursor>,
) -> Result<serde_json::Value, String> {
    let limit = limit.clamp(1, MAX_LANE_INTERACTION_LIMIT);
    let records = runtime_store(axon_root)
        .interactions_for_lane(lane_id, limit, before)
        .map_err(|error| format!("读取车道 Interaction 失败：{error:?}"))?;
    let interactions: Vec<InteractionView> = records
        .iter()
        .map(|record| InteractionView {
            interaction_id: record.interaction_id.as_str().to_owned(),
            status: serde_json::to_value(record.status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned()),
            channel: record.source.channel().to_owned(),
            received_at_ms: record.received_at_ms,
            content: record.content.clone(),
            attachments: record.attachments.clone(),
        })
        .collect();
    Ok(serde_json::json!({
        "laneId": lane_id,
        "interactions": interactions,
        "totalRecords": interactions.len(),
        "rawPayloadPersisted": false,
    }))
}

/// 一条 Interaction 的历史帧页。
///
/// 正文一次取回，客户端读完历史后再从 `nextAfter` 起订阅实时尾：事件通道上因此
/// 只出现真正的增量，不再承担整段回放。
pub fn interaction_events_payload(
    axon_root: &Path,
    interaction_id: &str,
    after: u64,
    limit: u32,
) -> Result<serde_json::Value, String> {
    let limit = limit.clamp(1, MAX_INTERACTION_EVENTS_LIMIT);
    let id = InteractionId::new(interaction_id)
        .map_err(|error| format!("Interaction 标识非法：{error:?}"))?;
    let page = interaction_history_page(&runtime_store(axon_root), &id, after, limit)
        .map_err(|error| format!("读取历史帧失败：{error}"))?;
    let events: Vec<serde_json::Value> = page
        .events
        .iter()
        .map(|event| {
            serde_json::json!({
                "eventType": event.event_type,
                "payloadJson": event.payload_json,
                "cursor": event.cursor,
                "replay": event.replay,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "interactionId": interaction_id,
        "events": events,
        "nextAfter": page.next_after,
        "hasMore": page.has_more,
        "tailSequence": page.tail_sequence,
        "tailCursor": page.tail_cursor,
        "rawPayloadPersisted": false,
    }))
}

/// 运行态存储句柄缓存：同一个运行态数据目录在进程内只建一次。
///
/// 句柄内部是进程内共享的连接池，克隆代价极低（见 `SqliteRuntimeStore` 的注释）；
/// 此前每个请求都调一次 `from_root`，等于每个请求都重新打开一次 SQLite 并重建
/// 空连接池，既放大延迟又让等锁上限（5 秒）远大于网关探活预算。
static RUNTIME_STORES: LazyLock<Mutex<HashMap<PathBuf, SqliteRuntimeStore>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 运行态存储句柄；内部为进程内共享连接池，克隆代价极低。
pub(crate) fn runtime_store(axon_root: &Path) -> SqliteRuntimeStore {
    let root = axon_root.join(AXON_RUNTIME_DATA_DIR);
    let mut stores = RUNTIME_STORES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    stores
        .entry(root.clone())
        .or_insert_with(|| SqliteRuntimeStore::from_root(&root))
        .clone()
}

/// 工作区载荷。
pub fn workspaces_payload(axon_root: &Path) -> Result<serde_json::Value, String> {
    let records = runtime_store(axon_root)
        .list_workspaces()
        .map_err(|error| format!("读取工作区注册表失败：{error:?}"))?;
    let workspaces: Vec<WorkspaceView> = records.iter().map(WorkspaceView::from).collect();
    Ok(serde_json::json!({
        "workspaces": workspaces,
        "totalRecords": workspaces.len(),
        "rawPayloadPersisted": false,
    }))
}

/// 车道载荷；给定 `workspace` 时只返回该工作区下的车道。
pub fn lanes_payload(
    axon_root: &Path,
    workspace_filter: Option<&str>,
) -> Result<serde_json::Value, String> {
    let mut lanes = LaneQueryService::new(runtime_store(axon_root))
        .list_lanes()
        .map_err(|error| format!("读取车道列表失败：{error:?}"))?;
    if let Some(workspace_id) = workspace_filter {
        lanes.retain(|lane| lane.workspace_id == workspace_id);
    }
    let pending_counts = lane_pending_question_counts(axon_root).unwrap_or_default();
    let lanes: Vec<LaneView> = lanes
        .iter()
        .map(|lane| {
            let mut view = LaneView::from(lane);
            view.pending_question_count = pending_counts.get(&lane.lane_id).copied().unwrap_or(0);
            view
        })
        .collect();
    Ok(serde_json::json!({
        "lanes": lanes,
        "totalRecords": lanes.len(),
        "rawPayloadPersisted": false,
    }))
}

/// 单条聚合 SQL 查询各车道待答提问数量（P9 判据）。
fn lane_pending_question_counts(
    axon_root: &Path,
) -> Result<std::collections::HashMap<String, u64>, String> {
    let store = runtime_store(axon_root);
    let conn = rusqlite::Connection::open(store.path()).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT lane_id, COUNT(*) FROM jarvis_user_questions \
             WHERE state IN ('pending', 'suspended') GROUP BY lane_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })
        .map_err(|e| e.to_string())?;
    let mut map = std::collections::HashMap::new();
    for row in rows.flatten() {
        map.insert(row.0, row.1);
    }
    Ok(map)
}

/// 新车道标识派生请求。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaneIdentityRequest {
    workspace_id: String,
    channel: String,
    destination: String,
}

/// 工作区注册请求。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceRegisterRequest {
    path: String,
}

/// 子代理消息请求体。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubagentMessageRequest {
    /// 消息正文。
    content: String,
}

/// 解析入站车道标识派生载荷。
fn derive_lane_identity(body: &[u8]) -> Result<serde_json::Value, String> {
    let request: LaneIdentityRequest =
        serde_json::from_slice(body).map_err(|error| format!("解析车道标识请求失败：{error}"))?;
    if request.workspace_id.trim().is_empty() {
        return Err("workspaceId 不能为空".to_string());
    }
    if request.channel.trim().is_empty() {
        return Err("channel 不能为空".to_string());
    }
    if request.destination.trim().is_empty() {
        return Err("destination 不能为空".to_string());
    }
    let audience = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
    let lane_id = session_lane_id(
        &request.workspace_id,
        &audience,
        &request.channel,
        &request.destination,
    );
    Ok(serde_json::json!({ "laneId": lane_id }))
}

/// 工作台 HTTP 路由适配器。
pub struct WorkbenchHttpAdapter {
    /// Axon 运行时根目录。
    axon_root: PathBuf,
    /// 活跃主 turn 注册表：中断入口据此定位正在运行的那一轮。
    active_primary_turns: ActivePrimaryTurnRegistry,
    /// 答案入口应用服务。
    answer_service: Option<crate::user_question::UserQuestionAnswerService>,
}

impl WorkbenchHttpAdapter {
    /// 以运行时根目录构造。
    #[must_use]
    pub fn from_root(axon_root: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
            active_primary_turns: ActivePrimaryTurnRegistry::default(),
            answer_service: None,
        }
    }

    /// 注入活跃主 turn 注册表，使中断入口能触达正在运行的一轮。
    #[must_use]
    pub fn with_active_primary_turns(
        mut self,
        active_primary_turns: ActivePrimaryTurnRegistry,
    ) -> Self {
        self.active_primary_turns = active_primary_turns;
        self
    }

    /// 注入答案入口应用服务。
    #[allow(dead_code)]
    #[must_use]
    pub fn with_answer_service(
        mut self,
        answer_service: crate::user_question::UserQuestionAnswerService,
    ) -> Self {
        self.answer_service = Some(answer_service);
        self
    }

    fn handle_question_answer(
        &self,
        lane_id: &str,
        call_id: &str,
        body: &[u8],
    ) -> GatewayHttpResponse {
        let store = Arc::new(runtime_store(&self.axon_root));
        let default_service = crate::user_question::UserQuestionAnswerService::new(
            store,
            None,
            Arc::new(crate::user_question::NoopUserQuestionEventSink),
            Arc::new(crate::user_question::NoopQuestionMessageSteerer),
        );
        let service = self.answer_service.as_ref().unwrap_or(&default_service);
        crate::user_question::http_adapter::handle_submit_answer(service, lane_id, call_id, body)
    }

    /// 中断某条车道上正在运行的主 turn。
    ///
    /// 取消令牌是既有机制：置位后 provider 传输、重试退避与工具执行的所有取消点
    /// 都会立即收口。没有活跃主 turn 时如实回执 `false`，不假装成功。
    fn interrupt_lane(&self, lane_id: &str) -> Result<serde_json::Value, String> {
        let lane_id = lane_id.trim();
        if lane_id.is_empty() {
            return Err("车道标识不能为空".to_string());
        }
        let affinity_key = ActivePrimaryTurnRegistry::affinity_key(lane_id);
        match self.active_primary_turns.interrupt(&affinity_key)? {
            Some(interaction_id) => Ok(serde_json::json!({
                "laneId": lane_id,
                "interrupted": true,
                "interactionId": interaction_id.as_str(),
            })),
            None => Ok(serde_json::json!({
                "laneId": lane_id,
                "interrupted": false,
                "interactionId": serde_json::Value::Null,
            })),
        }
    }

    /// 向某条车道上正在运行的子代理投递一条用户消息。
    ///
    /// 消息走 mailbox，只有目标子代理会看到；主代理上下文不受影响。
    /// 没有活跃主 turn、通道未就绪、目标不存在或目标当前不可接收时一律拒绝，
    /// 不做「排队等以后」的假受理。
    fn send_subagent_message(
        &self,
        lane_id: &str,
        agent: &str,
        body: &[u8],
    ) -> Result<serde_json::Value, String> {
        let lane_id = lane_id.trim();
        if lane_id.is_empty() {
            return Err("车道标识不能为空".to_string());
        }
        let request: SubagentMessageRequest = serde_json::from_slice(body)
            .map_err(|error| format!("解析子代理消息请求失败：{error}"))?;
        let content = request.content.trim();
        if content.is_empty() {
            return Err("消息内容不能为空".to_string());
        }
        if content.chars().count() > MAX_SUBAGENT_MESSAGE_CHARS {
            return Err(format!(
                "消息超过 {MAX_SUBAGENT_MESSAGE_CHARS} 个字符，请拆分后再发送"
            ));
        }
        // 桌面端把子代理编成 `subagent:<agent path>`；两种写法都接受，
        // 合法性由 AgentPath 自己裁决。
        let agent = agent
            .trim()
            .strip_prefix("subagent:")
            .unwrap_or(agent.trim())
            .to_owned();
        let affinity_key = ActivePrimaryTurnRegistry::affinity_key(lane_id);
        match self
            .active_primary_turns
            .try_send_subagent(&affinity_key, &agent, content)
        {
            SubagentSendOutcome::NoActiveTurn => {
                Err("这条车道上没有正在运行的会话，无法投递".to_string())
            }
            SubagentSendOutcome::Rejected(reason) => Err(reason),
            SubagentSendOutcome::Delivered {
                status,
                wake_required,
            } => {
                if wake_required {
                    return Err("该子代理当前不在运行，无法即时接收消息".to_string());
                }
                Ok(serde_json::json!({
                    "laneId": lane_id,
                    "agentId": agent,
                    "delivered": true,
                    "recipientStatus": status,
                }))
            }
        }
    }
}

impl GatewayHttpRouteHandler for WorkbenchHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let subpath = path.strip_prefix(WORKBENCH_PATH_PREFIX)?;
        let subpath = subpath.strip_prefix('/').unwrap_or(subpath);
        if method == "POST" && subpath == "lanes/identity" {
            return Some(match derive_lane_identity(body) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(400, &serde_json::json!({ "error": error }))
                }
            });
        }
        if method == "POST" && subpath == "workspaces" {
            return Some(match register_workspace(&self.axon_root, body) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(400, &serde_json::json!({ "error": error }))
                }
            });
        }
        if method == "POST"
            && let Some(lane_id) = lane_interrupt_target(subpath)
        {
            return Some(match self.interrupt_lane(&lane_id) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(500, &serde_json::json!({ "error": error }))
                }
            });
        }
        if method == "POST"
            && let Some((lane_id, agent)) = lane_subagent_message_target(subpath)
        {
            return Some(match self.send_subagent_message(&lane_id, &agent, body) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(409, &serde_json::json!({ "error": error }))
                }
            });
        }
        if method == "POST"
            && let Some((lane_id, job_id)) = lane_job_stop_target(subpath)
        {
            return Some(match stop_lane_job(&self.axon_root, &lane_id, &job_id) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(409, &serde_json::json!({ "error": error }))
                }
            });
        }
        if method == "POST"
            && let Some((lane_id, call_id)) = lane_question_answer_target(subpath)
        {
            return Some(self.handle_question_answer(&lane_id, &call_id, body));
        }
        if method != "GET" {
            return None;
        }
        match subpath {
            "workspaces" => Some(match workspaces_payload(&self.axon_root) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(500, &serde_json::json!({ "error": error }))
                }
            }),
            "lanes" => {
                let filter = query_param(query, "workspace");
                Some(match lanes_payload(&self.axon_root, filter.as_deref()) {
                    Ok(payload) => GatewayHttpResponse::json(200, &payload),
                    Err(error) => {
                        GatewayHttpResponse::json(500, &serde_json::json!({ "error": error }))
                    }
                })
            }
            _ => lane_subagents_target(subpath)
                .map(
                    |lane_id| match lane_subagents_payload(&self.axon_root, &lane_id) {
                        Ok(payload) => GatewayHttpResponse::json(200, &payload),
                        Err(error) => {
                            GatewayHttpResponse::json(500, &serde_json::json!({ "error": error }))
                        }
                    },
                )
                .or_else(|| {
                    lane_jobs_target(subpath).map(|lane_id| {
                        match lane_jobs_payload(&self.axon_root, &lane_id) {
                            Ok(payload) => GatewayHttpResponse::json(200, &payload),
                            Err(error) => GatewayHttpResponse::json(
                                500,
                                &serde_json::json!({ "error": error }),
                            ),
                        }
                    })
                })
                .or_else(|| {
                    lane_questions_target(subpath).map(|lane_id| {
                        crate::user_question::http_adapter::handle_list_questions(
                            &runtime_store(&self.axon_root),
                            &lane_id,
                            query,
                        )
                    })
                })
                .or_else(|| {
                    lane_interactions_target(subpath, query).map(|(lane_id, limit, before)| {
                        match lane_interactions_payload(
                            &self.axon_root,
                            &lane_id,
                            limit,
                            before.as_ref(),
                        ) {
                            Ok(payload) => GatewayHttpResponse::json(200, &payload),
                            Err(error) => GatewayHttpResponse::json(
                                500,
                                &serde_json::json!({ "error": error }),
                            ),
                        }
                    })
                })
                .or_else(|| {
                    interaction_events_target(subpath, query).map(
                        |(interaction_id, after, limit)| match interaction_events_payload(
                            &self.axon_root,
                            &interaction_id,
                            after,
                            limit,
                        ) {
                            Ok(payload) => GatewayHttpResponse::json(200, &payload),
                            Err(error) => GatewayHttpResponse::json(
                                404,
                                &serde_json::json!({ "error": error }),
                            ),
                        },
                    )
                }),
        }
    }
}

/// 扩展路由组合器：按注册顺序询问各适配器，首个命中的结果胜出。
///
/// 网关只接受一个扩展处理器，因此各业务适配器通过这里组合，
/// 新增路由族时只需再注册一个适配器，不必改动网关本身。
pub struct CompositeHttpAdapter {
    handlers: Vec<Arc<dyn GatewayHttpRouteHandler>>,
}

impl CompositeHttpAdapter {
    /// 以固定顺序构造组合器。
    #[must_use]
    pub fn new(handlers: Vec<Arc<dyn GatewayHttpRouteHandler>>) -> Self {
        Self { handlers }
    }
}

impl GatewayHttpRouteHandler for CompositeHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        self.handlers
            .iter()
            .find_map(|handler| handler.handle_request(method, path, query, body))
    }
}

/// 注册（或刷新）一个工作区根目录，并返回其权威身份。
///
/// 车道身份是 `(workspace, trust scope, channel, destination)` 的复合哈希，
/// 因此工作区标识必须来自注册表：客户端若拿着一个目录字符串当标识用，
/// 派生出的车道既无法回溯归属，也无法通过提交侧的注册校验。
/// 注册本身是幂等的，重复调用只是刷新最近访问时间。
fn register_workspace(axon_root: &Path, body: &[u8]) -> Result<serde_json::Value, String> {
    let request: WorkspaceRegisterRequest =
        serde_json::from_slice(body).map_err(|error| format!("请求体不是合法 JSON：{error}"))?;
    let path = request.path.trim();
    if path.is_empty() {
        return Err("path 不能为空".to_string());
    }
    let record = axon_session::WorkspaceResolver
        .resolve(Path::new(path), system_now_ms())
        .map_err(|error| format!("解析工作区失败：{error}"))?;
    let store = runtime_store(axon_root);
    store
        .register_workspace(&record)
        .map_err(|error| format!("注册工作区失败：{error:?}"))?;
    Ok(serde_json::json!({ "workspace": WorkspaceView::from(&record) }))
}

fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
