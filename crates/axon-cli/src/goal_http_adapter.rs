//! 目标看板只读 HTTP 适配器。
//!
//! 只读：把 Work Ledger 的 Goal、Execution 与 Result 事实投影成看板载荷，
//! 不写账本、不改数据库 schema、不新增索引。桌面端因此不必直连运行态数据库，
//! 一律经这一面读取。
//!
//! 可见范围由服务端决定，不接受客户端传入任意作用域：固定为全局作用域加
//! Workspace 注册表里的全部工作区。

use std::collections::HashMap;
use std::path::PathBuf;

use axon_daemon::GatewayHttpResponse;
use axon_daemon::GatewayHttpRouteHandler;
use axon_daemon::infrastructure::goal_trace::{GOAL_LIST_FIELD, GoalTrace};
use axon_domain::{GoalDetailQuery, GoalId, GoalListQuery, GoalQueryScope, ScopeRef};
use axon_session::{
    ExecutionStore, GoalPage, GoalQueryService, ResultStore, WorkspaceRecord, WorkspaceStore,
};
use serde_json::json;

use crate::workbench_http_adapter::{WORKBENCH_PATH_PREFIX, runtime_store};

#[path = "goal_http_views.rs"]
mod goal_http_views;

use goal_http_views::{DETAIL_EXECUTION_LIMIT, GoalCardView, goal_card_view, goal_detail_view};

/// 目标路由在 `/api/v1` 下的子路径。
const GOALS_SUBPATH: &str = "goals";
/// 列表默认条数，同时是硬上限。
const LIST_LIMIT: u32 = 256;
/// 列表读取错误码。
const INVALID_QUERY: &str = "invalid_query";
/// 目标不存在错误码。
const GOAL_NOT_FOUND: &str = "goal_not_found";
/// 账本读取失败错误码。
const QUERY_FAILED: &str = "goal_query_failed";
/// `limit` 越界文案。
const LIMIT_MESSAGE: &str = "limit 必须在 1 到 256 之间";
/// 目标不存在文案。
const NOT_FOUND_MESSAGE: &str = "目标不存在或已被清理";
/// 工作区标识非法文案。
const WORKSPACE_MESSAGE: &str = "workspaceId 不是合法的 Workspace 标识";

/// 目标看板 HTTP 适配器。
pub(crate) struct GoalHttpAdapter {
    axon_root: PathBuf,
}

impl GoalHttpAdapter {
    /// 以 Axon 运行时根目录构造。
    pub(crate) fn new(axon_root: PathBuf) -> Self {
        Self { axon_root }
    }

