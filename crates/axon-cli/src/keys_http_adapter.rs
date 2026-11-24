//! `/api/v1/keys` 管理面：超管列出、签发与吊销 Gateway 多 Key。
//!
//! 三条口径不可退让：
//!
//! 1. **只接受超管**。身份来自网关路由层解析出的 [`GatewayIdentity`]（不是布尔值），
//!    适配器按身份分级而不是重新鉴权。普通 Key 的凭据本身有效、缺的是权限级别，因此
//!    回 403 而不是 401；路由层已经在管理面关掉了 `?api_token=` 查询串通道，本适配器
//!    拿不到、也不接受第二条凭据通道。
//! 2. **明文只出现一次**。签发回执带明文，此后列表只回标签、两个时间与指纹前 6 位，
//!    任何响应、错误文案与日志都不再出现明文与摘要。
//! 3. **存储句柄由装配层注入**，与鉴权层共享同一个 `Arc`。这里绝不自己 `open` 一份：
//!    两份存储的写入互不可见，签发与吊销的「热更新」会当场失效。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axon_daemon::GatewayHttpResponse;
use axon_daemon::infrastructure::gateway_auth::{
    GatewayIdentity, GatewayKeyError, GatewayKeyStore, KeyOrigin,
};
use serde_json::{Value, json};

/// 列出与签发路由。
const KEYS_PATH: &str = "/api/v1/keys";
/// 吊销路由前缀；其后一段是 Key 的 `id`。
const KEYS_ID_PREFIX: &str = "/api/v1/keys/";
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 非超管凭据的错误码。
const FORBIDDEN: &str = "forbidden";
/// 请求体不是合法 JSON 的错误码。
const INVALID_JSON: &str = "invalid_json";
/// 标签缺失或空白的错误码。
const MISSING_LABEL: &str = "missing_label";
/// 指定 Key 不存在（含已吊销）的错误码。
const KEY_NOT_FOUND: &str = "key_not_found";
/// 存储层不可用的错误码。
const KEY_STORE_ERROR: &str = "key_store_error";

/// 多 Key 管理面适配器。
pub struct KeysHttpAdapter {
    /// 与鉴权层共享的 Key 存储。
    store: Arc<GatewayKeyStore>,
}

impl KeysHttpAdapter {
    /// 以共享的存储句柄构造。
    ///
    /// 调用方必须传入**与 [`axon_daemon::GatewayAuth`] 同一份** `Arc`；重新 `open` 一份
    /// 会让签发结果对鉴权层不可见。
    #[must_use]
    pub fn new(store: Arc<GatewayKeyStore>) -> Self {
        Self { store }
    }

    /// 统一入口：路径未命中返回 None，命中后先判身份、再判方法。
    fn route(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        identity: Option<&GatewayIdentity>,
        peer: Option<&str>,
    ) -> Option<GatewayHttpResponse> {
        // 只认自己这两条精确路径，未命中一律交回组合器：返回统一 404 会截断后续适配器。
        // 吊销路由的 `{id}` 只匹配一段非空路径（与网关的每路由上限表同一口径），
        // 因此 `/api/v1/keys/` 与 `/api/v1/keys/abc/def` 都不属于本族。
        let id = path
            .strip_prefix(KEYS_ID_PREFIX)
            .filter(|rest| !rest.is_empty() && !rest.contains('/'));
        let is_collection = path == KEYS_PATH;
        if !is_collection && id.is_none() {
            return None;
        }
        // 身份判定排在方法判定之前：管理面「只接受超管」优先于「这个方法不存在」，
        // 否则普通 Key 能用 405 与 404 的差别把管理面的路由形状探出来。
        if !identity.is_some_and(GatewayIdentity::is_super_admin) {
            return Some(error(403, FORBIDDEN));
        }
        let origin = KeyOrigin::http(
            &identity.map_or_else(String::new, GatewayIdentity::describe),
            peer,
        );
        Some(match id {
            Some(id) => match method {
                "DELETE" => self.revoke(id, &origin),
                _ => method_not_allowed(),
            },
            None => match method {
                "GET" => self.list(),
                "POST" => self.create(body, &origin),
                _ => method_not_allowed(),
            },
        })
    }

