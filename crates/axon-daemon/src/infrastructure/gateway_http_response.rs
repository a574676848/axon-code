//! Gateway HTTP 响应构造与请求头读取。
//!
//! 响应体口径与既有契约逐字一致：网关自身错误一律是 `{"error":"<code>"}`，
//! 扩展路由族的未授权与未命中则是统一错误模型。两者都由本模块产出，
//! 处理逻辑里不再出现任何手写的 HTTP 报文拼接。

use crate::infrastructure::gateway_http_handler::GatewayHttpResponse;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::Response;

/// 统一错误模型里的未授权文案。
const UNAUTHORIZED_MESSAGE: &str = "未授权访问：缺少或无效的 API Token";
/// 自定义路由族未命中的文案。
const NOT_FOUND_MESSAGE: &str = "未找到指定的接口资源";

/// 按状态码与已序列化的正文构造响应；状态码非法时退化为 500，不做 panic。
fn raw(status: u16, content_type: HeaderValue, body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response
}

/// 网关自身错误响应：`{"error":"<code>"}`。
pub(super) fn json_error(status: u16, code: &str) -> Response {
    raw(
        status,
        HeaderValue::from_static("application/json"),
        serde_json::json!({ "error": code })
            .to_string()
            .into_bytes(),
    )
}

/// 统一错误模型响应；字段与顺序由 `serde_json` 保证与既有契约一致。
fn unified_error(status: u16, code: &str, message: &str) -> Response {
    let payload = serde_json::json!({
        "error": {
            "code": code,
            "message": message,
            "details": serde_json::Value::Null,
        }
    });
    raw(
        status,
        HeaderValue::from_static("application/json"),
        payload.to_string().into_bytes(),
    )
}

/// 扩展路由族的未授权响应。
pub(super) fn unauthorized() -> Response {
    unified_error(401, "UNAUTHORIZED", UNAUTHORIZED_MESSAGE)
}

/// 扩展路由族的未命中响应。
pub(super) fn unified_not_found() -> Response {
    unified_error(404, "NOT_FOUND", NOT_FOUND_MESSAGE)
}

/// JSON 成功响应。
pub(super) fn json_ok(value: &impl serde::Serialize) -> Response {
    raw(
        200,
        HeaderValue::from_static("application/json"),
        serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
    )
}

/// 交互提交回执响应。
pub(super) fn accepted(receipt: &impl serde::Serialize) -> Response {
    raw(
        202,
        HeaderValue::from_static("application/json"),
        serde_json::to_vec(receipt).unwrap_or_else(|_| b"{}".to_vec()),
    )
}

/// 把扩展处理器交出的响应搬到 axum 响应上。
///
/// 扩展处理器自带 Content-Type：非法取值退化为 JSON，而不是写出一个畸形响应。
pub(super) fn from_extension(response: GatewayHttpResponse) -> Response {
    let content_type = HeaderValue::from_str(&response.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/json"));
    raw(response.status, content_type, response.body)
}

/// 请求头里的 Bearer 凭据。
pub(super) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
}

/// 请求目标（路径 + 查询串）；查询串缺失时只有路径。
pub(super) fn request_target(uri: &Uri) -> &str {
    uri.path_and_query()
        .map_or_else(|| uri.path(), |target| target.as_str())
}

/// 声明的内容类型（去掉参数部分）。
pub(super) fn media_type(headers: &HeaderMap) -> &str {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map_or("", |value| value.split(';').next().unwrap_or("").trim())
}

/// 声明的请求体长度；缺失或非法即为 None。
pub(super) fn declared_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
}

/// 请求行与请求头的总字节数；按线上口径估算（请求行也算在内）。
///
/// 口径与既有实现一致：旧实现按已读缓冲区长度判断，缓冲区里既有请求行也有请求头，
/// 因此这里同样把 `方法 目标 HTTP/1.1\r\n` 计入，避免超长请求行绕过上限。
pub(super) fn request_head_bytes(method: &Method, uri: &Uri, headers: &HeaderMap) -> usize {
    let request_line = method.as_str().len() + request_target(uri).len() + " HTTP/1.1\r\n".len();
    request_line
        + headers
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 4)
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_errors_keep_the_compact_error_envelope() {
        let response = json_error(404, "not_found");
        assert_eq!(response.status().as_u16(), 404);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .expect("content type"),
            "application/json"
        );
    }

    #[test]
    fn extension_response_with_invalid_content_type_falls_back_to_json() {
        // 带换行的 Content-Type 是响应头注入的典型形态：必须退化成 JSON 而不是照抄。
        let response = from_extension(GatewayHttpResponse {
            status: 200,
            content_type: "application/json\r\nX-Injected: 1".to_owned(),
            body: b"{}".to_vec(),
        });
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .expect("content type"),
            "application/json"
        );
    }
}
