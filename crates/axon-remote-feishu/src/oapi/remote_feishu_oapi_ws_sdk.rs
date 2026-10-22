//! Feishu 官方 `oapi-sdk-go/ws` 的 Rust 兼容层。
//!
//! 本模块只翻译当前远控入站所需的 WebSocket SDK 子集：bootstrap endpoint、
//! pbbp2 frame 编解码、ping frame、data frame payload 派发与响应帧构造。
//! 不引入 Feishu `service/*` 全量 API 生成模型，避免远控 gateway 被非必要 SDK 面污染。

use std::{future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use axon_domain::{AxonError, AxonResult};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use futures_util::{FutureExt as _, SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    sync::Mutex,
    time::{MissedTickBehavior, sleep, timeout},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use url::Url;

use crate::{
    remote_feishu_oapi_ws_loop::{RemoteFeishuOapiWsLoopControl, RemoteFeishuOapiWsLoopReport},
    remote_feishu_oapi_ws_ping::feishu_oapi_ws_ping_interval,
};

pub use crate::remote_feishu_oapi_ws_frame_codec::{decode_frame, encode_frame};
pub use crate::remote_feishu_oapi_ws_reconnect::{
    RemoteFeishuOapiWsClientConfig, RemoteFeishuOapiWsReconnectPolicy,
};

/// 官方 WS bootstrap endpoint。
pub const REMOTE_FEISHU_OAPI_WS_ENDPOINT_PATH: &str = "/callback/ws/endpoint";
/// 官方默认 Feishu OpenAPI 域名。
pub const REMOTE_FEISHU_OAPI_DEFAULT_DOMAIN: &str = "https://open.feishu.cn";
/// WS 连接 URL 中的 device id 参数名。
pub const REMOTE_FEISHU_OAPI_WS_DEVICE_ID_QUERY: &str = "device_id";
/// WS 连接 URL 中的 service id 参数名。
pub const REMOTE_FEISHU_OAPI_WS_SERVICE_ID_QUERY: &str = "service_id";

const HEADER_TYPE: &str = "type";
const HEADER_MESSAGE_ID: &str = "message_id";
const HEADER_TRACE_ID: &str = "trace_id";
const HEADER_SUM: &str = "sum";
const HEADER_SEQ: &str = "seq";
const HEADER_BIZ_RT: &str = "biz_rt";
const MESSAGE_TYPE_EVENT: &str = "event";
const MESSAGE_TYPE_CARD: &str = "card";
const MESSAGE_TYPE_PING: &str = "ping";
const MESSAGE_TYPE_PONG: &str = "pong";
const FRAME_TYPE_CONTROL: i32 = 0;
const FRAME_TYPE_DATA: i32 = 1;
const STATUS_OK: i32 = 0;
const STATUS_SYSTEM_BUSY: i32 = 1;
const STATUS_INTERNAL_ERROR: i32 = 1_000_040_343;
const HTTP_STATUS_OK: i32 = 200;
const HTTP_STATUS_INTERNAL_SERVER_ERROR: i32 = 500;
const WS_BOOTSTRAP_MAX_ATTEMPTS: usize = 3;
const WS_BOOTSTRAP_RETRY_DELAY_MS: u64 = 500;
const WS_CONNECT_TIMEOUT_SECS: u64 = 30;
const WS_READ_IDLE_TIMEOUT_SECS: u64 = 180;
const WS_FRAGMENT_MAX_INFLIGHT_MESSAGES: usize = 64;
const WS_FRAGMENT_MAX_PARTS: usize = 128;
const WS_FRAGMENT_MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const WS_FRAGMENT_MAX_MESSAGE_ID_BYTES: usize = 4 * 1024;
const WS_FRAGMENT_MAX_BUFFERED_MESSAGES: usize = 4;
const WS_FRAGMENT_MAX_BUFFERED_BYTES: usize =
    WS_FRAGMENT_MAX_MESSAGE_BYTES * WS_FRAGMENT_MAX_BUFFERED_MESSAGES;
const FEISHU_WS_BOOTSTRAP_MAX_BODY_BYTES: usize = 256 * 1024;

fn feishu_oapi_ws_transport_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(WS_FRAGMENT_MAX_MESSAGE_BYTES))
        .max_frame_size(Some(WS_FRAGMENT_MAX_MESSAGE_BYTES))
}

/// Feishu OAPI WS SDK 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuOapiWsSdkConfig {
    /// App ID。
    pub app_id: String,
    /// App Secret。
    pub app_secret: String,
    /// Feishu OpenAPI 域名。
    pub domain: String,
    /// SDK source 标识。
    pub source: String,
    /// 单次 HTTP 请求超时。
    pub request_timeout: Duration,
}

