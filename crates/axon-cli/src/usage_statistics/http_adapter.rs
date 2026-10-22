//! 可观测性 RESTful API HTTP 路由适配器（SSOT §5.1 ~ §5.6 / §2.3 - M6）。
//!
//! 在 HttpGatewayAdapter 中挂载 `/api/v1/observability/*` 完整路由族，
//! 严格复用 GatewayAuth 鉴权机制，统一错误模型。

use std::collections::HashMap;
use std::path::PathBuf;

use axon_daemon::GatewayHttpResponse;
use axon_daemon::GatewayHttpRouteHandler;

use super::observability_model::{
    ObservabilityErrorResponse, RecalculatePricingPayload, SavePricingRulePayload,
    SetPricingAutoSyncPayload,
};
use super::observability_service::{
    delete_pricing_rule_service, export_audit_logs_service, list_pricing_rules_service,
    pricing_auto_sync_state_service, query_audit_logs_service, query_diagnostics_service,
    query_distribution, query_overview, query_realtime, recalculate_pricing_service,
    save_pricing_rule_service, set_pricing_auto_sync_service, sync_pricing_service,
};
use super::repository::audit::AuditLogQuery;

const OBSERVABILITY_PATH_PREFIX: &str = "/api/v1/observability";

fn error_response(status: u16, err: ObservabilityErrorResponse) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &err)
}

/// 可观测性 HTTP 路由分发器。
pub struct ObservabilityHttpAdapter {
    config_path: PathBuf,
}

impl ObservabilityHttpAdapter {
    #[must_use]
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }
}

impl GatewayHttpRouteHandler for ObservabilityHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let subpath = path.strip_prefix(OBSERVABILITY_PATH_PREFIX)?;
        let subpath = subpath.strip_prefix('/').unwrap_or(subpath);
        let params = parse_query(query.unwrap_or_default());

        match (method, subpath) {
            // 1. GET /overview
            ("GET", "overview") => Some(self.handle_overview(&params)),

            // 2. GET /realtime
            ("GET", "realtime") => Some(self.handle_realtime(&params)),

            // 3. GET /distribution
            ("GET", "distribution") => Some(self.handle_distribution(&params)),

            // 4. GET /diagnostics
            ("GET", "diagnostics") => Some(self.handle_diagnostics(&params)),

            // 5. GET /audit/logs
            ("GET", "audit/logs") => Some(self.handle_audit_logs(&params)),

            // 6. GET /audit/export
            ("GET", "audit/export") => Some(self.handle_audit_export(&params)),

            // 7. GET /pricing/rules
            ("GET", "pricing/rules") => Some(self.handle_list_pricing_rules()),

            // 8. POST /pricing/rules
            ("POST", "pricing/rules") => Some(self.handle_save_pricing_rule(body)),

            // 9. DELETE /pricing/rules/{id}
            ("DELETE", sub) if sub.starts_with("pricing/rules/") => {
                let id = sub.strip_prefix("pricing/rules/").unwrap_or("");
                Some(self.handle_delete_pricing_rule(id))
            }

            // 10. POST /pricing/sync
            ("POST", "pricing/sync") => Some(self.handle_sync_pricing()),

            // 11. POST /pricing/recalculate
            ("POST", "pricing/recalculate") => Some(self.handle_recalculate_pricing(body)),

            // 12. GET /pricing/auto-sync：读取每日自动同步任务的状态
            ("GET", "pricing/auto-sync") => Some(self.handle_auto_sync_state()),

            // 13. POST /pricing/auto-sync：创建/启用或暂停每日自动同步任务
            ("POST", "pricing/auto-sync") => Some(self.handle_set_auto_sync(body)),

            // 14. 鉴权探测端点
            ("GET", "auth/probe") => Some(error_response(
                401,
                ObservabilityErrorResponse::unauthorized("未授权访问：缺少或无效的 API Token"),
            )),

            // 其他未匹配子路径返回 404
            _ => Some(error_response(
                404,
                ObservabilityErrorResponse::not_found("指定的可观测性接口不存在"),
            )),
        }
    }
}

impl ObservabilityHttpAdapter {
    fn handle_overview(&self, params: &HashMap<String, String>) -> GatewayHttpResponse {
        let period = params.get("period").map(String::as_str);
        let provider = params.get("provider").map(String::as_str);
        let model = params.get("model").map(String::as_str);
        let source = params.get("source").map(String::as_str);

        match query_overview(&self.config_path, period, provider, model, source) {
            Ok(report) => GatewayHttpResponse::json(200, &report),
            Err(err) => {
                error_response(400, ObservabilityErrorResponse::invalid_argument(err, None))
            }
        }
    }

