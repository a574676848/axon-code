//! 桌面宿主的本地调试桥。
//!
//! 用途：让浏览器中的真实 Web 调试（自动化验证、样式核对）能够访问与桌面
//! 窗口完全相同的后端能力，而不引入任何伪造数据。桥接层只是把同一批
//! Rust 函数原样暴露为 HTTP 接口，读写对象依旧是真实的 `~/.axon` 数据。
//!
//! 仅在显式传入 `--serve <端口> [静态目录]` 时启动；正常启动不监听任何端口。
//! 只绑定回环地址，不对外网暴露。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::conversation;
use crate::conversation::wire::StreamEvent;
use crate::dev_server_dispatch::dispatch;

/// 单次请求允许的最大请求体字节数，防止异常客户端占满内存。
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// 会话事件流的空转保活间隔。
const EVENT_STREAM_KEEPALIVE: Duration = Duration::from_secs(15);
/// 会话事件流路径；车道流与会话流共用这一条 SSE。
const EVENT_STREAM_PATH: &str = "/events";
/// 附件预览路径前缀。
const ATTACHMENT_PATH_PREFIX: &str = "/attachment/";

/// 读取请求体。
fn read_body(reader: &mut BufReader<TcpStream>, length: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = vec![0_u8; length.min(MAX_BODY_BYTES)];
    reader.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// 写入一个 JSON 响应。
fn write_json(stream: &mut TcpStream, status: &str, payload: &Value) -> std::io::Result<()> {
    let body = payload.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Allow-Methods: POST, OPTIONS, GET\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

/// 写入一个静态文件响应。
fn write_file(stream: &mut TcpStream, path: &std::path::Path) -> std::io::Result<()> {
    let body = std::fs::read(path)?;
    let mime = match path.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        // `.mjs` 必须与 `.js` 同一种类型：模块 Worker 的获取会做严格的 MIME 检查，
        // 落到 application/octet-stream 时浏览器直接拒绝，表现为「worker 起不来」。
        Some("mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)
}

/// 会话事件流：把订阅器上收到的帧以 SSE 推给浏览器。
///
/// 浏览器 WebSocket 无法携带鉴权握手头，因此事件推送由 Rust 侧持有连接、
/// 浏览器只消费这一条本地流。转发的是同一批帧，不额外加工；车道流与会话流
/// 共用这一条流，帧内自带 `subscribeId`，前端按它分发。
fn serve_event_stream(stream: &mut TcpStream) {
    serve_stream(stream, conversation::stream::shared().subscribe_events());
}

fn serve_stream(stream: &mut TcpStream, receiver: mpsc::Receiver<StreamEvent>) {
    let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n";
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    let _ = stream.flush();
    loop {
        match receiver.recv_timeout(EVENT_STREAM_KEEPALIVE) {
            Ok(event) => {
                let payload = serde_json::to_string(&event).unwrap_or_default();
                if stream
                    .write_all(format!("data: {payload}\n\n").as_bytes())
                    .is_err()
                {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if stream.write_all(b": keepalive\n\n").is_err() || stream.flush().is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// 附件预览：按已编码的路径返回图片字节。
///
/// 与桌面窗口共用同一条读取路径，因此判据也一致：只认图片后缀与预览体积上限，
/// 不限定图片所在目录。非图片、不存在的路径与超限图片一律 404。
/// 本路由只在显式传 `--serve` 时存在，并只绑定 loopback。
fn serve_attachment(stream: &mut TcpStream, encoded: &str) {
    let decoded = crate::conversation::attachment::percent_decode(encoded)
        .and_then(|path| crate::conversation::attachment::read_preview(&path));
    let (status, content_type, body) = match decoded {
        Ok((bytes, mime)) => ("200 OK", mime, bytes),
        Err(_) => ("404 Not Found", "text/plain; charset=utf-8", Vec::new()),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

/// 解析静态目录中的目标文件，缺失时回落到 `index.html`。
fn resolve_static(root: &std::path::Path, request_path: &str) -> Option<PathBuf> {
    let clean = request_path.split('?').next().unwrap_or("/");
    let relative = clean.trim_start_matches('/');
    let candidate = if relative.is_empty() {
        root.join("index.html")
    } else {
        root.join(relative)
    };
    if candidate.is_file() {
        return Some(candidate);
    }
    let fallback = root.join("index.html");
    fallback.is_file().then_some(fallback)
}

/// 处理一条连接。
fn handle_connection(mut stream: TcpStream, static_root: Option<&PathBuf>) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut content_length = 0_usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    if method == "OPTIONS" {
        let _ = write_json(&mut stream, "204 No Content", &Value::Null);
        return;
    }

    if method == "POST" && target.starts_with("/rpc") {
        let body = read_body(&mut reader, content_length).unwrap_or_default();
        let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let command = parsed
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let args = parsed.get("args").cloned().unwrap_or(json!({}));

        let response = match dispatch(&command, &args) {
            Ok(data) => json!({ "ok": true, "data": data }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        let _ = write_json(&mut stream, "200 OK", &response);
        return;
    }

    if method == "GET" {
        if target.starts_with(EVENT_STREAM_PATH) {
            serve_event_stream(&mut stream);
            return;
        }
        if let Some(rest) = target.strip_prefix(ATTACHMENT_PATH_PREFIX) {
            let encoded = rest.split('?').next().unwrap_or(rest);
            serve_attachment(&mut stream, encoded);
            return;
        }
        if let Some(root) = static_root
            && let Some(file) = resolve_static(root, &target)
        {
            let _ = write_file(&mut stream, &file);
            return;
        }
        let _ = write_json(
            &mut stream,
            "404 Not Found",
            &json!({ "error": "未找到资源" }),
        );
        return;
    }

    let _ = write_json(
        &mut stream,
        "405 Method Not Allowed",
        &json!({ "error": "不支持的方法" }),
    );
}

/// 启动本地调试桥，阻塞直到进程退出。
pub fn serve(port: u16, static_root: Option<String>) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("无法绑定 127.0.0.1:{port}：{error}"))?;
    let root = std::sync::Arc::new(static_root.map(PathBuf::from));
    println!("axon desktop bridge listening on http://127.0.0.1:{port}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let shared = std::sync::Arc::clone(&root);
                std::thread::spawn(move || handle_connection(stream, shared.as_ref().as_ref()));
            }
            Err(error) => eprintln!("bridge connection error: {error}"),
        }
    }
    Ok(())
}
