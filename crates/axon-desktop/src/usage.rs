//! 用量统计：桌面端读取可观测性接口的薄封装。
//!
//! 与 Cron、记忆一致，数据一律经网关 `/api/v1/observability/*` 取得：
//! 桌面端既不直连 `usage/usage.sqlite3`，也不在这里重复实现聚合与计费口径，
//! 网关返回什么事实就展示什么事实。
//!
//! 查询参数只做白名单内的透传，缺省项整项省略，让网关使用它自己的默认窗口，
//! 避免桌面端与网关各存一份默认值。

use serde_json::Value;

use crate::gateway;

/// 可观测性扩展面路径前缀（网关内部已带 `/api/v1`）。
const OBSERVABILITY: &str = "/observability";

/// 事件日志的单页条数上限，与网关 `limit` 的校验边界一致。
const MAX_EVENT_PAGE_SIZE: usize = 500;
/// 事件日志的单页条数默认值。
const DEFAULT_EVENT_PAGE_SIZE: usize = 20;

/// 拼装查询串；空值整项省略，因此不会把空串当成有效过滤条件发给网关。
fn query(params: &[(&str, Option<String>)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        if let Some(value) = value
            && !value.trim().is_empty()
        {
            serializer.append_pair(key, value);
        }
    }
    serializer.finish()
}

/// 拼装可观测性接口地址（不含网关的 `/api/v1` 前缀）。
pub(crate) fn observability_endpoint(path: &str, params: &[(&str, Option<String>)]) -> String {
    let query = query(params);
    if query.is_empty() {
        format!("{OBSERVABILITY}{path}")
    } else {
        format!("{OBSERVABILITY}{path}?{query}")
    }
}

/// 拼装可观测性接口地址；模块内沿用短名。
fn endpoint(path: &str, params: &[(&str, Option<String>)]) -> String {
    observability_endpoint(path, params)
}

/// 大盘概览：汇总指标、迷你走势、活动热力图与健康时间线。
#[tauri::command]
pub fn usage_overview(
    period: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    source: Option<String>,
) -> Result<Value, String> {
    gateway::get_json(&endpoint(
        "/overview",
        &[
            ("period", period),
            ("provider", provider),
            ("model", model),
            ("source", source),
        ],
    ))
}

/// 用量分布与排行：按模型或供应商维度，附时间序列与占比。
#[tauri::command]
pub fn usage_distribution(
    dimension: Option<String>,
    period: Option<String>,
) -> Result<Value, String> {
    gateway::get_json(&endpoint(
        "/distribution",
        &[("dimension", dimension), ("period", period)],
    ))
}

/// 延迟与效率诊断：分位数、散点样本与模型单价效益。
#[tauri::command]
pub fn usage_diagnostics(period: Option<String>) -> Result<Value, String> {
    gateway::get_json(&endpoint("/diagnostics", &[("period", period)]))
}

/// 实时滑窗读数：进程内滑动窗口的每分钟吞吐与请求结果。
#[tauri::command]
pub fn usage_realtime(window: Option<String>) -> Result<Value, String> {
    gateway::get_json(&endpoint("/realtime", &[("window", window)]))
}

/// 请求事件日志：按模型、来源、状态过滤，游标分页。
#[tauri::command]
pub fn usage_events(
    model: Option<String>,
    source: Option<String>,
    status: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
) -> Result<Value, String> {
    let limit = limit
        .unwrap_or(DEFAULT_EVENT_PAGE_SIZE)
        .clamp(1, MAX_EVENT_PAGE_SIZE)
        .to_string();
    gateway::get_json(&endpoint(
        "/audit/logs",
        &[
            ("model", model),
            ("source", source),
            ("status", status),
            ("cursor", cursor),
            ("limit", Some(limit)),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_omits_empty_parameters() {
        assert_eq!(
            endpoint(
                "/overview",
                &[("period", Some("7d".to_owned())), ("provider", None)]
            ),
            "/observability/overview?period=7d"
        );
        assert_eq!(endpoint("/overview", &[]), "/observability/overview");
        assert_eq!(
            endpoint("/overview", &[("model", Some("   ".to_owned()))]),
            "/observability/overview"
        );
    }

    #[test]
    fn endpoint_escapes_parameter_values() {
        assert_eq!(
            endpoint("/overview", &[("model", Some("gpt 5+a/b".to_owned()))]),
            "/observability/overview?model=gpt+5%2Ba%2Fb"
        );
    }
}