    fn handle_realtime(&self, params: &HashMap<String, String>) -> GatewayHttpResponse {
        let window = params.get("window").map(String::as_str);
        match query_realtime(&self.config_path, window) {
            Ok(report) => GatewayHttpResponse::json(200, &report),
            Err(err) => {
                error_response(400, ObservabilityErrorResponse::invalid_argument(err, None))
            }
        }
    }

    fn handle_distribution(&self, params: &HashMap<String, String>) -> GatewayHttpResponse {
        let dimension = params.get("dimension").map(String::as_str);
        let period = params.get("period").map(String::as_str);
        match query_distribution(&self.config_path, dimension, period) {
            Ok(report) => GatewayHttpResponse::json(200, &report),
            Err(err) => {
                error_response(400, ObservabilityErrorResponse::invalid_argument(err, None))
            }
        }
    }

    fn handle_diagnostics(&self, params: &HashMap<String, String>) -> GatewayHttpResponse {
        let period = params.get("period").map(String::as_str);
        if let Some(p) = period
            && p != "today"
            && p != "7d"
        {
            let details = serde_json::json!({
                "param": "period",
                "maxAllowed": "7d",
                "provided": p,
            });
            return error_response(
                400,
                ObservabilityErrorResponse::invalid_argument(
                    "诊断接口窗口上限为 7 天 (仅支持 today 或 7d)",
                    Some(details),
                ),
            );
        }
        match query_diagnostics_service(&self.config_path, period) {
            Ok(report) => GatewayHttpResponse::json(200, &report),
            Err(err) => {
                error_response(400, ObservabilityErrorResponse::invalid_argument(err, None))
            }
        }
    }