impl RemoteFeishuOapiWsSdkConfig {
    /// 创建默认配置。
    #[must_use]
    pub fn new(app_id: String, app_secret: String) -> Self {
        Self {
            app_id,
            app_secret,
            domain: REMOTE_FEISHU_OAPI_DEFAULT_DOMAIN.to_owned(),
            source: "axon-feishu-oapi-rust".to_owned(),
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// WS bootstrap 请求体，对齐 Go SDK `BootstrapRequest`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteFeishuOapiWsBootstrapRequest {
    /// App ID。
    #[serde(rename = "AppID")]
    pub app_id: String,
    /// App Secret。
    pub app_secret: String,
    /// JWT client assertion，当前 Axon 远控链路暂不启用。
    pub client_assertion: String,
}

/// WS bootstrap 响应，对齐 Go SDK `EndpointResp`。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemoteFeishuOapiWsEndpointResponse {
    /// 业务 code。
    pub code: i32,
    /// 业务 message。
    pub msg: String,
    /// endpoint 数据。
    pub data: Option<RemoteFeishuOapiWsEndpoint>,
}

/// WS endpoint 数据。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemoteFeishuOapiWsEndpoint {
    /// WebSocket 连接 URL。
    #[serde(rename = "URL")]
    pub url: String,
    /// 服务端下发的 client config。
    #[serde(rename = "ClientConfig")]
    pub client_config: Option<RemoteFeishuOapiWsClientConfig>,
}

/// pbbp2 header。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemoteFeishuOapiWsHeader {
    /// Header key。
    pub key: String,
    /// Header value。
    pub value: String,
}

/// pbbp2 frame。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemoteFeishuOapiWsFrame {
    /// SeqID。
    pub seq_id: u64,
    /// LogID。
    pub log_id: u64,
    /// service。
    pub service: i32,
    /// method。
    pub method: i32,
    /// headers。
    pub headers: Vec<RemoteFeishuOapiWsHeader>,
    /// payload encoding。
    pub payload_encoding: String,
    /// payload type。
    pub payload_type: String,
    /// payload bytes。
    pub payload: Vec<u8>,
    /// new LogID。
    pub log_id_new: String,
}

/// WS data frame 入站 payload。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuOapiWsIncomingPayload {
    /// message kind: event 或 card。
    pub kind: String,
    /// message id。
    pub message_id: String,
    /// trace id。
    pub trace_id: String,
    /// payload JSON 字节。
    pub payload: Vec<u8>,
}

/// 单帧处理报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuOapiWsFrameHandleReport {
    /// 入站 payload。
    pub incoming: Option<RemoteFeishuOapiWsIncomingPayload>,
    /// 需要回写到 Feishu WS 的 response frame bytes。
    pub response_frame: Option<Vec<u8>>,
    /// 更新后的 client config。
    pub client_config: Option<RemoteFeishuOapiWsClientConfig>,
    /// 上层 callback 请求在响应当前帧后停止 loop。
    pub stop_requested: bool,
}

/// Feishu OAPI WS SDK runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuOapiWsSdkRuntime;

#[path = "remote_feishu_oapi_ws_fragments.rs"]
mod fragments;
#[path = "remote_feishu_oapi_ws_sdk_runtime.rs"]
mod runtime;

use fragments::{FragmentBuffer, combine_fragmented_frame};

impl RemoteFeishuOapiWsSdkRuntime {
    /// 生成 bootstrap URL。
    #[must_use]
    pub fn bootstrap_url(domain: &str) -> String {
        format!(
            "{}{}",
            domain.trim_end_matches('/'),
            REMOTE_FEISHU_OAPI_WS_ENDPOINT_PATH
        )
    }