    /// `GET /api/v1/keys`：只回标签、创建时间、最后使用时间与指纹前 6 位。
    ///
    /// 载荷直接用存储层的列表投影：字段名与「不含明文与摘要」这条边界都在那一层定死，
    /// 这里不再二次拼装，避免两处口径各自演化。
    fn list(&self) -> GatewayHttpResponse {
        GatewayHttpResponse::json(200, &json!({ "keys": self.store.list() }))
    }

    /// `POST /api/v1/keys`：体为 `{"label":"..."}`，明文只在本回执里出现一次。
    fn create(&self, body: &[u8], origin: &KeyOrigin) -> GatewayHttpResponse {
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return error(400, INVALID_JSON);
        };
        let Some(label) = request.get("label").and_then(Value::as_str) else {
            return error_with_message(400, MISSING_LABEL, "缺少 label：签发 Key 必须给出用途标签");
        };
        let label = label.trim();
        if label.is_empty() {
            return error_with_message(400, MISSING_LABEL, "label 不能为空白：标签用于审计追溯");
        }
        match self.store.create_as(label, origin) {
            Ok(issued) => {
                let created_at_ms = self.created_at_ms(&issued.id);
                GatewayHttpResponse::json(
                    200,
                    &json!({
                        "id": issued.id,
                        "label": issued.label,
                        // 明文只在这里出现一次；列表、日志与审计都不再带它。
                        "plaintext": issued.plaintext,
                        "createdAtMs": created_at_ms,
                    }),
                )
            }
            Err(error) => store_failure(&error),
        }
    }

    /// `DELETE /api/v1/keys/{id}`：吊销一把 Key，对下一次鉴权立即生效。
    fn revoke(&self, id: &str, origin: &KeyOrigin) -> GatewayHttpResponse {
        let id = id.trim();
        if id.is_empty() {
            return error(404, KEY_NOT_FOUND);
        }
        match self.store.revoke_as(id, origin) {
            Ok(()) => GatewayHttpResponse::json(200, &json!({ "id": id, "revoked": true })),
            // 已吊销与不存在给同一个 404：列表里都看不到它，对调用方是同一件事。
            Err(GatewayKeyError::UnknownKey(_) | GatewayKeyError::AlreadyRevoked(_)) => {
                error(404, KEY_NOT_FOUND)
            }
            Err(error) => store_failure(&error),
        }
    }

    /// 刚签发的 Key 在存储投影里的创建时间。
    ///
    /// 签发回执只有 `id`、`label` 与明文，创建时间由列表投影承担（存储层是唯一事实源），
    /// 因此这里按 `id` 回查一次。记录写入先于回执返回，回查不到只可能是存储另有改动，
    /// 此时按当前时刻回填，绝不回 0：0 会被界面读成 1970 年。
    fn created_at_ms(&self, id: &str) -> u64 {
        self.store
            .list()
            .iter()
            .find(|key| key.id == id)
            .map_or_else(now_ms, |key| key.created_at_ms)
    }
}

impl axon_daemon::GatewayHttpRouteHandler for KeysHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        // 无身份的调用一律按「非超管」处理：管理面在拿不到身份时绝不放行。
        self.route(method, path, body, None, None)
    }

    fn handle_request_as(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
        identity: Option<&GatewayIdentity>,
        peer: Option<&str>,
    ) -> Option<GatewayHttpResponse> {
        self.route(method, path, body, identity, peer)
    }
}

/// 当前时间（毫秒），与存储层同一口径；系统时间早于纪元时按 0 处理。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// 只有错误码的响应。
fn error(status: u16, code: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &json!({ "error": code }))
}

/// 带中文原因的响应。
fn error_with_message(status: u16, code: &str, message: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &json!({ "error": code, "message": message }))
}

/// 方法不匹配响应。
fn method_not_allowed() -> GatewayHttpResponse {
    error(405, METHOD_NOT_ALLOWED)
}

/// 存储层失败的响应。
///
/// [`GatewayKeyError`] 的中文文案只带 `id`、`label` 与路径，不含明文与摘要，因此可以
/// 原样回给调用方；标签为空这条属于入参问题，按 400 而不是 500 呈现。
fn store_failure(error: &GatewayKeyError) -> GatewayHttpResponse {
    match error {
        GatewayKeyError::EmptyLabel => error_with_message(400, MISSING_LABEL, "label 不能为空白"),
        other => error_with_message(500, KEY_STORE_ERROR, &other.to_string()),
    }
}

#[cfg(test)]
#[path = "keys_http_adapter_tests.rs"]
mod keys_http_adapter_tests;
