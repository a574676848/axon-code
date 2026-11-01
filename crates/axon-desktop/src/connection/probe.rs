//! 探活：一次 `GET /api/v1/meta` 同时拿到状态与时钟，并把失败分成三类。
//!
//! 三类结论发生在不同阶段（TLS 在 HTTP 之前），因此可以分离：凭据不匹配是 401/403，
//! 证书有问题发生在握手，连不上是 DNS、拒绝连接或超时。把证书错误并入「连不上」
//! 会让企业 TLS 中间盒下的用户永远找不到真正原因。
//!
//! 探活**不是轮询**：不设固定定时器，只在应用启动首帧之后、用户手动重试、
//! 任一网关请求返回 401 或传输失败、窗口从隐藏恢复可见时各发一次。

use std::time::{Duration, Instant};

use crate::connection::endpoint;
use crate::connection::profile::{CapabilitySet, CapsState, KnownCaps};

/// `/meta` 的协议版本；不识别时显式要求升级客户端。
pub const PROTOCOL_VERSION: u32 = 1;
/// 上传文件本身上限：64 MiB。
pub const FALLBACK_UPLOAD_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
/// `/meta` 与 `/system/status` 的服务器时间允许偏差。
const CLOCK_TOLERANCE_MS: i64 = 1_000;

/// 一次探活的截止时间。
///
/// 所有派生请求（老后端探测、`/system/status` 交叉校验、事件流端口校正）都从同一个
/// 截止时间里取剩余预算，而不是各自再吃满一份超时：一次探活的总耗时由调用方给的
/// 预算封顶，不再随派生请求的个数线性增长。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadline {
    /// 预算用尽的时刻。
    at: Instant,
}

impl Deadline {
    /// 以给定预算建立截止时间。
    pub(crate) fn after(budget: Duration) -> Self {
        Self {
            at: Instant::now() + budget,
        }
    }

    /// 剩余预算；已耗尽返回 `None`。
    fn remaining(&self) -> Option<Duration> {
        let now = Instant::now();
        (self.at > now).then(|| self.at - now)
    }
}

/// 预算耗尽时的统一失败结论。
fn budget_exhausted() -> ProbeFailure {
    ProbeFailure::Unreachable("探活预算已用尽，请求未能在上限内发出".to_owned())
}

/// `/api/v1/meta` 的载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaPayload {
    /// 协议版本。
    pub protocol_version: u32,
    /// 后端自报的能力位。
    pub capabilities: CapabilitySet,
    /// 服务器时间（毫秒）。
    pub server_time_ms: u64,
    /// 上传文件本身上限。
    pub upload_limit_bytes: u64,
    /// 本次请求所用凭据是否为超管。
    pub super_admin: bool,
}

/// `/api/v1/system/status` 里探活关心的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPayload {
    /// 服务器时间（毫秒）。
    pub server_time_ms: u64,
    /// 构建版本。
    pub build_version: Option<String>,
}

/// 探活失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    /// 401 或 403：服务端拒绝了这把凭据。
    CredentialRejected {
        /// HTTP 状态码。
        status: u16,
    },
    /// TLS 握手失败：根不受信、主机名不符、证书过期或链不完整。
    CertificateInvalid(String),
    /// DNS 解析失败、连接被拒、连接超时、读取超时。
    Unreachable(String),
    /// 响应不是合法 JSON。
    BodyInvalid(String),
    /// 协议版本不受本客户端支持。
    ProtocolUnsupported(u32),
}

impl ProbeFailure {
    /// 可展示的中文结论。
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::CredentialRejected { status } => {
                format!("凭据无效：服务端拒绝了这把凭据（HTTP {status}）")
            }
            Self::CertificateInvalid(reason) => format!("证书有问题：{reason}"),
            Self::Unreachable(reason) => format!("连不上：{reason}"),
            Self::BodyInvalid(reason) => format!("网关响应不是合法 JSON：{reason}"),
            Self::ProtocolUnsupported(version) => {
                format!("需升级客户端：后端协议版本 {version} 不受本客户端支持")
            }
        }
    }

    /// 归到健康点的哪一档：凭据与证书都是红点，连不上也是红点，协议不匹配算降级。
    #[must_use]
    pub const fn is_protocol_mismatch(&self) -> bool {
        matches!(self, Self::ProtocolUnsupported(_))
    }
}