    /// 生成 bootstrap 请求体。
    ///
    /// # Errors
    ///
    /// 当 app_id 或 app_secret 为空时返回错误。
    pub fn bootstrap_request(
        config: &RemoteFeishuOapiWsSdkConfig,
    ) -> AxonResult<RemoteFeishuOapiWsBootstrapRequest> {
        if config.app_id.trim().is_empty() || config.app_secret.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu OAPI WS SDK 缺少 app_id 或 app_secret",
            ));
        }
        Ok(RemoteFeishuOapiWsBootstrapRequest {
            app_id: config.app_id.clone(),
            app_secret: config.app_secret.clone(),
            client_assertion: String::new(),
        })
    }

    /// 请求 Feishu bootstrap endpoint。
    ///
    /// # Errors
    ///
    /// 当 HTTP 请求、响应 JSON 或业务 code 无效时返回错误。
    pub async fn bootstrap_endpoint(
        config: &RemoteFeishuOapiWsSdkConfig,
    ) -> AxonResult<RemoteFeishuOapiWsEndpoint> {
        for attempt in 1..=WS_BOOTSTRAP_MAX_ATTEMPTS {
            axon_logger::debug(&format!(
                "Feishu WS bootstrap 开始: attempt={attempt}/{WS_BOOTSTRAP_MAX_ATTEMPTS}, domain={}",
                config.domain
            ));
            match Self::bootstrap_endpoint_once(config).await {
                Ok(endpoint) => {
                    let host = Url::parse(&endpoint.url)
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".to_owned());
                    axon_logger::debug(&format!(
                        "Feishu WS bootstrap 成功: attempt={attempt}, host={host}"
                    ));
                    return Ok(endpoint);
                }
                Err(error) if attempt < WS_BOOTSTRAP_MAX_ATTEMPTS => {
                    sleep(Duration::from_millis(
                        WS_BOOTSTRAP_RETRY_DELAY_MS.saturating_mul(attempt as u64),
                    ))
                    .await;
                    axon_logger::warning(&format!(
                        "Feishu WS bootstrap 失败，将重试: attempt={attempt}, error={error:?}"
                    ));
                }
                Err(error) => {
                    axon_logger::error(&format!(
                        "Feishu WS bootstrap 最终失败: attempts={WS_BOOTSTRAP_MAX_ATTEMPTS}, error={error:?}"
                    ));
                    return Err(error);
                }
            }
        }
        unreachable!("bootstrap retry loop always returns")
    }

    async fn bootstrap_endpoint_once(
        config: &RemoteFeishuOapiWsSdkConfig,
    ) -> AxonResult<RemoteFeishuOapiWsEndpoint> {
        let request = Self::bootstrap_request(config)?;
        let request_body = serde_json::to_vec(&request)
            .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS endpoint 请求 JSON 无效"))?;
        ensure_rustls_crypto_provider();
        let response = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(|error| {
                AxonError::provider_transport(
                    format!("Feishu OAPI WS HTTP client 创建失败: {error}"),
                    None,
                )
            })?
            .post(Self::bootstrap_url(&config.domain))
            .header("locale", "zh")
            .header("Content-Type", "application/json")
            .header("User-Agent", &config.source)
            .body(request_body)
            .send()
            .await
            .map_err(|error| {
                AxonError::provider_transport(
                    format!("Feishu OAPI WS endpoint 请求失败: {error}"),
                    None,
                )
            })?;
        if !response.status().is_success() {
            let http_status = Some(response.status().as_u16());
            let body = read_bounded_response_body(response, FEISHU_WS_BOOTSTRAP_MAX_BODY_BYTES)
                .await
                .unwrap_or_default();
            return Err(AxonError::provider_transport(
                feishu_oapi_ws_endpoint_error_message(&String::from_utf8_lossy(&body)),
                http_status,
            ));
        }
        let body_bytes =
            read_bounded_response_body(response, FEISHU_WS_BOOTSTRAP_MAX_BODY_BYTES).await?;
        let body = serde_json::from_slice::<RemoteFeishuOapiWsEndpointResponse>(&body_bytes)
            .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS endpoint JSON 无效"))?;
        Self::endpoint_from_response(body)
    }

    /// 从 bootstrap response 中解析 endpoint。
    ///
    /// # Errors
    ///
    /// 当业务 code 非 OK 或 endpoint URL 为空时返回错误。
    pub fn endpoint_from_response(
        response: RemoteFeishuOapiWsEndpointResponse,
    ) -> AxonResult<RemoteFeishuOapiWsEndpoint> {
        match response.code {
            STATUS_OK => {}
            STATUS_SYSTEM_BUSY | STATUS_INTERNAL_ERROR => {
                return Err(AxonError::runtime_failure(
                    "Feishu OAPI WS endpoint 服务端繁忙",
                ));
            }
            _ => {
                return Err(AxonError::runtime_failure(
                    "Feishu OAPI WS endpoint 鉴权失败",
                ));
            }
        }
        let endpoint = response
            .data
            .ok_or(AxonError::runtime_failure("Feishu OAPI WS endpoint 为空"))?;
        if endpoint.url.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu OAPI WS endpoint URL 为空",
            ));
        }
        Ok(endpoint)
    }

    /// 从 endpoint URL 中读取 service_id。
    ///
    /// # Errors
    ///
    /// 当 URL 无效或 service_id 不存在时返回错误。
    pub fn service_id_from_endpoint(url: &str) -> AxonResult<i32> {
        let parsed = Url::parse(url)
            .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS endpoint URL 无效"))?;
        let service_id = parsed
            .query_pairs()
            .find(|(key, _)| key == REMOTE_FEISHU_OAPI_WS_SERVICE_ID_QUERY)
            .map(|(_, value)| value.to_string())
            .ok_or(AxonError::runtime_failure(
                "Feishu OAPI WS endpoint 缺少 service_id",
            ))?;
        service_id
            .parse::<i32>()
            .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS service_id 无效"))
    }

    /// 构造 ping frame。
    #[must_use]
    pub fn ping_frame(service_id: i32) -> RemoteFeishuOapiWsFrame {
        RemoteFeishuOapiWsFrame {
            service: service_id,
            method: FRAME_TYPE_CONTROL,
            headers: vec![RemoteFeishuOapiWsHeader {
                key: HEADER_TYPE.to_owned(),
                value: MESSAGE_TYPE_PING.to_owned(),
            }],
            ..RemoteFeishuOapiWsFrame::default()
        }
    }

    pub(crate) fn encode_ping_frame(service_id: i32) -> AxonResult<Vec<u8>> {
        encode_frame(&Self::ping_frame(service_id))
    }

    /// 处理单个 WS binary frame。
    ///
    /// # Errors
    ///
    /// 当 frame 编解码、payload callback 或响应 frame 编码失败时返回错误。
    pub async fn handle_binary_frame<F, Fut>(
        bytes: &[u8],
        mut on_payload: F,
    ) -> AxonResult<RemoteFeishuOapiWsFrameHandleReport>
    where
        F: FnMut(RemoteFeishuOapiWsIncomingPayload) -> Fut,
        Fut: Future<Output = AxonResult<Option<Value>>>,
    {
        let frame = decode_frame(bytes)?;
        Self::handle_decoded_frame(frame, &mut on_payload).await
    }

    async fn handle_decoded_frame<F, Fut>(
        frame: RemoteFeishuOapiWsFrame,
        on_payload: &mut F,
    ) -> AxonResult<RemoteFeishuOapiWsFrameHandleReport>
    where
        F: FnMut(RemoteFeishuOapiWsIncomingPayload) -> Fut,
        Fut: Future<Output = AxonResult<Option<Value>>>,
    {
        match frame.method {
            FRAME_TYPE_CONTROL => handle_control_frame(&frame),
            FRAME_TYPE_DATA => {
                let incoming = incoming_payload(&frame)?;
                let callback_response = on_payload(incoming.clone()).await;
                let stop_requested = callback_response
                    .as_ref()
                    .ok()
                    .is_some_and(|value| value.as_ref().is_some_and(callback_requests_stop));
                let response_frame = response_frame(frame, callback_response)?;
                Ok(RemoteFeishuOapiWsFrameHandleReport {
                    incoming: Some(incoming),
                    response_frame: Some(encode_frame(&response_frame)?),
                    client_config: None,
                    stop_requested,
                })
            }
            _ => Ok(RemoteFeishuOapiWsFrameHandleReport {
                incoming: None,
                response_frame: None,
                client_config: None,
                stop_requested: false,
            }),
        }
    }

    /// 连接 Feishu WS 并循环读取 binary frame。
    ///
    /// # Errors
    ///
    /// 当 bootstrap、WS 连接、frame 处理或写响应失败时返回错误。
    pub async fn run_until_closed<F, Fut>(
        config: RemoteFeishuOapiWsSdkConfig,
        mut on_payload: F,
    ) -> AxonResult<()>
    where
        F: FnMut(RemoteFeishuOapiWsIncomingPayload) -> Fut,
        Fut: Future<Output = AxonResult<Option<Value>>>,
    {
        Self::run_until_closed_with_control(
            config,
            RemoteFeishuOapiWsLoopControl::default(),
            &mut on_payload,
        )
        .await
        .map(|_| ())
    }
}

