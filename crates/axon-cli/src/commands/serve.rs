use std::{
    fs,
    io::{Read as _, Write as _},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::Path,
    time::{Duration, Instant},
};

use axon_config::{AxonConfig, ConfigLoader};
use serde::Serialize;
use serde_json::json;

use crate::workbench_http_adapter::{
    WORKBENCH_PROTOCOL_VERSION, lanes_payload, workspaces_payload,
};
use crate::{
    CliRunResult, OutputFormat, ServeCommand,
    management_commands::{cli_config_error, cli_error, json_result, resolved_config_path},
    runtime_paths::runtime_axon_root_path,
    session_commands::{bounded_session_limit, session_summary_views_for_root},
};

const SERVE_READ_TIMEOUT: Duration = Duration::from_secs(5);
const SERVE_MAX_REQUEST_BYTES: usize = 16 * 1024;
const SERVE_MAX_TOKEN_BYTES: usize = 4096;
const SERVE_MIN_TOKEN_BYTES: usize = 16;
const HEADER_CONTENT_TYPE_JSON: &str = "content-type: application/json; charset=utf-8";
const AUTH_SCHEME_BEARER: &str = "Bearer ";

pub(crate) fn run_serve(
    command: &ServeCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let loaded = match ConfigLoader::load_from_path(&path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    let auth = match ServeAuth::from_file(command.auth_token_file.as_deref()) {
        Ok(auth) => auth,
        Err(error) => return cli_error(error),
    };
    let bind_addr = match resolve_bind_addr(&command.host, command.port, auth.enabled()) {
        Ok(addr) => addr,
        Err(error) => return cli_error(error),
    };
    let max_requests = match command.max_requests {
        Some(0) => return cli_error("serve max-requests 必须大于 0"),
        value => value,
    };
    let axon_root = match runtime_axon_root_path(&path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    match ServeRuntime::bind(bind_addr, loaded.config, auth, axon_root).and_then(|runtime| {
        let bound_addr = runtime.bound_addr;
        let auth_required = runtime.auth.enabled();
        runtime
            .serve(max_requests)
            .map(|handled_requests| ServeReport {
                status: "serve_stopped",
                bind_addr: bound_addr.to_string(),
                handled_requests,
                auth_required,
                remote_trigger_allowed: false,
                raw_payload_persisted: false,
            })
    }) {
        Ok(report) => render_report(format, &report),
        Err(error) => cli_error(error),
    }
}

struct ServeRuntime {
    listener: TcpListener,
    bound_addr: SocketAddr,
    config: AxonConfig,
    auth: ServeAuth,
    axon_root: std::path::PathBuf,
}

impl ServeRuntime {
    fn bind(
        bind_addr: SocketAddr,
        config: AxonConfig,
        auth: ServeAuth,
        axon_root: std::path::PathBuf,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind(bind_addr)
            .map_err(|error| format!("无法绑定 headless server 地址 {bind_addr}: {error}"))?;
        let bound_addr = listener
            .local_addr()
            .map_err(|error| format!("无法读取 headless server 监听地址: {error}"))?;
        Ok(Self {
            listener,
            bound_addr,
            config,
            auth,
            axon_root,
        })
    }

    fn serve(self, max_requests: Option<usize>) -> Result<usize, String> {
        let mut handled_requests = 0usize;
        loop {
            if max_requests.is_some_and(|limit| handled_requests >= limit) {
                return Ok(handled_requests);
            }
            let (mut stream, _) = self
                .listener
                .accept()
                .map_err(|error| format!("headless server 接收连接失败: {error}"))?;
            stream
                .set_read_timeout(Some(SERVE_READ_TIMEOUT))
                .map_err(|error| format!("无法设置请求读取超时: {error}"))?;
            let response = self.handle_stream(&mut stream);
            write_response(&mut stream, &response)?;
            handled_requests += 1;
        }
    }

    fn handle_stream(&self, stream: &mut TcpStream) -> HttpResponse {
        let request = match read_request(stream) {
            Ok(request) => request,
            Err(error) => return json_response(400, &json!({ "error": error })),
        };
        if request.method != "GET" {
            return json_response(405, &json!({ "error": "method_not_allowed" }));
        }
        let path = request
            .path
            .split('?')
            .next()
            .unwrap_or(request.path.as_str());
        if path != "/v1/health" && !self.auth.authorized(&request) {
            return json_response(401, &json!({ "error": "unauthorized" }));
        }
        match path {
            "/v1/health" => json_response(200, &self.health_body()),
            "/v1/ready" => json_response(200, &self.ready_body()),
            "/v1/models" => json_response(200, &self.models_body()),
            "/v1/workspaces" => match self.workspaces_body() {
                Ok(body) => json_response(200, &body),
                Err(error) => json_response(400, &json!({ "error": error })),
            },
            "/v1/lanes" => match self.lanes_body(&request) {
                Ok(body) => json_response(200, &body),
                Err(error) => json_response(400, &json!({ "error": error })),
            },
            "/v1/sessions" => match self.sessions_body(&request) {
                Ok(body) => json_response(200, &body),
                Err(error) => json_response(400, &json!({ "error": error })),
            },
            _ => json_response(404, &json!({ "error": "not_found" })),
        }
    }

    fn health_body(&self) -> serde_json::Value {
        json!({
            "status": "ok",
            "service": "axon-serve",
            "version": env!("CARGO_PKG_VERSION"),
            "authRequired": self.auth.enabled(),
            // 协议能力声明：客户端据此判断工作区与车道路由是否可用，
            // 而不是探测失败后再猜是「没有数据」还是「服务端不支持」。
            "capabilities": {
                "protocolVersion": WORKBENCH_PROTOCOL_VERSION,
                "workspaces": true,
                "lanes": true,
            },
            "remoteTriggerAllowed": false,
            "remoteStdinAllowed": false,
            "rawPayloadPersisted": false,
        })
    }

    /// 工作区注册表视图。
    fn workspaces_body(&self) -> Result<serde_json::Value, String> {
        workspaces_payload(&self.axon_root)
    }

    /// 车道视图；带 `workspace` 查询参数时只返回该工作区下的车道。
    fn lanes_body(&self, request: &HttpRequest) -> Result<serde_json::Value, String> {
        let filter = query_value(&request.path, "workspace")?;
        lanes_payload(&self.axon_root, filter.as_deref())
    }

    fn ready_body(&self) -> serde_json::Value {
        json!({
            "status": "ready",
            "configuredProviders": self.config.providers.len(),
            "configuredModels": self.config.models.len(),
            "remoteTriggerAllowed": false,
        })
    }

    fn models_body(&self) -> serde_json::Value {
        let models = self
            .config
            .models
            .iter()
            .filter(|model| self.config.model_is_enabled(model))
            .map(|model| {
                json!({
                    "id": model.id,
                    "provider": model.provider,
                    "supportsVision": model.supports_vision,
                    "supportsEmbedding": model.supports_embedding,
                    "contextWindow": model.context_window,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "models": models,
            "remoteTriggerAllowed": false,
            "providerRawPayloadExposed": false,
        })
    }

    fn sessions_body(&self, request: &HttpRequest) -> Result<serde_json::Value, String> {
        let limit = bounded_session_limit(query_limit(&request.path)?)
            .map_err(std::borrow::ToOwned::to_owned)?;
        let (total_records, sessions) = session_summary_views_for_root(&self.axon_root, limit)?;
        Ok(json!({
            "status": "session_ready",
            "limit": limit,
            "totalRecords": total_records,
            "sessions": sessions,
            "sessionRuntimePending": false,
            "postSessionAllowed": false,
            "sseAllowed": false,
            "remoteTriggerAllowed": false,
            "rawTranscriptExposed": false,
            "providerRawPayloadExposed": false,
        }))
    }
}

#[derive(Debug, Clone)]
struct ServeAuth {
    token: Option<String>,
}

impl ServeAuth {
    fn from_file(path: Option<&Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self { token: None });
        };
        let token = fs::read_to_string(path)
            .map_err(|_| "无法读取 serve auth token 文件".to_owned())?
            .trim()
            .to_owned();
        if token.len() < SERVE_MIN_TOKEN_BYTES || token.len() > SERVE_MAX_TOKEN_BYTES {
            return Err("serve auth token 长度必须在 16..=4096 字节范围内".to_owned());
        }
        Ok(Self { token: Some(token) })
    }

    fn enabled(&self) -> bool {
        self.token.is_some()
    }

    fn authorized(&self, request: &HttpRequest) -> bool {
        let Some(token) = self.token.as_deref() else {
            return true;
        };
        request
            .headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .is_some_and(|(_, value)| value == &format!("{AUTH_SCHEME_BEARER}{token}"))
    }
}

#[derive(Debug, Clone)]
struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct HttpResponse {
    status: u16,
    body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServeReport {
    status: &'static str,
    bind_addr: String,
    handled_requests: usize,
    auth_required: bool,
    remote_trigger_allowed: bool,
    raw_payload_persisted: bool,
}

fn resolve_bind_addr(host: &str, port: u16, auth_enabled: bool) -> Result<SocketAddr, String> {
    let mut addrs = (host, port)
        .to_socket_addrs()
        .map_err(|_| "serve host/port 无法解析".to_owned())?;
    let Some(addr) = addrs.next() else {
        return Err("serve host/port 无法解析".to_owned());
    };
    if !is_loopback(addr.ip()) && !auth_enabled {
        return Err("非 loopback 绑定必须提供 --auth-token-file".to_owned());
    }
    Ok(addr)
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

fn query_limit(path: &str) -> Result<Option<usize>, String> {
    query_value(path, "limit")?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "session limit 必须为数字".to_owned())
        })
        .transpose()
}

fn query_value(path: &str, target_name: &str) -> Result<Option<String>, String> {
    let Some((_, query)) = path.split_once('?') else {
        return Ok(None);
    };
    query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find_map(|(name, value)| (name == target_name).then_some(value))
        .map(percent_decode_query_value)
        .transpose()
}

fn percent_decode_query_value(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                    .map_err(|_| "query 参数编码无效".to_owned())?;
                let byte =
                    u8::from_str_radix(hex, 16).map_err(|_| "query 参数编码无效".to_owned())?;
                decoded.push(byte);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| "query 参数不是 UTF-8".to_owned())
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let start = Instant::now();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    while buffer.len() < SERVE_MAX_REQUEST_BYTES {
        let read = stream
            .read(&mut chunk)
            .map_err(|_| "请求读取失败".to_owned())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if start.elapsed() > SERVE_READ_TIMEOUT {
            return Err("请求读取超时".to_owned());
        }
    }
    if buffer.len() >= SERVE_MAX_REQUEST_BYTES {
        return Err("请求过大".to_owned());
    }
    let text = std::str::from_utf8(&buffer).map_err(|_| "请求不是 UTF-8".to_owned())?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or_else(|| "请求行缺失".to_owned())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or_else(|| "请求 method 缺失".to_owned())?;
    let path = parts.next().ok_or_else(|| "请求 path 缺失".to_owned())?;
    let headers = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();
    Ok(HttpRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        headers,
    })
}

fn write_response(stream: &mut TcpStream, response: &HttpResponse) -> Result<(), String> {
    let status_text = match response.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\n{}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.status,
        status_text,
        HEADER_CONTENT_TYPE_JSON,
        response.body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(response.body.as_bytes()))
        .map_err(|_| "响应写入失败".to_owned())
}

fn json_response(status: u16, body: &serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        body: body.to_string(),
    }
}

fn render_report(format: OutputFormat, report: &ServeReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon serve stopped: {} handled_requests={} remote_trigger_allowed={}\n",
                report.bind_addr, report.handled_requests, report.remote_trigger_allowed
            ),
            stderr: String::new(),
        },
    }
}
