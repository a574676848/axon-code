//! 目标看板：只读目标列表与详情的 IPC 命令。
//!
//! 只做参数校验、拼装与网关转发：目标、执行与会话事实一律经网关读取，
//! 桌面端不直连运行态数据库，也不缓存第二份事实。

use serde_json::Value;

use crate::gateway;

/// 列表条数上限，与网关契约一致；桌面端固定请求这个数。
const MAX_LIST_LIMIT: u32 = 256;
/// 列表条数下限。
const MIN_LIST_LIMIT: u32 = 1;

/// 列出可见目标。
///
/// `limit` 在这里夹取到 1 至 256，越界不报错也不放行：网关会拒绝越界值，
/// 而界面只有「看板固定取 256」这一种用法，夹取后请求总能成立。
#[tauri::command]
pub fn goal_list(limit: Option<u32>, workspace_id: Option<String>) -> Result<Value, String> {
    let limit = limit
        .unwrap_or(MAX_LIST_LIMIT)
        .clamp(MIN_LIST_LIMIT, MAX_LIST_LIMIT);
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("limit", &limit.to_string());
    if let Some(workspace_id) = workspace_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.append_pair("workspaceId", workspace_id);
    }
    get(&format!("/goals?{}", query.finish()))
}

/// 读取单个目标的详情。
///
/// 标识去掉首尾空白后必须非空；含路径分隔符、查询串或片段的标识直接拒绝，
/// 不发请求，也不让一个畸形的标识把路由拼到别的资源上。
#[tauri::command]
pub fn goal_get(goal_id: String) -> Result<Value, String> {
    let goal_id = goal_id.trim();
    if goal_id.is_empty() {
        return Err("目标标识不能为空".to_string());
    }
    if goal_id.contains(['/', '?', '#']) {
        return Err("目标标识不合法".to_string());
    }
    get(&format!("/goals/{goal_id}"))
}

/// GET 一个网关接口，并把非 2xx 翻译成可执行的中文说明。
///
/// 不走 `get_json`：那条路径对非 2xx 只报状态码，而详情 404 必须落到
/// 「目标不存在或已被清理」与「网关不含该接口」两种截然不同的结论上。
fn get(path: &str) -> Result<Value, String> {
    let (status, body) = gateway::get_raw(path)?;
    if !(200..300).contains(&status) {
        return Err(gateway::gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

#[cfg(test)]
mod tests {
    use super::goal_get;

    #[test]
    fn goal_identifiers_are_checked_before_any_request() {
        // 空标识与带路径分隔符的标识都在本地被拒，不会拼出一个越界路径。
        assert!(goal_get("   ".to_owned()).is_err());
        assert!(goal_get("goal-1/../../secret".to_owned()).is_err());
        assert!(goal_get("goal-1?limit=1".to_owned()).is_err());
        assert!(goal_get("goal-1#frag".to_owned()).is_err());
        // 合法标识通过校验，随后由网关可达性决定成败；本机没有网关时只要求
        // 错误不是参数校验那两条。
        let error = goal_get("goal-01234567-89ab-cdef-0123-456789abcdef".to_owned())
            .expect_err("没有运行中的网关时不应返回数据");
        assert!(!error.contains("目标标识"));
    }
}
