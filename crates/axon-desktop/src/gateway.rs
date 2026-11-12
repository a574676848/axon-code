//! 网关客户端：桌面端访问 Axon 运行态的唯一入口。
//!
//! 桌面端不直连运行态数据库，也不链接会话子系统，一律经网关 HTTP 协议读写，
//! 从而与后端实现解耦。
//!
//! 本模块只保留 HTTP 动词层：地址、凭据与 TLS 信任来源都由
//! [`crate::connection::endpoint`] 解析，连接档案一换，这里的所有请求自动跟着换。
//! 网关有**两个协议面**：工作台扩展面以 [`API_PREFIX`] 为前缀，交互事件面则是
//! `/gateway/v1/*`。两者共用同一份地址与凭据，因此地址组装只在这里实现一次。

use std::time::Duration;

use axon_config::ConfigLoader;

use crate::connection::endpoint;

/// 网关凭据环境变量名配置键。
pub use crate::connection::endpoint::KEY_API_TOKEN_ENV;
/// 网关 HTTP 监听地址配置键。
pub use crate::connection::endpoint::KEY_HTTP_BIND;
/// 网关 WebSocket 监听地址配置键。
pub use crate::connection::endpoint::KEY_WEBSOCKET_BIND;
/// 把通配绑定地址映射成可连接的探测地址。
pub use crate::connection::endpoint::probe_target;

// 语音相关的网关调用在 `gateway_voice`；调用方仍按 `gateway::xxx` 取，路径不因拆分而变。
pub use crate::gateway_voice::{
    get_voice_presets, get_voice_runtime_status, post_voice_preview, post_voice_tts,
    voice_websocket_url, VoicePreviewAudio, VoicePreviewError, VoiceTtsError,
};

/// 单次请求超时。
const REQUEST_TIMEOUT: Duration = endpoint::REQUEST_TIMEOUT;

/// 读取配置项文本值。
///
/// 只服务诊断类读数（系统信息、服务修复引导），**不在请求路径上**：请求路径的
/// 地址与凭据一律取 [`crate::connection`] 的生效档案，不在这里读盘。
pub fn config_text(key: &str) -> Option<String> {
    let path = ConfigLoader::default_user_config_path().ok()?;
    axon_config::raw_string_value(&path, key)
}

/// 网关根地址，形如 `http://127.0.0.1:3143`。
///
/// # Errors
///
/// 当前实现不会失败：地址总有兜底值。保留 `Result` 形态是为了不改变既有调用点。
pub fn base_url() -> Result<String, String> {
    Ok(endpoint::http_base())
}

/// 网关 WebSocket 根地址，形如 `ws://127.0.0.1:3144`。
///
/// # Errors
///
/// 同 [`base_url`]。
pub fn websocket_base_url() -> Result<String, String> {
    Ok(endpoint::websocket_base())
}

/// 网关凭据；未配置时返回 None。
pub fn api_token() -> Option<String> {
    endpoint::api_token()
}

/// 构造带鉴权的请求代理。
fn agent() -> ureq::Agent {
    endpoint::http_agent(REQUEST_TIMEOUT)
}

pub(crate) fn agent_with_timeout(timeout: Duration) -> ureq::Agent {
    endpoint::http_agent(timeout)
}

/// 网关请求共用的路径前缀。
const API_PREFIX: &str = "/api/v1";

/// 组装完整地址。
pub(crate) fn endpoint(path: &str) -> Result<String, String> {
    Ok(format!("{}{API_PREFIX}{path}", base_url()?))
}

/// 组装不经工作台前缀的完整地址。
///
/// 交互事件面（`/gateway/v1/*`）与工作台面同源同凭据，但不共用前缀，
/// 因此这里显式接受完整路径，避免调用方各自拼字符串。
pub fn endpoint_at(path: &str) -> Result<String, String> {
    Ok(format!("{}{path}", base_url()?))
}

/// 构造带鉴权的 GET 请求。
fn authorized_get_with_timeout(
    url: &str,
    timeout: Duration,
) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    let mut request = agent_with_timeout(timeout)
        .get(url)
        .header("accept", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    request
}