/// 探活成功的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReady {
    /// `/meta` 载荷。
    pub meta: MetaPayload,
    /// 往返延迟（毫秒）。
    pub latency_ms: u64,
    /// 与本机的时钟偏移（毫秒）；未校验时为 0。
    pub clock_offset_ms: i64,
    /// 偏移是否已与 `/system/status` 交叉校验。
    pub clock_verified: bool,
    /// 后端构建版本。
    pub build_version: Option<String>,
}

/// 一次探活的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 取到能力位。
    Ready(Box<ProbeReady>),
    /// 连不上、凭据被拒或协议不匹配。
    Failed(ProbeFailure),
}

impl ProbeOutcome {
    /// 归到能力位两态。
    #[must_use]
    pub fn caps_state(&self) -> CapsState {
        match self {
            Self::Ready(ready) => CapsState::Known(KnownCaps {
                set: ready.meta.capabilities,
            }),
            Self::Failed(_) => CapsState::Unknown,
        }
    }
}

/// 对当前生效档案发起一次探活。
#[must_use]
pub fn probe() -> ProbeOutcome {
    let base = endpoint::http_base();
    let token = endpoint::api_token();
    probe_base_with_token(&base, token.as_deref())
}

/// 对一个显式地址发起一次探活（试连接与档案清单探测共用同一实现）。
#[must_use]
pub fn probe_base_with_token(base: &str, token: Option<&str>) -> ProbeOutcome {
    probe_base_with_token_timeout(base, token, endpoint::PROBE_TIMEOUT)
}

/// 带显式预算的探活。
///
/// 档案清单的并发探测有**总预算**（§15.2 P3）：调用方把剩余预算作为单路超时传进来，
/// 后面的档案才不会各自再吃满一整份预算。
#[must_use]
pub fn probe_base_with_token_timeout(
    base: &str,
    token: Option<&str>,
    timeout: Duration,
) -> ProbeOutcome {
    probe_within(base, token, Deadline::after(timeout))
}

/// 在调用方给的截止时间内探活。
///
/// 试连接与保存档案都是「HTTP 面 + 事件流面」两次请求，两面必须共享同一份预算，
/// 否则单次操作会随请求数线性变长。
pub(crate) fn probe_within(base: &str, token: Option<&str>, deadline: Deadline) -> ProbeOutcome {
    let url = format!("{}/api/v1/meta", base.trim_end_matches('/'));
    let Some(request_timeout) = deadline.remaining() else {
        return ProbeOutcome::Failed(budget_exhausted());
    };
    let started = Instant::now();
    let response = match endpoint::http_agent(request_timeout)
        .get(&url)
        .header("accept", "application/json")
        .header_if_present(token)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(status)) => {
            return classify_status_failure(status);
        }
        Err(error) => return ProbeOutcome::Failed(classify_transport_failure(&error)),
    };
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let status = response.status().as_u16();
    let body = match response.into_body().read_to_string() {
        Ok(body) => body,
        Err(error) => {
            return ProbeOutcome::Failed(ProbeFailure::Unreachable(format!(
                "响应体读取失败：{error}"
            )));
        }
    };
    if status != 200 {
        return classify_status_failure(status);
    }
    match parse_meta(&body) {
        Ok(meta) => {
            /*
             * 一次 `/system/status` 同时服务两个目的：交叉校验时钟与取构建版本。
             * 同一条请求发两遍既浪费预算，也让「总预算」在派生请求上失去意义。
             */
            let status_payload = fetch_status(base, token, deadline);
            let (clock_offset_ms, clock_verified) =
                clock_offset(meta.server_time_ms, status_payload.as_ref());
            let build_version = status_payload.and_then(|payload| payload.build_version);
            ProbeOutcome::Ready(Box::new(ProbeReady {
                meta,
                latency_ms,
                clock_offset_ms,
                clock_verified,
                build_version,
            }))
        }
        Err(failure) => ProbeOutcome::Failed(failure),
    }
}