async fn send_ws_message(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    write_lock: &Mutex<()>,
    message: Message,
) -> AxonResult<()> {
    let _guard = write_lock.lock().await;
    socket.send(message).await.map_err(|error| {
        AxonError::provider_transport(format!("Feishu OAPI WS 写入失败: {error}"), None)
    })
}

fn close_frame_reason(
    frame: Option<&tokio_tungstenite::tungstenite::protocol::CloseFrame>,
) -> String {
    frame.map_or_else(
        || "无 close code/reason".to_owned(),
        |frame| format!("code={:?}, reason={}", frame.code, frame.reason),
    )
}

fn handle_control_frame(
    frame: &RemoteFeishuOapiWsFrame,
) -> AxonResult<RemoteFeishuOapiWsFrameHandleReport> {
    if header_value(&frame.headers, HEADER_TYPE).as_deref() != Some(MESSAGE_TYPE_PONG)
        || frame.payload.is_empty()
    {
        return Ok(RemoteFeishuOapiWsFrameHandleReport {
            incoming: None,
            response_frame: None,
            client_config: None,
            stop_requested: false,
        });
    }
    let config = serde_json::from_slice::<RemoteFeishuOapiWsClientConfig>(&frame.payload)
        .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS pong config JSON 无效"))?;
    Ok(RemoteFeishuOapiWsFrameHandleReport {
        incoming: None,
        response_frame: None,
        client_config: Some(config),
        stop_requested: false,
    })
}