/// GET 一个网关接口并解析 JSON。
pub fn get_json(path: &str) -> Result<serde_json::Value, String> {
    let (status, body) = get_raw(path)?;
    if !(200..300).contains(&status) {
        return Err(format!("网关返回状态码 {status}"));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// GET 一个网关接口，返回状态码与响应体。
///
/// 探测需要区分状态码：401 是凭据问题，404 是服务端不含该接口（版本过旧），
/// 二者修复动作完全不同，必须分别归类而不是笼统报「接口调用失败」。
pub fn get_raw(path: &str) -> Result<(u16, String), String> {
    get_raw_with_timeout(path, REQUEST_TIMEOUT)
}

/// GET 一个网关接口，并使用调用方指定的总超时。
pub fn get_raw_with_timeout(path: &str, timeout: Duration) -> Result<(u16, String), String> {
    let url = endpoint(path)?;
    execute_get(&url, timeout)
}

/// GET 一个**完整路径**的网关接口（路径自带 `/api/v1` 前缀）。
///
/// 与 [`get_raw`] 的分工必须写清楚：`get_raw` 收的是相对路径，由本模块补前缀；
/// 本函数收的是完整路径，不再补。配置类客户端自己保管完整路由常量，两者混用会
/// 拼出 `/api/v1/api/v1/...` 这种必然 404 的地址，而 404 又会被翻成「后台服务
/// 版本过旧」，把一个拼装错误说成后端太旧。
pub fn get_raw_at(path: &str) -> Result<(u16, String), String> {
    get_raw_at_with_timeout(path, REQUEST_TIMEOUT)
}

/// GET 一个完整路径的网关接口，并使用调用方指定的总超时。
pub fn get_raw_at_with_timeout(path: &str, timeout: Duration) -> Result<(u16, String), String> {
    let url = endpoint_at(path)?;
    execute_get(&url, timeout)
}

/// 发一次带鉴权的 GET 并读回状态码与正文。
fn execute_get(url: &str, timeout: Duration) -> Result<(u16, String), String> {
    let request = authorized_get_with_timeout(url, timeout);
    match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            Ok((status, body))
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(transport_failure_message(&error)),
    }
}

/// 传输层失败的结论。
///
/// 「超时」与「不可达」的修复动作完全不同：前者是这次请求在预算内没回来（网关正在
/// 排队、或一条大会话的历史还没读完），后者是端口没人接受连接。把两者都说成
/// 「网关不可达」，会让人去重启一个其实好好的后台服务，而这正是历史回放最容易
/// 撞上的那一次失败。
pub(crate) fn transport_failure_message(error: &ureq::Error) -> String {
    match error {
        // 连接阶段超时是「握手没等到」，等同于连不上；其余阶段超时都是「慢」。
        ureq::Error::Timeout(ureq::Timeout::Connect) => format!("网关不可达：{error}"),
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => format!("网关响应超时：{error}"),
        _ => format!("网关不可达：{error}"),
    }
}

/// 写请求的传输层结论：超时单独成句，其余沿用既有措辞。
fn transport_failure_message_rejected(error: &ureq::Error) -> String {
    match error {
        // 连接阶段超时是「握手没等到」，等同于连不上；其余阶段超时都是「慢」。
        ureq::Error::Timeout(ureq::Timeout::Connect) => format!("网关不可达：{error}"),
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => format!("网关响应超时：{error}"),
        _ => format!("网关不可达或拒绝请求：{error}"),
    }
}

/// 探活失败的原因分类：决定桌面端给用户哪一句话、要不要重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    /// 请求在预算内没有完成：网关可能正在排队，也可能是连接挂住。
    Timeout,
    /// 连接层失败：端口无人接受连接，或链路根本不通。
    Unreachable,
    /// 网关已经应答，但响应体没能读完。
    BodyRead,
}

/// 带失败分类的 GET。
///
/// 口径与 [`get_raw_with_timeout`] 完全一致，只是把传输层错误分成「超时」与
/// 「不可达」两类：二者的修复动作不同，笼统报一句不可达会把排队误报成宕机。
pub fn probe_raw_with_timeout(
    path: &str,
    timeout: Duration,
) -> Result<(u16, String), ProbeFailure> {
    let url = endpoint(path).map_err(|_| ProbeFailure::Unreachable)?;
    match authorized_get_with_timeout(&url, timeout).call() {
        Ok(response) => {
            let status = response.status().as_u16();
            response
                .into_body()
                .read_to_string()
                .map(|body| (status, body))
                .map_err(|_| ProbeFailure::BodyRead)
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(classify_transport_failure(&error)),
    }
}

/// 把传输层错误归到探活的两类失败上；读体失败已在上游单独归类。
fn classify_transport_failure(error: &ureq::Error) -> ProbeFailure {
    match error {
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => ProbeFailure::Timeout,
        ureq::Error::Io(io_error)
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            ProbeFailure::Timeout
        }
        _ => ProbeFailure::Unreachable,
    }
}