/// 给请求按需加上 Authorization 头。
trait HeaderIfPresent {
    /// 有凭据就带上；没有就原样返回。
    fn header_if_present(self, token: Option<&str>) -> Self;
}

impl HeaderIfPresent for ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    fn header_if_present(self, token: Option<&str>) -> Self {
        match token {
            Some(token) if !token.is_empty() => {
                self.header("authorization", &format!("Bearer {token}"))
            }
            _ => self,
        }
    }
}

/// 把 `/meta` 的失败状态码翻成结论。
///
/// 项目未上线，后端与桌面端同版本发布，因此不再探测「老后端」：`/meta` 不在就是
/// 这个地址上没有本协议的后端，按不可达处理，交给用户核对地址与端口。
fn classify_status_failure(status: u16) -> ProbeOutcome {
    ProbeOutcome::Failed(match status {
        404 => ProbeFailure::Unreachable(
            "该地址上没有 /api/v1/meta：请确认地址与端口指向 Axon 后端".to_owned(),
        ),
        _ => classify_status_failure_value(status),
    })
}

/// 非 404 的状态码分类。
fn classify_status_failure_value(status: u16) -> ProbeFailure {
    match status {
        401 | 403 => ProbeFailure::CredentialRejected { status },
        _ => ProbeFailure::Unreachable(format!("网关返回状态码 {status}")),
    }
}

/// 用 `/system/status` 的服务器时间交叉校验 `/meta` 的时钟偏移。
///
/// 两条路由的服务器时间相差超过 1 秒即判 `/meta` 不可信：偏移按 0 处理并显式标注
/// 未校验，不把一个错的时间戳当真。
fn clock_offset(meta_server_time_ms: u64, status: Option<&StatusPayload>) -> (i64, bool) {
    let Some(status) = status else {
        return (0, false);
    };
    let delta = i64::try_from(meta_server_time_ms).unwrap_or(i64::MAX)
        - i64::try_from(status.server_time_ms).unwrap_or(i64::MAX);
    if delta.unsigned_abs() > u64::try_from(CLOCK_TOLERANCE_MS).unwrap_or(1000) {
        eprintln!(
            "WARNING: /meta 与 /system/status 的服务器时间相差 {delta}ms，本次不采信时钟偏移"
        );
        return (0, false);
    }
    (delta, true)
}

/// 读 `/system/status`；预算已耗尽或读不到时返回 `None`。
fn fetch_status(base: &str, token: Option<&str>, deadline: Deadline) -> Option<StatusPayload> {
    let timeout = deadline.remaining()?;
    let url = format!("{}/api/v1/system/status", base.trim_end_matches('/'));
    let response = endpoint::http_agent(timeout)
        .get(&url)
        .header("accept", "application/json")
        .header_if_present(token)
        .call()
        .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let body = response.into_body().read_to_string().ok()?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let server_time_ms = value.get("serverTimeMs")?.as_u64()?;
    let build_version = value
        .get("buildVersion")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some(StatusPayload {
        server_time_ms,
        build_version,
    })
}

/// 把传输层错误分成「证书有问题」与「连不上」。
fn classify_transport_failure(error: &ureq::Error) -> ProbeFailure {
    match error {
        ureq::Error::Tls(reason) => ProbeFailure::CertificateInvalid((*reason).to_owned()),
        ureq::Error::TlsRequired => {
            ProbeFailure::CertificateInvalid("该地址要求 TLS，但地址写的是明文 http".to_owned())
        }
        ureq::Error::Io(io_error) => {
            let text = io_error.to_string();
            if looks_like_tls_failure(&text) {
                ProbeFailure::CertificateInvalid(text)
            } else {
                ProbeFailure::Unreachable(text)
            }
        }
        ureq::Error::Timeout(_) => ProbeFailure::Unreachable("请求未能在探活预算内完成".to_owned()),
        ureq::Error::HostNotFound => ProbeFailure::Unreachable("域名解析失败".to_owned()),
        ureq::Error::ConnectionFailed => ProbeFailure::Unreachable("连接被拒绝".to_owned()),
        other => ProbeFailure::Unreachable(other.to_string()),
    }
}