    /// 可见作用域：全局加全部已注册工作区，或客户端指定的单个工作区。
    fn visible_scopes(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<(Vec<ScopeRef>, Vec<WorkspaceRecord>), GatewayHttpResponse> {
        let workspaces = runtime_store(&self.axon_root)
            .list_workspaces()
            .map_err(|error| query_failed(&format!("读取工作区注册表失败：{error}")))?;
        if let Some(requested) = workspace_id {
            let scope = ScopeRef::workspace(requested.to_owned()).map_err(|_| {
                GatewayHttpResponse::error(400, INVALID_QUERY, WORKSPACE_MESSAGE, None)
            })?;
            let kept: Vec<WorkspaceRecord> = workspaces
                .into_iter()
                .filter(|record| record.workspace_id == requested)
                .collect();
            return Ok((vec![scope], kept));
        }
        let mut scopes = Vec::with_capacity(workspaces.len().saturating_add(1));
        scopes.push(ScopeRef::global());
        scopes.extend(
            workspaces
                .iter()
                .map(|record| ScopeRef::workspace(record.workspace_id.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| query_failed("工作区注册表里存在非法的 Workspace 标识"))?,
        );
        Ok((scopes, workspaces))
    }

    /// 列表载荷。
    fn list(&self, query: Option<&str>) -> GatewayHttpResponse {
        let trace = GoalTrace::start(GOAL_LIST_FIELD);
        let response = self.list_inner(query);
        if let Some(trace) = trace {
            trace.finish(response.status.to_string().as_str());
        }
        response
    }

    fn list_inner(&self, query: Option<&str>) -> GatewayHttpResponse {
        let limit = match parse_limit(query) {
            Ok(limit) => limit,
            Err(message) => return GatewayHttpResponse::error(400, INVALID_QUERY, &message, None),
        };
        let workspace_filter = query_value(query, "workspaceId")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let (scopes, workspaces) = match self.visible_scopes(workspace_filter.as_deref()) {
            Ok(visible) => visible,
            Err(response) => return response,
        };
        let store = runtime_store(&self.axon_root);
        // 一次取满硬上限：仓储内部按 `limit + 1` 探测是否还有下一页，因此单页读取
        // 与「翻页到 next_cursor 为空或达到 256 条」的收敛结果逐条相同，且只有一次查询。
        let page = match GoalQueryService::new(store.clone()).list(
            &scopes,
            &GoalListQuery {
                scope: GoalQueryScope::AllVisible,
                statuses: Vec::new(),
                limit: LIST_LIMIT,
                cursor: None,
            },
        ) {
            Ok(page) => page,
            Err(error) => return query_failed(&error.to_string()),
        };
        let cards = match self.collect_cards(&page, &workspaces) {
            Ok(cards) => cards,
            Err(error) => return query_failed(&error),
        };
        let available = cards.len();
        // `truncated` 表示「这次读取达到上限且仍有更多」：仓储给出下一页，
        // 或可见目标多于调用方要的条数，两者任一成立都为真。
        let truncated = page.next_cursor.is_some() || available > limit as usize;
        let goals: Vec<GoalCardView> = cards.into_iter().take(limit as usize).collect();
        GatewayHttpResponse::json(
            200,
            &json!({
                "goals": goals,
                "returned": goals.len(),
                "truncated": truncated,
            }),
        )
    }

    /// 为一页目标补最近执行、最近结果与工作区，并按最近活动降序排序。
    fn collect_cards(
        &self,
        page: &GoalPage,
        workspaces: &[WorkspaceRecord],
    ) -> Result<Vec<GoalCardView>, String> {
        let store = runtime_store(&self.axon_root);
        let by_id: HashMap<&str, &WorkspaceRecord> = workspaces
            .iter()
            .map(|record| (record.workspace_id.as_str(), record))
            .collect();
        let mut cards = Vec::with_capacity(page.goals.len());
        for goal in &page.goals {
            let latest_execution = store
                .list_executions_for_goal(&goal.goal_id, 1)
                .map_err(|error| format!("读取目标执行失败：{error}"))?
                .into_iter()
                .next();
            let latest_result = latest_execution
                .as_ref()
                .map(|execution| {
                    store
                        .result_for_execution(&execution.execution_id)
                        .map_err(|error| format!("读取执行结果失败：{error}"))
                })
                .transpose()?
                .flatten();
            let workspace = goal
                .scope
                .workspace_id()
                .and_then(|id| by_id.get(id.as_str()).copied());
            cards.push(goal_card_view(
                goal,
                latest_execution.as_ref(),
                latest_result.as_ref(),
                workspace,
            ));
        }
        cards.sort_by(|left, right| {
            right
                .last_activity_at_ms
                .cmp(&left.last_activity_at_ms)
                .then_with(|| right.goal_id.cmp(&left.goal_id))
        });
        Ok(cards)
    }

    /// 详情载荷。
    fn detail(&self, goal_id: &str) -> GatewayHttpResponse {
        let goal_id = match GoalId::new(goal_id.to_owned()) {
            Ok(goal_id) => goal_id,
            Err(_) => {
                return GatewayHttpResponse::error(404, GOAL_NOT_FOUND, NOT_FOUND_MESSAGE, None);
            }
        };
        let (scopes, workspaces) = match self.visible_scopes(None) {
            Ok(visible) => visible,
            Err(response) => return response,
        };
        let store = runtime_store(&self.axon_root);
        let view = match GoalQueryService::new(store.clone()).get(
            &scopes,
            &GoalDetailQuery {
                goal_id: goal_id.clone(),
                include_executions: true,
                include_events: false,
                limit: DETAIL_EXECUTION_LIMIT,
            },
        ) {
            Ok(Some(view)) => view,
            Ok(None) => {
                return GatewayHttpResponse::error(404, GOAL_NOT_FOUND, NOT_FOUND_MESSAGE, None);
            }
            Err(error) => return query_failed(&error.to_string()),
        };
        let workspace = view.goal.scope.workspace_id().and_then(|id| {
            workspaces
                .iter()
                .find(|record| record.workspace_id == id.as_str())
        });
        let card = goal_card_view(
            &view.goal,
            view.latest_execution.as_ref(),
            view.latest_result.as_ref(),
            workspace,
        );
        let detail = goal_detail_view(
            card,
            &view.goal,
            &view.recent_executions,
            view.latest_result.as_ref(),
        );
        GatewayHttpResponse::json(200, &detail)
    }
}

impl GatewayHttpRouteHandler for GoalHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        _body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        if method != "GET" {
            return None;
        }
        let subpath = path
            .strip_prefix(WORKBENCH_PATH_PREFIX)?
            .strip_prefix('/')?;
        let rest = subpath.strip_prefix(GOALS_SUBPATH)?;
        if rest.is_empty() {
            return Some(self.list(query));
        }
        // 只接受一层目标标识：更深的路径不属于本路由族，交回组合器。
        let goal_id = rest.strip_prefix('/')?;
        if goal_id.is_empty() || goal_id.contains('/') {
            return None;
        }
        Some(self.detail(goal_id))
    }
}

/// 解析 `limit`；缺省取硬上限，越界或非数字一律拒绝，不做静默夹取。
fn parse_limit(query: Option<&str>) -> Result<u32, String> {
    match query_value(query, "limit") {
        None => Ok(LIST_LIMIT),
        Some(raw) => match raw.trim().parse::<u32>() {
            Ok(value) if (1..=LIST_LIMIT).contains(&value) => Ok(value),
            _ => Err(LIMIT_MESSAGE.to_owned()),
        },
    }
}

/// 读取单个查询参数；空值与缺省同义。
fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name != key {
            return None;
        }
        Some(percent_decode(value))
    })
}

/// 最小百分号解码：`%XX` 还原为字节，非法序列原样保留。
///
/// 只服务查询串里的工作区标识：网关不提供解码器，而客户端可能带上编码过的标识。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// 单个十六进制字符的取值。
const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 账本读取失败响应。
fn query_failed(message: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::error(500, QUERY_FAILED, message, None)
}

#[cfg(test)]
#[path = "goal_http_adapter_tests.rs"]
mod goal_http_adapter_tests;

#[cfg(test)]
#[path = "goal_http_perf_tests.rs"]
mod goal_http_perf_tests;