    fn handle_audit_logs(&self, params: &HashMap<String, String>) -> GatewayHttpResponse {
        let limit = params
            .get("limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50);
        if limit > 500 {
            let details = serde_json::json!({
                "param": "limit",
                "maxAllowed": 500,
                "provided": limit,
            });
            return error_response(
                400,
                ObservabilityErrorResponse::invalid_argument(
                    "limit 参数超出上限 500",
                    Some(details),
                ),
            );
        }
        let from_ms = params.get("from_ms").and_then(|v| v.parse::<u64>().ok());
        let to_ms = params.get("to_ms").and_then(|v| v.parse::<u64>().ok());
        let query = AuditLogQuery {
            model: params.get("model").map(String::as_str),
            source: params.get("source").map(String::as_str),
            status: params.get("status").map(String::as_str),
            from_ms,
            to_ms,
            cursor: params.get("cursor").map(String::as_str),
            limit,
        };

        match query_audit_logs_service(&self.config_path, &query) {
            Ok(report) => GatewayHttpResponse::json(200, &report),
            Err(err) => {
                error_response(400, ObservabilityErrorResponse::invalid_argument(err, None))
            }
        }
    }

    fn handle_audit_export(&self, params: &HashMap<String, String>) -> GatewayHttpResponse {
        let format = params.get("format").map(String::as_str).unwrap_or("csv");
        if format != "csv" && format != "json" {
            let details = serde_json::json!({
                "param": "format",
                "supported": ["csv", "json"],
                "provided": format,
            });
            return error_response(
                400,
                ObservabilityErrorResponse::invalid_argument(
                    "导出格式仅支持 csv 或 json",
                    Some(details),
                ),
            );
        }

        let from_ms = params.get("from_ms").and_then(|v| v.parse::<u64>().ok());
        let to_ms = params.get("to_ms").and_then(|v| v.parse::<u64>().ok());
        let query = AuditLogQuery {
            model: params.get("model").map(String::as_str),
            source: params.get("source").map(String::as_str),
            status: params.get("status").map(String::as_str),
            from_ms,
            to_ms,
            cursor: params.get("cursor").map(String::as_str),
            limit: 500,
        };

        let mut buffer = Vec::new();
        match export_audit_logs_service(&self.config_path, &query, format, &mut buffer) {
            Ok(_) => {
                if format == "csv" {
                    GatewayHttpResponse::csv(buffer)
                } else {
                    GatewayHttpResponse {
                        status: 200,
                        content_type: "application/json".to_owned(),
                        body: buffer,
                    }
                }
            }
            Err(err) => error_response(500, ObservabilityErrorResponse::internal(err)),
        }
    }

    fn handle_list_pricing_rules(&self) -> GatewayHttpResponse {
        match list_pricing_rules_service(&self.config_path) {
            Ok(rules) => GatewayHttpResponse::json(200, &rules),
            Err(err) => error_response(500, ObservabilityErrorResponse::internal(err)),
        }
    }

    fn handle_save_pricing_rule(&self, body: &[u8]) -> GatewayHttpResponse {
        let payload: SavePricingRulePayload = match serde_json::from_slice(body) {
            Ok(p) => p,
            Err(err) => {
                return error_response(
                    400,
                    ObservabilityErrorResponse::invalid_argument(
                        format!("JSON 报文解析失败: {err}"),
                        None,
                    ),
                );
            }
        };

        // 审计日志纪律 (SSOT §2.3)
        let now_ms = axon_time::unix_now_ms();
        eprintln!(
            "[audit] [pricing-rule-save] at_ms={now_ms} pattern={} style={} priority={}",
            payload.model_pattern, payload.pricing_style, payload.priority
        );

        match save_pricing_rule_service(&self.config_path, payload) {
            Ok(id) => {
                let resp = serde_json::json!({ "id": id, "status": "created" });
                GatewayHttpResponse::json(200, &resp)
            }
            Err(err) => {
                error_response(400, ObservabilityErrorResponse::invalid_argument(err, None))
            }
        }
    }

    fn handle_delete_pricing_rule(&self, id: &str) -> GatewayHttpResponse {
        if id.trim().is_empty() {
            return error_response(
                400,
                ObservabilityErrorResponse::invalid_argument("缺少 pricing rule ID", None),
            );
        }

        let now_ms = axon_time::unix_now_ms();
        eprintln!("[audit] [pricing-rule-delete] at_ms={now_ms} id={id}");

        match delete_pricing_rule_service(&self.config_path, id) {
            Ok(true) => {
                let resp = serde_json::json!({ "id": id, "status": "deleted" });
                GatewayHttpResponse::json(200, &resp)
            }
            Ok(false) => error_response(
                404,
                ObservabilityErrorResponse::not_found(format!("未找到指定的 pricing rule: {id}")),
            ),
            Err(err) => error_response(500, ObservabilityErrorResponse::internal(err)),
        }
    }

    fn handle_sync_pricing(&self) -> GatewayHttpResponse {
        let now_ms = axon_time::unix_now_ms();
        eprintln!("[audit] [pricing-sync] at_ms={now_ms} trigger=http");

        match block_on_async(sync_pricing_service(&self.config_path)).and_then(|r| r) {
            Ok(resp) => GatewayHttpResponse::json(200, &resp),
            Err(err) => error_response(500, ObservabilityErrorResponse::internal(err)),
        }
    }

    fn handle_recalculate_pricing(&self, body: &[u8]) -> GatewayHttpResponse {
        let payload: RecalculatePricingPayload =
            serde_json::from_slice(body).unwrap_or(RecalculatePricingPayload { from_ms: None });

        let now_ms = axon_time::unix_now_ms();
        eprintln!(
            "[audit] [pricing-recalculate] at_ms={now_ms} from_ms={:?}",
            payload.from_ms
        );

        match block_on_async(recalculate_pricing_service(
            &self.config_path,
            payload.from_ms,
        ))
        .and_then(|r| r)
        {
            Ok(resp) => GatewayHttpResponse::json(200, &resp),
            Err(err) => error_response(500, ObservabilityErrorResponse::internal(err)),
        }
    }

    fn handle_auto_sync_state(&self) -> GatewayHttpResponse {
        match pricing_auto_sync_state_service(&self.config_path) {
            Ok(report) => GatewayHttpResponse::json(200, &report),
            Err(err) => error_response(500, ObservabilityErrorResponse::internal(err)),
        }
    }

    fn handle_set_auto_sync(&self, body: &[u8]) -> GatewayHttpResponse {
        let payload: SetPricingAutoSyncPayload = match serde_json::from_slice(body) {
            Ok(payload) => payload,
            Err(err) => {
                return error_response(
                    400,
                    ObservabilityErrorResponse::invalid_argument(
                        format!("JSON 报文解析失败: {err}"),
                        None,
                    ),
                );
            }
        };

        let now_ms = axon_time::unix_now_ms();
        eprintln!(
            "[audit] [pricing-auto-sync] at_ms={now_ms} enabled={}",
            payload.enabled
        );

        match set_pricing_auto_sync_service(&self.config_path, payload.enabled) {
            Ok(report) => GatewayHttpResponse::json(200, &report),
            Err(err) => error_response(500, ObservabilityErrorResponse::internal(err)),
        }
    }
}

fn block_on_async<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| Ok(handle.block_on(future)))
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("创建 Tokio 运行时失败: {e}"))
            .map(|rt| rt.block_on(future))
    }
}

fn parse_query(query: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}