/// 判断一段错误文本是不是 TLS 握手失败。
///
/// rustls 的握手错误会经由 `io::Error` 上抛，`ureq::Error::Tls` 只覆盖一部分情形；
/// 这里按关键词兜住另一部分，避免把证书问题报成「连不上」。
fn looks_like_tls_failure(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("certificate")
        || lowered.contains("handshake")
        || lowered.contains("tls")
        || lowered.contains("unknownissuer")
        || lowered.contains("invalid peer")
}

/// 严格解析 `/meta` 载荷：**恰好**五个字段，能力位 12 键齐全。
fn parse_meta(body: &str) -> Result<MetaPayload, ProbeFailure> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| ProbeFailure::BodyInvalid(error.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| ProbeFailure::BodyInvalid("响应不是对象".to_owned()))?;
    let protocol_version = object
        .get("protocolVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| ProbeFailure::BodyInvalid("缺少 protocolVersion".to_owned()))?;
    let protocol_version = u32::try_from(protocol_version)
        .map_err(|_| ProbeFailure::BodyInvalid("protocolVersion 超出取值范围".to_owned()))?;
    if protocol_version != PROTOCOL_VERSION {
        return Err(ProbeFailure::ProtocolUnsupported(protocol_version));
    }
    let capabilities = object
        .get("capabilities")
        .ok_or_else(|| ProbeFailure::BodyInvalid("缺少 capabilities".to_owned()))
        .and_then(|value| {
            CapabilitySet::from_meta_json(value).map_err(ProbeFailure::BodyInvalid)
        })?;
    let server_time_ms = object
        .get("serverTimeMs")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| ProbeFailure::BodyInvalid("缺少 serverTimeMs".to_owned()))?;
    let upload_limit_bytes = object
        .get("uploadLimitBytes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(FALLBACK_UPLOAD_LIMIT_BYTES);
    let super_admin = object
        .get("superAdmin")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok(MetaPayload {
        protocol_version,
        capabilities,
        server_time_ms,
        upload_limit_bytes,
        super_admin,
    })
}

/// 网关事件流路径：只挂在 WebSocket 监听上，HTTP 面没有这条路由。
pub const WS_PATH: &str = "/gateway/v1/ws";

/// 事件流端口探测的结论。
///
/// 探测必须**发一次真实的升级握手**：网关的 WebSocket 面在握手回调之前就校验升级头，
/// 缺头时直接断开连接、不回任何 HTTP 响应，于是「端口对不对」会退化成「连不上」，
/// 与真正的网络故障无法区分。这里带上完整的升级头，只取第一行状态码就断开，
/// 既不订阅事件，也不留连接。
///
/// 判定必须收得足够窄：只有明确的升级结果才算这个端口上有事件流路由。200、500、502
/// 既可能来自另一个 HTTP 服务，也可能来自中间层，把它们当成功会让保存闸门形同虚设。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsProbeOutcome {
    /// 该端口上存在事件流路由。
    Ready,
    /// 该端口没有事件流路由：地址指到了 HTTP 面（两个协议面是两条独立监听）。
    WrongPort,
    /// 凭据被拒。
    CredentialRejected {
        /// HTTP 状态码。
        status: u16,
    },
    /// 端口有响应，但不是事件流路由该有的应答。
    Unexpected {
        /// HTTP 状态码。
        status: u16,
    },
    /// 连不上。
    Unreachable(String),
}

/// 把 `/gateway/v1/ws` 的握手状态码翻成结论。
///
/// 101 是升级成功；400 与 426 是 WebSocket 服务（或它前面的代理）对不合规升级请求的
/// 标准拒绝；404 说明该端口没有这条路由。其余状态码一律不认。
const fn ws_status_outcome(status: u16) -> WsProbeOutcome {
    match status {
        101 | 400 | 426 => WsProbeOutcome::Ready,
        404 => WsProbeOutcome::WrongPort,
        401 | 403 => WsProbeOutcome::CredentialRejected { status },
        other => WsProbeOutcome::Unexpected { status: other },
    }
}

/// 探测请求的 `Sec-WebSocket-Key`：固定值。
///
/// 它只用于换一次 101，没有任何保密含义；固定值让服务端日志里这条探测一眼可辨。
const WS_PROBE_KEY: &str = "QXhvbiBwcm9iZSBrZXkhIQ==";

