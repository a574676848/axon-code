//! `/api/v1/*` 的每路由请求体上限表与准入判定。
//!
//! 网关的 router 级上限只服务事件路由与兜底路径，扩展路由族的正文上限是逐路由契约
//! 的一部分：配置写入允许 4 MiB，密钥写入只允许 64 KiB，附件上传是 64 MiB 加固定开销。
//! 若整族共用一条全局取值，内存占用就由对端声明的体积决定而不是由路由决定。
//!
//! 表外路径统一回落 [`FALLBACK_BODY_LIMIT`]，绝不回落到 router 级取值：未登记的路由
//! 只应拿到默认额度，不应因此获得 300 MiB 的收体权。上限为 0 表示该路由不声明正文，
//! 带正文时按超限拒绝，与其它超限共用同一错误码语义。

/// 表外 `/api/v1/*` 路径的统一正文上限。
pub(super) const FALLBACK_BODY_LIMIT: usize = 4 * 1024 * 1024;
/// 不声明正文的路由上限。
const NO_BODY_LIMIT: usize = 0;
/// 配置写入类路由的上限。
const CONFIG_BODY_LIMIT: usize = 4 * 1024 * 1024;
/// 密钥写入类路由的上限。
const SECRET_BODY_LIMIT: usize = 64 * 1024;
/// 人设与行为文件写入的上限。
const DOCUMENT_BODY_LIMIT: usize = 1024 * 1024;
/// 附件上传的请求体上限。
///
/// 文件本身的上限是 64 MiB，额外 8 KiB 是 multipart 分隔符与各段头部的固定开
/// 销。文件字节数只能在读体过程中判定，本值只用来提前挡住声明超限的请求。
const ATTACHMENT_BODY_LIMIT: usize = 64 * 1024 * 1024 + 8 * 1024;
/// 签发 Key 的请求体上限。
const KEY_BODY_LIMIT: usize = 4 * 1024;
/// 通知送达回执的请求体上限：只带通知标识与目标，几 KiB 足够。
const NOTIFICATION_ACK_BODY_LIMIT: usize = 4 * 1024;

/// 一条路由的正文上限登记。
struct RouteLimit {
    /// 方法名，必须与请求行逐字一致（区分大小写）。
    method: &'static str,
    /// 路径模板；某一段写成 `{参数名}` 即表示该段是一维路径参数。
    path: &'static str,
    /// 请求体上限（字节）。
    limit: usize,
}

/// 构造一条上限登记。
const fn route(method: &'static str, path: &'static str, limit: usize) -> RouteLimit {
    RouteLimit {
        method,
        path,
        limit,
    }
}

/// 已登记的每路由正文上限。
///
/// 新增路由时在此加一行即可；未登记的路由按 [`FALLBACK_BODY_LIMIT`] 收体。
const ROUTE_LIMITS: &[RouteLimit] = &[
    route("GET", "/api/v1/meta", NO_BODY_LIMIT),
    route("GET", "/api/v1/system/status", NO_BODY_LIMIT),
    route("GET", "/api/v1/config", NO_BODY_LIMIT),
    route("GET", "/api/v1/config/schema", NO_BODY_LIMIT),
    route("PUT", "/api/v1/config", CONFIG_BODY_LIMIT),
    route("GET", "/api/v1/secrets/status", NO_BODY_LIMIT),
    route("PUT", "/api/v1/secrets", SECRET_BODY_LIMIT),
    route("POST", "/api/v1/providers/discover", SECRET_BODY_LIMIT),
    route("GET", "/api/v1/mcp/config", NO_BODY_LIMIT),
    route("PUT", "/api/v1/mcp/config", CONFIG_BODY_LIMIT),
    route("GET", "/api/v1/skills", NO_BODY_LIMIT),
    route("DELETE", "/api/v1/skills/{id}", NO_BODY_LIMIT),
    route("GET", "/api/v1/files", NO_BODY_LIMIT),
    route("GET", "/api/v1/files/content", NO_BODY_LIMIT),
    route("GET", "/api/v1/fs/dirs", NO_BODY_LIMIT),
    route("GET", "/api/v1/documents/soul", NO_BODY_LIMIT),
    route("GET", "/api/v1/documents/agents", NO_BODY_LIMIT),
    route("PUT", "/api/v1/documents/soul", DOCUMENT_BODY_LIMIT),
    route("PUT", "/api/v1/documents/agents", DOCUMENT_BODY_LIMIT),
    route("POST", "/api/v1/attachments", ATTACHMENT_BODY_LIMIT),
    route("POST", "/api/v1/keys", KEY_BODY_LIMIT),
    route("GET", "/api/v1/keys", NO_BODY_LIMIT),
    route("DELETE", "/api/v1/keys/{id}", NO_BODY_LIMIT),
    route("GET", "/api/v1/notifications", NO_BODY_LIMIT),
    route("POST", "/api/v1/notifications/ack", NOTIFICATION_ACK_BODY_LIMIT),
];

/// 按 (方法, 路径) 取该路由的请求体上限；未登记时回落 [`FALLBACK_BODY_LIMIT`]。
pub(super) fn body_limit(method: &str, path: &str) -> usize {
    ROUTE_LIMITS
        .iter()
        .find(|entry| entry.method == method && path_matches(entry.path, path))
        .map_or(FALLBACK_BODY_LIMIT, |entry| entry.limit)
}

/// 正文准入结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BodyAdmission {
    /// 允许读取正文，读体上限即本值。
    Allowed(usize),
    /// 声明的长度超过该路由上限。
    TooLarge,
    /// 声明了分块传输，但没有声明长度。
    LengthRequired,
}

/// 按路由上限与请求头声明的传输形态判定是否允许读体。
///
/// 只对「声明了正文」的请求做长度校验：有长度按长度比，分块传输要长度，两者都没有
/// 即视为无正文按 0 处理，GET、HEAD、DELETE 因此不受影响。
pub(super) fn admit_body(limit: usize, declared: Option<usize>, chunked: bool) -> BodyAdmission {
    match (declared, chunked) {
        (Some(length), _) if length > limit => BodyAdmission::TooLarge,
        (Some(_), _) => BodyAdmission::Allowed(limit),
        (None, true) => BodyAdmission::LengthRequired,
        (None, false) => BodyAdmission::Allowed(limit),
    }
}

/// 模板段是否为路径参数（`{参数名}` 形态）。
fn is_parameter(segment: &str) -> bool {
    segment.len() > 2 && segment.starts_with('{') && segment.ends_with('}')
}

/// 按段比对路径模板与实际路径。
///
/// 参数段匹配且只匹配一段非空路径，因此 `/api/v1/keys/{id}` 命中
/// `/api/v1/keys/abc123`，但不命中 `/api/v1/keys/abc/def` 与 `/api/v1/keys/`。
fn path_matches(template: &str, path: &str) -> bool {
    let mut template_segments = template.split('/');
    let mut path_segments = path.split('/');
    loop {
        match (template_segments.next(), path_segments.next()) {
            (None, None) => return true,
            (Some(template_segment), Some(path_segment)) => {
                let matched = if is_parameter(template_segment) {
                    !path_segment.is_empty()
                } else {
                    template_segment == path_segment
                };
                if !matched {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

#[cfg(test)]
#[path = "gateway_route_limits_tests.rs"]
mod gateway_route_limits_tests;
