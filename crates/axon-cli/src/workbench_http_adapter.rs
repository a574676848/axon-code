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

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_domain::DaemonSessionId;
use axon_session::{
    AXON_RUNTIME_DATA_DIR, InteractionCursor, RuntimeStore, RuntimeStoreError, SqliteRuntimeStore,
    WorkspaceRecord, WorkspaceStore,
};
use serde::Serialize;

use crate::run_commands::run_steer::{ActivePrimaryTurnRegistry, SubagentSendOutcome};

/// daemon 网关上的扩展路由前缀。
pub const WORKBENCH_PATH_PREFIX: &str = "/api/v1";
/// 协议版本；新增路由或改动字段语义时必须递增，供客户端协商。
pub const WORKBENCH_PROTOCOL_VERSION: u32 = 8;
/// 单次车道 Interaction 回放查询的默认条数上限。
const DEFAULT_SESSION_INTERACTION_LIMIT: u32 = 200;
/// 单次车道 Interaction 回放查询的条数硬上限。
const MAX_SESSION_INTERACTION_LIMIT: u32 = 1_000;
/// 历史帧页的默认帧数：一次取够一屏正文，又不至于让单次响应过大。
const DEFAULT_INTERACTION_EVENTS_LIMIT: u32 = 800;
/// 单页帧数硬上限。
pub(super) const MAX_INTERACTION_EVENTS_LIMIT: u32 = 2_000;
/// 单条发往子代理的用户消息字符上限；与主会话输入保持同一量级。
const MAX_SUBAGENT_MESSAGE_CHARS: usize = 8_000;

#[path = "workbench_http_views.rs"]
mod workbench_http_views;

pub use workbench_http_views::{InteractionView, TurnMetricsView, WorkspaceView};

#[path = "workbench_http_targets.rs"]
mod workbench_http_targets;

#[cfg(test)]
pub(crate) use workbench_http_targets::owner_matches_session;
use workbench_http_targets::{
    interaction_events_target, query_param, session_interactions_target, session_interrupt_target,
    session_job_output_target, session_job_stop_target, session_jobs_target,
    session_question_answer_target, session_questions_target, session_subagent_message_target,
    session_subagents_target,
};

#[path = "workbench_http_subagents_jobs.rs"]
mod workbench_http_subagents_jobs;

#[path = "workbench_http_session_readings.rs"]
mod session_readings;

#[path = "workbench_http_session_interactions.rs"]
mod session_interactions;

pub use session_interactions::session_interactions_payload;

#[path = "workbench_http_interaction_events.rs"]
mod interaction_events;

use interaction_events::interaction_events_payload;
use workbench_http_subagents_jobs::{
    JOB_OUTPUT_DEFAULT_MAX_BYTES, session_job_output_payload, session_jobs_payload,
    session_subagents_payload, stop_session_job,
};

#[cfg(test)]
#[path = "workbench_http_adapter_tests.rs"]
mod workbench_http_adapter_tests;

/// 车道内 Interaction 列表载荷的组装在 [`session_interactions`] 里：它与只读列表的
/// 逐轮读数、最终正文共用同一条「不回放正文」的取数口径，独立成文后本文件只保留路由。
///
/// 运行态存储句柄缓存：同一个运行态数据目录在进程内只建一次。
///
/// 句柄内部是进程内共享的连接池，克隆代价极低（见 `SqliteRuntimeStore` 的注释）；
/// 此前每个请求都调一次 `from_root`，等于每个请求都重新打开一次 SQLite 并重建
/// 空连接池，既放大延迟又让等锁上限（5 秒）远大于网关探活预算。
static RUNTIME_STORES: LazyLock<Mutex<HashMap<PathBuf, SqliteRuntimeStore>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 运行态存储句柄；内部为进程内共享连接池，克隆代价极低。
pub(crate) fn runtime_store(axon_root: &Path) -> SqliteRuntimeStore {
    runtime_store_for_data_dir(&axon_root.join(AXON_RUNTIME_DATA_DIR))
}