/// 探测 WebSocket 端口。
#[must_use]
pub fn probe_websocket(ws_base: &str, token: Option<&str>) -> WsProbeOutcome {
    probe_websocket_within(ws_base, token, Deadline::after(endpoint::PROBE_TIMEOUT))
}

/// 在给定截止时间内探测 WebSocket 端口。
fn probe_websocket_within(
    ws_base: &str,
    token: Option<&str>,
    deadline: Deadline,
) -> WsProbeOutcome {
    let Some(timeout) = deadline.remaining() else {
        return WsProbeOutcome::Unreachable("探活预算已用尽，事件流端口没有探测".to_owned());
    };
    let http_base = endpoint::ws_base_to_http(ws_base);
    let url = format!("{}{WS_PATH}", http_base.trim_end_matches('/'));
    match endpoint::http_agent(timeout)
        .get(&url)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", WS_PROBE_KEY)
        .header_if_present(token)
        .call()
    {
        Ok(response) => {
            let status = response.status().as_u16();
            // 101 没有正文，握手完成后立刻断开：这里只要状态码。
            drop(response);
            ws_status_outcome(status)
        }
        Err(ureq::Error::StatusCode(status)) => ws_status_outcome(status),
        Err(error) => WsProbeOutcome::Unreachable(classify_transport_failure(&error).message()),
    }
}

/// 解析事件流地址：先按同一 authority 试，试不通再用后端自报的端口校正。
///
/// 网关把 HTTP 与 WebSocket 绑成两条独立监听，客户端无法从 HTTP 地址里推出
/// WebSocket 端口。因此这里分两步：
///
/// 1. 先在 HTTP 地址的同一 authority 上问 `/gateway/v1/ws`。两者共用一条监听时
///    在这一步就通过。
/// 2. 返回 404 说明该端口没有事件流路由，此时读 `/system/status` 里后端自报的
///    `webSocketPort`，把端口换掉再试一次。
///
/// 两步都失败才报「端口不对」。这样用户只填一个 HTTP 地址也能连通，不必知道网关
/// 把事件流绑在哪个端口，也不需要中间层做端口归一。
#[must_use]
pub fn resolve_ws_base(
    http_base: &str,
    token: Option<&str>,
    ws_base: &str,
) -> (String, WsProbeOutcome) {
    resolve_ws_base_within(
        http_base,
        token,
        ws_base,
        Deadline::after(endpoint::PROBE_TIMEOUT),
    )
}

/// 在调用方给的截止时间内解析事件流地址。
///
/// 两次探测与一次端口读取共享同一份预算：试连接与保存都把 HTTP 面剩下的时间交给它，
/// 整个操作的总耗时因此只由调用方给的预算决定。
pub(crate) fn resolve_ws_base_within(
    http_base: &str,
    token: Option<&str>,
    ws_base: &str,
    deadline: Deadline,
) -> (String, WsProbeOutcome) {
    let first = probe_websocket_within(ws_base, token, deadline);
    if !matches!(first, WsProbeOutcome::WrongPort) {
        return (ws_base.to_owned(), first);
    }
    let Some(port) = fetch_web_socket_port(http_base, token, deadline) else {
        return (ws_base.to_owned(), first);
    };
    let corrected = endpoint::ws_base_with_port(ws_base, port);
    if corrected == ws_base {
        return (ws_base.to_owned(), first);
    }
    let second = probe_websocket_within(&corrected, token, deadline);
    (corrected, second)
}

/// 读后端自报的 WebSocket 端口。
fn fetch_web_socket_port(http_base: &str, token: Option<&str>, deadline: Deadline) -> Option<u16> {
    let timeout = deadline.remaining()?;
    let url = format!("{}/api/v1/system/status", http_base.trim_end_matches('/'));
    let response = endpoint::http_agent(timeout)
        .get(&url)
        .header("accept", "application/json")
        .header_if_present(token)
        .call()
        .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let body = response.into_body().read_to_string().ok()?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let port = value.get("webSocketPort")?.as_u64()?;
    u16::try_from(port).ok().filter(|port| *port != 0)
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod probe_tests;
#[cfg(test)]
#[path = "probe_tls_tests.rs"]
mod probe_tls_tests;