/// GET 一个绝对路径接口，要求 2xx 并把响应解析为 JSON。
pub fn get_json_at(path: &str) -> Result<serde_json::Value, String> {
    get_json_at_with_timeout(path, REQUEST_TIMEOUT)
}

/// 同 [`get_json_at`]，但用调用方给的预算。
///
/// 历史帧页读的是持久日志：一条大会话的一页本身就要扫成千上万帧，通用的 15 秒预算
/// 会把「还在读」报成「网关不可达」。这类调用方自己声明预算，别让通路口径替它决定。
pub fn get_json_at_with_timeout(
    path: &str,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let url = endpoint_at(path)?;
    let (status, body) = match authorized_get_with_timeout(&url, timeout).call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(transport_failure_message(&error)),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// POST 一个网关接口，返回状态码与响应体。
///
/// 需要按状态码分档的调用方走这一层：403 与 401 的修复动作不同，只拿到一句话就没法分开
/// 告诉用户「换个超管凭据」还是「凭据失效了」。只关心成败的调用方继续用 [`post_json`]。
pub fn post_raw(path: &str, payload: &serde_json::Value) -> Result<(u16, String), String> {
    let url = endpoint(path)?;
    let mut request = agent()
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    match request.send(payload.to_string()) {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            Ok((status, body))
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(transport_failure_message_rejected(&error)),
    }
}

/// POST 一个网关接口并解析 JSON。
pub fn post_json(path: &str, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
    let (status, body) = post_raw(path, payload)?;
    decode_json(status, &body)
}

/// DELETE 一个网关接口，返回状态码与响应体。
pub fn delete_raw(path: &str) -> Result<(u16, String), String> {
    let url = endpoint(path)?;
    let mut request = agent().delete(&url).header("accept", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            Ok((status, body))
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(transport_failure_message_rejected(&error)),
    }
}

/// DELETE 一个网关接口并解析 JSON。
pub fn delete_json(path: &str) -> Result<serde_json::Value, String> {
    let (status, body) = delete_raw(path)?;
    decode_json(status, &body)
}

/// 2xx 才解析 JSON；非 2xx 一律翻译成可执行的中文说明。
///
/// 动词层与需要自己看状态码的调用方共用这一处，避免出现两套错误口径。
pub(crate) fn decode_json(status: u16, body: &str) -> Result<serde_json::Value, String> {
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, body));
    }
    serde_json::from_str(body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// POST 一个绝对路径接口，要求 2xx 并把响应解析为 JSON。
pub fn post_json_at(path: &str, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
    let url = endpoint_at(path)?;
    let mut request = agent()
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let (status, body) = match request.send(payload.to_string()) {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(transport_failure_message_rejected(&error)),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// DELETE 一个绝对路径接口，要求 2xx 并把响应解析为 JSON。
///
/// 与 [`post_json_at`] 同一套路径与错误口径：路径自带 `/api/v1` 前缀，因此走
/// 不补前缀的那条组装；DELETE 不带请求体，网关对这类路由也不声明正文。
pub(crate) fn delete_json_at(path: &str) -> Result<serde_json::Value, String> {
    let url = endpoint_at(path)?;
    let mut request = agent().delete(&url).header("accept", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let (status, body) = match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(transport_failure_message_rejected(&error)),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// 把非 2xx 响应翻译成可执行的中文说明。
///
/// 401 是凭据问题、404 是服务端不含该接口（版本过旧），二者修复动作完全不同，
/// 不能笼统报「调用失败」；网关自身的错误码原样附在后面，便于定位。
pub(crate) fn gateway_status_message(status: u16, body: &str) -> String {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value.get("error").and_then(|error| {
                error.as_str().map(str::to_owned).or_else(|| {
                    error
                        .get("code")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
            })
        })
        .unwrap_or_default();
    let suffix = if code.is_empty() {
        String::new()
    } else {
        format!("（{code}）")
    };
    match status {
        // 401 是凭据问题，403 是网关入口策略拒绝了这条事件（字段、附件或时效不合规）：
        // 两者的修复动作不同，共用一句「凭据不匹配」会把内容问题报成配置问题。
        401 => format!("网关拒绝访问：凭据缺失或不匹配{suffix}"),
        403 if code == "event_rejected" => {
            format!("网关拒绝了这条消息：内容、附件或时效不合规{suffix}")
        }
        403 => format!("网关拒绝访问：凭据缺失或不匹配{suffix}"),
        // 目标详情 404 说「这个目标不在了」，路由缺失 404 说「后台版本过旧」：
        // 两者的下一步完全不同，因此这条分支必须排在通用 404 之前。
        404 if code == "goal_not_found" => format!("目标不存在或已被清理{suffix}"),
        404 => format!("网关不含该接口，后台服务版本可能过旧{suffix}"),
        429 => format!("网关繁忙，请稍后重试{suffix}"),
        _ => format!("网关返回状态码 {status}{suffix}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{gateway_status_message, transport_failure_message};

    #[test]
    fn a_slow_gateway_is_not_reported_as_an_unreachable_one() {
        // 慢与断是两件事：预算内没回来该等或该减少单页体量，连不上才该去重启后台。
        let slow = ureq::Error::Timeout(ureq::Timeout::Global);
        let message = transport_failure_message(&slow);
        assert!(message.starts_with("网关响应超时"), "{message}");
        assert!(!message.contains("网关不可达"), "{message}");

        let handshake = ureq::Error::Timeout(ureq::Timeout::Connect);
        assert!(
            transport_failure_message(&handshake).starts_with("网关不可达"),
            "连接阶段超时按不可达归类"
        );
    }

    #[test]
    fn credential_and_ingress_rejections_give_different_advice() {
        let unauthorized = gateway_status_message(401, r#"{"error":"unauthorized"}"#);
        assert!(unauthorized.contains("凭据"), "{unauthorized}");

        let rejected = gateway_status_message(403, r#"{"error":"event_rejected"}"#);
        assert!(rejected.contains("内容、附件或时效"), "{rejected}");
        assert!(!rejected.contains("凭据"), "{rejected}");
        assert!(rejected.contains("event_rejected"), "{rejected}");
    }

    #[test]
    fn goal_not_found_is_reported_as_a_missing_goal_not_an_old_service() {
        // 目标详情 404 与路由缺失 404 是两件事：前者重试无用，后者要升级后台。
        let missing = gateway_status_message(
            404,
            r#"{"error":{"code":"goal_not_found","message":"目标不存在或已被清理","details":null}}"#,
        );
        assert_eq!(missing, "目标不存在或已被清理（goal_not_found）");

        // 通用 404 文案保持不变：没有错误码的路由缺失仍归因到版本过旧。
        let no_route = gateway_status_message(404, "");
        assert_eq!(no_route, "网关不含该接口，后台服务版本可能过旧");
        let other_code = gateway_status_message(404, r#"{"error":{"code":"NOT_FOUND"}}"#);
        assert_eq!(
            other_code,
            "网关不含该接口，后台服务版本可能过旧（NOT_FOUND）"
        );
    }

    #[test]
    fn preview_status_classification_follows_protocol_codes() {
        use crate::gateway_voice::{classify_preview_status, VoicePreviewError};

        let unavailable = classify_preview_status(
            409,
            r#"{"error":"provider_unavailable","message":"尚未接入云端语音提供方，请先配置阿里百炼或火山引擎"}"#,
        );
        assert!(
            matches!(unavailable, VoicePreviewError::Unavailable(ref reason) if reason.contains("尚未接入云端语音提供方")),
            "{unavailable:?}"
        );

        let timeout = classify_preview_status(504, r#"{"error":"preview_timeout"}"#);
        assert!(matches!(timeout, VoicePreviewError::Timeout));

        let invalid = classify_preview_status(400, r#"{"error":"invalid_json"}"#);
        assert!(
            matches!(invalid, VoicePreviewError::Invalid(ref reason) if !reason.is_empty()),
            "{invalid:?}"
        );

        // 没有服务端文案时按错误码兜底，不把空串交给界面。
        let fallback = classify_preview_status(409, r#"{"error":"tts_failed"}"#);
        assert!(
            matches!(fallback, VoicePreviewError::Unavailable(ref reason) if reason.contains("朗读失败")),
            "{fallback:?}"
        );
    }
}