/// 已解析到运行态数据目录的句柄缓存入口：适配器持的就是数据目录。
pub(crate) fn runtime_store_for_data_dir(data_dir: &Path) -> SqliteRuntimeStore {
    let root = data_dir.to_path_buf();
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
///
/// 字段直接取 `read_session_rows`：桌面左侧菜单与 CLI `sessions list` 读同一份投影，
/// 标题、置顶、归档、最近活动与会话累计读数因此不会再出现两套口径。
pub fn sessions_payload(
    axon_root: &Path,
    workspace_filter: Option<&str>,
    include_archived: bool,
) -> Result<serde_json::Value, String> {
    let rows = super::sessions_http_adapter::read_session_rows(&runtime_store(axon_root))
        .map_err(|error| format!("读取会话列表失败：{error:?}"))?;
    let sessions: Vec<serde_json::Value> = rows
        .iter()
        .filter(|row| {
            workspace_filter.is_none_or(|workspace| row.workspace_id.as_deref() == Some(workspace))
        })
        .filter(|row| include_archived || !row.archived)
        .map(super::sessions_http_adapter::SessionListRow::to_json)
        .collect();
    Ok(serde_json::json!({
        "sessions": sessions,
        "totalRecords": sessions.len(),
        "rawPayloadPersisted": false,
    }))
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
        session_id: &str,
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
        crate::user_question::http_adapter::handle_submit_answer(service, session_id, call_id, body)
    }

    /// 中断某条会话上正在运行的轮次。
    ///
    /// 两条链路缺一不可：主 turn 在进程内注册了取消令牌，置位即收口；独立会话（目标执行、
    /// cron）没有进程内句柄，只能写 durable 取消标记，由运行侧的看门狗在有界周期内消费。
    /// 两条都打不到时如实回执 `interrupted=false`，不假装成功。
    fn interrupt_session(&self, session_id: &str) -> Result<serde_json::Value, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("会话标识不能为空".to_string());
        }
        let affinity_key = ActivePrimaryTurnRegistry::affinity_key(session_id);
        if let Some(interaction_id) = self.active_primary_turns.interrupt(&affinity_key)? {
            return Ok(serde_json::json!({
                "sessionId": session_id,
                "interrupted": true,
                "interactionId": interaction_id.as_str(),
            }));
        }
        // 没有主 turn 句柄：落到 durable 标记，覆盖 worker 驱动的目标执行与 cron 轮次。
        let durable = axon_domain::DaemonSessionId::new(session_id.to_owned())
            .map_err(|error| format!("会话标识非法：{error}"))?;
        let marked = axon_session::RuntimeStore::request_session_cancellation(
            &runtime_store(&self.axon_root),
            &durable,
            "用户从前端中断",
            axon_time::unix_now_ms(),
        )
        .map_err(|error| format!("请求取消失败：{error}"))?;
        Ok(serde_json::json!({
            "sessionId": session_id,
            "interrupted": marked > 0,
            "interactionId": serde_json::Value::Null,
        }))
    }

    /// 向某条车道上正在运行的子代理投递一条用户消息。
    ///
    /// 消息走 mailbox，只有目标子代理会看到；主代理上下文不受影响。
    /// 没有活跃主 turn、通道未就绪、目标不存在或目标当前不可接收时一律拒绝，
    /// 不做「排队等以后」的假受理。
    fn send_subagent_message(
        &self,
        session_id: &str,
        agent: &str,
        body: &[u8],
    ) -> Result<serde_json::Value, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
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
        let affinity_key = ActivePrimaryTurnRegistry::affinity_key(session_id);
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
                    "sessionId": session_id,
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
        if method == "POST" && subpath == "workspaces" {
            return Some(match register_workspace(&self.axon_root, body) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(400, &serde_json::json!({ "error": error }))
                }
            });
        }
        if method == "POST"
            && let Some(session_id) = session_interrupt_target(subpath)
        {
            return Some(match self.interrupt_session(&session_id) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => {
                    GatewayHttpResponse::json(500, &serde_json::json!({ "error": error }))
                }
            });
        }
        if method == "POST"
            && let Some((session_id, agent)) = session_subagent_message_target(subpath)
        {
            return Some(
                match self.send_subagent_message(&session_id, &agent, body) {
                    Ok(payload) => GatewayHttpResponse::json(200, &payload),
                    Err(error) => {
                        GatewayHttpResponse::json(409, &serde_json::json!({ "error": error }))
                    }
                },
            );
        }
        if method == "POST"
            && let Some((session_id, job_id)) = session_job_stop_target(subpath)
        {
            return Some(
                match stop_session_job(&self.axon_root, &session_id, &job_id) {
                    Ok(payload) => GatewayHttpResponse::json(200, &payload),
                    Err(error) => {
                        GatewayHttpResponse::json(409, &serde_json::json!({ "error": error }))
                    }
                },
            );
        }
        if method == "POST"
            && let Some((session_id, call_id)) = session_question_answer_target(subpath)
        {
            return Some(self.handle_question_answer(&session_id, &call_id, body));
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
            "sessions" => {
                let filter = query_param(query, "workspace");
                // 归档会话只有显式请求才返回：列表默认只给当前在用的那些。
                let include_archived = query_param(query, "include_archived")
                    .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
                Some(
                    match sessions_payload(&self.axon_root, filter.as_deref(), include_archived) {
                        Ok(payload) => GatewayHttpResponse::json(200, &payload),
                        Err(error) => {
                            GatewayHttpResponse::json(500, &serde_json::json!({ "error": error }))
                        }
                    },
                )
            }
            _ => session_subagents_target(subpath)
                .map(
                    |session_id| match session_subagents_payload(&self.axon_root, &session_id) {
                        Ok(payload) => GatewayHttpResponse::json(200, &payload),
                        Err(error) => {
                            GatewayHttpResponse::json(500, &serde_json::json!({ "error": error }))
                        }
                    },
                )
                .or_else(|| {
                    session_jobs_target(subpath).map(|session_id| {
                        match session_jobs_payload(&self.axon_root, &session_id) {
                            Ok(payload) => GatewayHttpResponse::json(200, &payload),
                            Err(error) => GatewayHttpResponse::json(
                                500,
                                &serde_json::json!({ "error": error }),
                            ),
                        }
                    })
                })
                .or_else(|| {
                    session_job_output_target(subpath).map(|(session_id, job_id)| {
                        let offset = query_param(query, "offset")
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(0);
                        let max_bytes = query_param(query, "max")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(JOB_OUTPUT_DEFAULT_MAX_BYTES);
                        match session_job_output_payload(
                            &self.axon_root,
                            &session_id,
                            &job_id,
                            offset,
                            max_bytes,
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
                    session_questions_target(subpath).map(|session_id| {
                        crate::user_question::http_adapter::handle_list_questions(
                            &runtime_store(&self.axon_root),
                            &session_id,
                            query,
                        )
                    })
                })
                .or_else(|| {
                    session_interactions_target(subpath, query).map(
                        |(session_id, limit, before)| match session_interactions_payload(
                            &self.axon_root,
                            &session_id,
                            limit,
                            before.as_ref(),
                        ) {
                            Ok(payload) => GatewayHttpResponse::json(200, &payload),
                            Err(error) => GatewayHttpResponse::json(
                                500,
                                &serde_json::json!({ "error": error }),
                            ),
                        },
                    )
                })
                .or_else(|| {
                    interaction_events_target(subpath, query).map(
                        |(interaction_id, after, limit, tail)| {
                            interaction_events_payload(
                                &self.axon_root,
                                &interaction_id,
                                after,
                                limit,
                                tail,
                            )
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

    /// 带身份的入口：身份必须逐层透传。
    ///
    /// 网关路由层已经完成鉴权并按路径分级，本层不做第二次判定，只负责把身份送到需要它
    /// 的适配器手上。少了这一层，`/api/v1/keys` 这类按身份分级的路由族拿到的永远是
    /// `None`，管理面会对超管也回 403。未覆写 `handle_request_as` 的适配器由 trait 的
    /// 默认实现回落，行为与改造前逐字一致。
    fn handle_request_as(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
        identity: Option<&axon_daemon::infrastructure::gateway_auth::GatewayIdentity>,
        peer: Option<&str>,
    ) -> Option<GatewayHttpResponse> {
        self.handlers.iter().find_map(|handler| {
            handler.handle_request_as(method, path, query, body, identity, peer)
        })
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