fn callback_requests_stop(value: &Value) -> bool {
    value
        .get("axonControl")
        .and_then(|control| control.get("stop"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn incoming_payload(
    frame: &RemoteFeishuOapiWsFrame,
) -> AxonResult<RemoteFeishuOapiWsIncomingPayload> {
    let kind = header_value(&frame.headers, HEADER_TYPE).ok_or(AxonError::runtime_failure(
        "Feishu OAPI WS data frame 缺少 type",
    ))?;
    if kind != MESSAGE_TYPE_EVENT && kind != MESSAGE_TYPE_CARD {
        return Err(AxonError::runtime_failure(
            "Feishu OAPI WS data frame type 不支持",
        ));
    }
    Ok(RemoteFeishuOapiWsIncomingPayload {
        kind,
        message_id: header_value(&frame.headers, HEADER_MESSAGE_ID).unwrap_or_default(),
        trace_id: header_value(&frame.headers, HEADER_TRACE_ID).unwrap_or_default(),
        payload: frame.payload.clone(),
    })
}

fn response_frame(
    mut frame: RemoteFeishuOapiWsFrame,
    callback_response: AxonResult<Option<Value>>,
) -> AxonResult<RemoteFeishuOapiWsFrame> {
    let started_at_ms = now_ms();
    let mut response = RemoteFeishuOapiWsResponse {
        code: HTTP_STATUS_OK,
        headers: serde_json::Map::new(),
        data: Vec::new(),
    };
    match callback_response {
        Ok(Some(value)) => {
            response.data = serde_json::to_vec(&value)
                .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS 响应 JSON 无效"))?;
        }
        Ok(None) => {}
        Err(_) => response.code = HTTP_STATUS_INTERNAL_SERVER_ERROR,
    }
    frame.headers.push(RemoteFeishuOapiWsHeader {
        key: HEADER_BIZ_RT.to_owned(),
        value: now_ms().saturating_sub(started_at_ms).to_string(),
    });
    frame.payload = serde_json::to_vec(&response)
        .map_err(|_| AxonError::runtime_failure("Feishu OAPI WS response 序列化失败"))?;
    Ok(frame)
}

fn feishu_oapi_ws_endpoint_error_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct ErrorBody {
        msg: Option<String>,
    }

    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "Feishu OAPI WS endpoint HTTP 状态无效".to_owned();
    }
    serde_json::from_str::<ErrorBody>(trimmed)
        .ok()
        .and_then(|error| error.msg)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| trimmed.chars().take(200).collect())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RemoteFeishuOapiWsResponse {
    code: i32,
    headers: serde_json::Map<String, Value>,
    data: Vec<u8>,
}

fn header_value(headers: &[RemoteFeishuOapiWsHeader], key: &str) -> Option<String> {
    headers
        .iter()
        .find(|header| header.key == key)
        .map(|header| header.value.clone())
}

fn header_usize(headers: &[RemoteFeishuOapiWsHeader], key: &str) -> Option<usize> {
    header_value(headers, key)?.parse::<usize>().ok()
}

fn now_ms() -> u64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

async fn read_bounded_response_body(
    response: reqwest::Response,
    max_bytes: usize,
) -> AxonResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(AxonError::provider_transport(
            "Feishu OAPI WS endpoint 响应体超过大小上限".to_owned(),
            Some(413),
        ));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(|error| {
            AxonError::provider_transport(
                format!("Feishu OAPI WS endpoint 读取失败: {error}"),
                None,
            )
        })?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(AxonError::provider_transport(
                "Feishu OAPI WS endpoint 响应体超过大小上限".to_owned(),
                Some(413),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
#[path = "remote_feishu_oapi_ws_sdk_tests.rs"]
mod tests;
