//! 流式识别：监督 sherpa-onnx 在线识别服务，把管道音频换成 partial / final。
//!
//! 为什么不是一个「每次调用都重新加载模型」的命令行：合同 P-06 要求 RTF ≤ 0.6、
//! `start → warm` ≤ 10 s，而一次性命令行每条语句都要重新加载模型（实测 20 到 30 s）。
//! 这里改为常驻服务：模型只加载一次，音频经 WebSocket 持续喂入，端点由服务端判定，
//! 中间结果按 partial 实时上抛。
//!
//! 进程模型：本模块 spawn 官方 `sherpa-onnx-online-websocket-server`，自己是 WS 客户端；
//! 端口每次从系统取空闲端口，避免与他进程抢占固定端口。

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::inference::Inference;
use crate::model::manifest::Manifest;
use crate::ws::{WsClient, WsError, WsMessage};

/// 等待识别服务开始监听的上限（模型加载实测 2 s 量级，留足余量）。
const LISTEN_TIMEOUT: Duration = Duration::from_secs(30);
/// 连接识别服务的超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 一次 `poll` 的读预算。
const POLL_BUDGET: Duration = Duration::from_millis(4);
/// 端点静音阈值，与 PROTOCOL §2.6 的 `endpoint_silence_ms` 一致。
const ENDPOINT_SILENCE_MS: u64 = 500;
/// 松键补静音时长：比端点阈值多 200 ms，保证服务端一定判出端点。
const FLUSH_SILENCE_MS: u64 = 700;
/// 松键后等待定稿的上限。
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);
/// 预热探针喂入的静音时长。
const PROBE_MS: u64 = 3_000;
/// 服务端在输入结束时会补的尾部时长；作为 RTF 分母的一部分如实计入。
const TAIL_PADDING_MS: u64 = 800;
/// 结束输入信号：服务端收到任意文本帧即认为输入结束。
const DONE_SIGNAL: &str = "Done";
/// 采样率，协议常量。
const SAMPLE_RATE: u32 = 16_000;
/// 16 位采样的字节数。
const SAMPLE_BYTES: usize = 2;
/// 一次喂给服务端的最大音频时长（100 ms，与上行合帧一致）。
const PUSH_CHUNK_MS: u64 = 100;
/// Windows 下不弹控制台窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 识别事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrEvent {
    /// 中间结果。
    Partial {
        /// 句序号。
        utterance_seq: u32,
        /// 文本。
        text: String,
    },
    /// 定稿。
    Final {
        /// 句序号。
        utterance_seq: u32,
        /// 文本。
        text: String,
        /// 本句起点（相对会话开始，毫秒）。
        start_ms: u64,
        /// 端点判定时刻。
        end_ms: u64,
        /// 定稿产出时刻。
        finalize_ms: u64,
    },
}

/// 流式识别服务。
pub struct StreamingAsr {
    child: Child,
    client: WsClient,
    started: Instant,
    utterance_seq: u32,
    last_partial: String,
}

impl StreamingAsr {
    /// 拉起识别服务并连上它。
    ///
    /// # Errors
    ///
    /// 模型清单不齐、服务无法启动或握手失败。
    pub fn start(layout: &Inference) -> Result<Self, String> {
        let files = asr_files(&layout.asr_dir)?;
        let port = free_port()?;
        let mut child = spawn_server(&layout.bin_dir, &files, port)?;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        if let Err(error) = wait_listening(address, &mut child) {
            // 起不来的识别服务必须当场回收，不能留成孤儿继续占着模型内存。
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let client =
            match WsClient::connect(address, &format!("127.0.0.1:{port}"), "/", CONNECT_TIMEOUT) {
                Ok(client) => client,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.to_string());
                }
            };
        Ok(Self {
            child,
            client,
            started: Instant::now(),
            utterance_seq: 1,
            last_partial: String::new(),
        })
    }

    /// 已运行的识别服务端口。
    #[must_use]
    pub fn port(&self) -> u16 {
        self.client.peer_port().unwrap_or_default()
    }

    /// 推入 16 位小端 PCM。
    ///
    /// # Errors
    ///
    /// 采样字节数为奇数、格式非法或写失败。
    pub fn push_audio(&mut self, pcm: &[u8]) -> Result<(), String> {
        if !pcm.len().is_multiple_of(SAMPLE_BYTES) {
            return Err("音频字节数为奇数，无法按 s16le 解释".to_owned());
        }
        let samples: Vec<f32> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0)
            .collect();
        let per_chunk = (SAMPLE_RATE as usize * PUSH_CHUNK_MS as usize) / 1_000;
        for chunk in samples.chunks(per_chunk.max(1)) {
            let mut bytes = Vec::with_capacity(chunk.len() * 4);
            for sample in chunk {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            self.client
                .send_binary(&bytes)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// 读取一批事件；预算内没有新帧时返回空集合。
    ///
    /// # Errors
    ///
    /// 帧格式错误或连接断开。
    pub fn poll(&mut self, budget: Duration) -> Result<Vec<AsrEvent>, String> {
        let mut events = Vec::new();
        self.drain(budget, &mut events)?;
        Ok(events)
    }

    /// 结束当前句：补一段静音触发服务端端点判定，并等到定稿。
    ///
    /// # Errors
    ///
    /// 写失败或连接断开。
    pub fn finish_utterance(&mut self) -> Result<Vec<AsrEvent>, String> {
        let silence_samples = (SAMPLE_RATE as usize * FLUSH_SILENCE_MS as usize) / 1_000;
        let silence = vec![0.0f32; silence_samples];
        let mut bytes = Vec::with_capacity(silence.len() * 4);
        for sample in &silence {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.client
            .send_binary(&bytes)
            .map_err(|error| error.to_string())?;

        let mut events = Vec::new();
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        while Instant::now() < deadline {
            self.drain(POLL_BUDGET, &mut events)?;
            if events
                .iter()
                .any(|event| matches!(event, AsrEvent::Final { .. }))
            {
                return Ok(events);
            }
        }
        Ok(events)
    }

    /// 丢弃当前句的识别状态（取消）：重连即清空服务端流。
    ///
    /// # Errors
    ///
    /// 重连失败。
    pub fn reset(&mut self) -> Result<(), String> {
        let port = self.port();
        if port == 0 {
            return Err("识别服务端口未知".to_owned());
        }
        self.client.close();
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        self.client =
            WsClient::connect(address, &format!("127.0.0.1:{port}"), "/", CONNECT_TIMEOUT)
                .map_err(|error| error.to_string())?;
        // 服务端的流内时间随重连归零，本地基准必须同步重置，
        // 否则下一句的 `start_ms` 会回落到旧基准之下（时间倒流）。
        self.started = Instant::now();
        self.last_partial.clear();
        Ok(())
    }

    /// 实测 RTF：喂一段静音并等到服务端处理完，用时除以音频时长。
    ///
    /// 口径：静音长度 + 服务端 `--end-tail-padding`（尾部补帧）一起算作「处理的音频」，
    /// 因此这个值包含端点收尾成本，偏保守，不会把 RTF 报得比实际更低。
    ///
    /// # Errors
    ///
    /// 写失败、读失败或超时。
    pub fn measure_rtf(&mut self) -> Result<f64, String> {
        let samples = ((SAMPLE_RATE as u64 * PROBE_MS) / 1_000) as usize;
        let silence = vec![0.0f32; samples];
        let mut bytes = Vec::with_capacity(silence.len() * 4);
        for sample in &silence {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let per_chunk = (SAMPLE_RATE as u64 * PUSH_CHUNK_MS / 1_000) as usize;
        let started = Instant::now();
        for chunk in silence.chunks(per_chunk.max(1)) {
            let mut payload = Vec::with_capacity(chunk.len() * 4);
            for sample in chunk {
                payload.extend_from_slice(&sample.to_le_bytes());
            }
            self.client
                .send_binary(&payload)
                .map_err(|error| error.to_string())?;
        }
        // 结束输入信号：服务端处理完会把 `is_eof` 置真，这是唯一可靠的「处理完毕」判据。
        self.client
            .send_text(DONE_SIGNAL)
            .map_err(|error| error.to_string())?;
        let deadline = started + Duration::from_secs(30);
        loop {
            if Instant::now() >= deadline {
                return Err("预热探针超时".to_owned());
            }
            match self.client.read_message(Duration::from_millis(50)) {
                Ok(Some(WsMessage::Text(text))) => {
                    if json_flag(&text, "is_eof") {
                        break;
                    }
                }
                Ok(Some(WsMessage::Binary(_))) | Ok(None) => {}
                Ok(Some(WsMessage::Close)) => return Err("识别服务提前关闭".to_owned()),
                Err(error) => return Err(error.to_string()),
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        let processed = (PROBE_MS + TAIL_PADDING_MS) as f64 / 1_000.0;
        // 探针把输入结束后，服务端流已 closed；重连以恢复可用状态。
        self.reset()?;
        Ok(elapsed / processed)
    }

    /// 关闭识别服务并回收子进程。
    pub fn shutdown(&mut self) {
        self.client.close();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// 会话开始：时间戳基准与句序号都从 `sessionStart` 重新起算（PROTOCOL §2.6）。
    pub fn begin_session(&mut self) {
        self.started = Instant::now();
        self.utterance_seq = 1;
        self.last_partial.clear();
    }

    fn drain(&mut self, budget: Duration, events: &mut Vec<AsrEvent>) -> Result<(), String> {
        let deadline = Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            // 端点时刻取「读到这一帧之前」的时刻：`end_ms` 是端点判定，
            // `finalize_ms` 是解析出定稿之后，两者之差就是定稿处理时间（P-03）。
            let observed_ms = self.started.elapsed().as_millis() as u64;
            let message = self
                .client
                .read_message(remaining.max(Duration::from_millis(1)))
                .map_err(|error| match error {
                    WsError::Closed => "识别服务关闭了连接".to_owned(),
                    other => other.to_string(),
                })?;
            match message {
                Some(WsMessage::Text(text)) => self.handle_json(&text, observed_ms, events),
                Some(WsMessage::Binary(_)) => {}
                Some(WsMessage::Close) => return Err("识别服务关闭了连接".to_owned()),
                None => return Ok(()),
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
        }
    }

    fn handle_json(&mut self, text: &str, observed_ms: u64, events: &mut Vec<AsrEvent>) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return;
        };
        let content = value
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_owned();
        let is_final = value
            .get("is_final")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if is_final {
            if !content.is_empty() {
                let start_seconds = value
                    .get("start_time")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0);
                let finalize_ms = self.started.elapsed().as_millis() as u64;
                let start_ms = (start_seconds * 1_000.0).round() as u64;
                let end_ms = observed_ms.max(start_ms).min(finalize_ms.max(start_ms));
                let finalize_ms = finalize_ms.max(end_ms);
                events.push(AsrEvent::Final {
                    utterance_seq: self.utterance_seq,
                    text: content,
                    start_ms,
                    end_ms,
                    finalize_ms,
                });
                self.utterance_seq += 1;
            }
            self.last_partial.clear();
            return;
        }
        if content.is_empty() || content == self.last_partial {
            return;
        }
        self.last_partial.clone_from(&content);
        events.push(AsrEvent::Partial {
            utterance_seq: self.utterance_seq,
            text: content,
        });
    }
}

impl Drop for StreamingAsr {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 识别服务所需文件（清单驱动，不写死文件名）。
struct AsrFiles {
    tokens: PathBuf,
    encoder: PathBuf,
    decoder: PathBuf,
    joiner: PathBuf,
}

fn asr_files(asr_dir: &Path) -> Result<AsrFiles, String> {
    let manifest_path = asr_dir.join("manifest.json");
    let bytes =
        std::fs::read(&manifest_path).map_err(|error| format!("读取模型清单失败：{error}"))?;
    let manifest = Manifest::parse(&bytes).map_err(|error| format!("模型清单非法：{error:?}"))?;
    let pick = |keyword: &str| -> Option<PathBuf> {
        let mut candidates: Vec<&str> = manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .filter(|path| {
                path.ends_with(".onnx")
                    && path.contains(keyword)
                    && !path.contains('/')
                    && !path.contains('\\')
            })
            .collect();
        // 优先非量化模型：int8 在个别语句上会出现重复字与噪声 token。
        candidates.sort_by_key(|path| path.contains(".int8."));
        candidates
            .first()
            .map(|path| asr_dir.join(path))
            .filter(|path| path.is_file())
    };
    let tokens = asr_dir.join("tokens.txt");
    if !tokens.is_file() {
        return Err("模型缺少 tokens.txt".to_owned());
    }
    let encoder = pick("encoder").ok_or("模型缺少 encoder")?;
    let decoder = pick("decoder").ok_or("模型缺少 decoder")?;
    let joiner = pick("joiner").ok_or("模型缺少 joiner")?;
    Ok(AsrFiles {
        tokens,
        encoder,
        decoder,
        joiner,
    })
}

fn free_port() -> Result<u16, String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|error| format!("申请空闲端口失败：{error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("读取端口失败：{error}"))?
        .port();
    drop(listener);
    Ok(port)
}

fn spawn_server(bin_dir: &Path, files: &AsrFiles, port: u16) -> Result<Child, String> {
    let binary = bin_dir.join(exe("sherpa-onnx-online-websocket-server"));
    if !binary.is_file() {
        return Err(format!("缺少流式识别服务：{}", binary.display()));
    }
    let mut command = Command::new(&binary);
    command
        .arg(format!("--port={port}"))
        .arg(format!("--tokens={}", files.tokens.display()))
        .arg(format!("--encoder={}", files.encoder.display()))
        .arg(format!("--decoder={}", files.decoder.display()))
        .arg(format!("--joiner={}", files.joiner.display()))
        .arg("--num-threads=2")
        .arg("--num-work-threads=2")
        .arg("--max-batch-size=2")
        .arg("--loop-interval-ms=10")
        .arg("--enable-endpoint=true")
        .arg(format!(
            "--rule1-min-trailing-silence={}",
            ENDPOINT_SILENCE_MS as f64 / 1_000.0
        ))
        .arg("--rule1-must-contain-nonsilence=true")
        .arg("--rule2-min-trailing-silence=0.8")
        .arg("--rule3-min-utterance-length=20")
        .arg("--decoding-method=greedy_search")
        // 显式声明尾部补帧，RTF 口径（`TAIL_PADDING_MS`）与实际行为一致。
        .arg(format!(
            "--end-tail-padding={}",
            TAIL_PADDING_MS as f64 / 1_000.0
        ))
        // 服务端默认在当前目录写 log.txt；指向空设备，避免污染仓库与用户目录。
        .arg(format!("--log-file={}", null_device()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map_err(|error| format!("启动流式识别服务失败：{error}"))
}

fn wait_listening(address: SocketAddr, child: &mut Child) -> Result<(), String> {
    let deadline = Instant::now() + LISTEN_TIMEOUT;
    while Instant::now() < deadline {
        match TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionRefused | ErrorKind::TimedOut | ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(format!("连接识别服务失败：{error}")),
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("识别服务提前退出：{status}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("识别服务未在限期内开始监听".to_owned())
}

/// 平台空设备：识别服务的日志写到这里，不落仓库也不落用户目录。
fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// 从识别服务返回的 JSON 里读一个布尔字段。
fn json_flag(text: &str, field: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get(field).and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asr_files_prefers_the_full_precision_model() {
        let manifest = serde_json::json!({
            "family": "asr",
            "name": "probe",
            "version": "1.0.0",
            "runtime": "sherpa-onnx",
            "sampleRate": 16000,
            "languages": ["zh"],
            "license": "Apache-2.0",
            "rtfP50": 0.25,
            "rtfP95": 0.45,
            "ramMb": 420,
            "files": [
                {"path": "tokens.txt", "sha256": "0".repeat(64), "bytes": 1},
                {"path": "encoder-epoch-99-avg-1.int8.onnx", "sha256": "0".repeat(64), "bytes": 1},
                {"path": "encoder-epoch-99-avg-1.onnx", "sha256": "0".repeat(64), "bytes": 1},
                {"path": "decoder-epoch-99-avg-1.onnx", "sha256": "0".repeat(64), "bytes": 1},
                {"path": "joiner-epoch-99-avg-1.onnx", "sha256": "0".repeat(64), "bytes": 1}
            ]
        })
        .to_string();
        let dir = std::env::temp_dir().join(format!("axon-streaming-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时模型目录");
        std::fs::write(dir.join("manifest.json"), manifest).expect("写清单");
        for name in [
            "tokens.txt",
            "encoder-epoch-99-avg-1.int8.onnx",
            "encoder-epoch-99-avg-1.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.onnx",
        ] {
            std::fs::write(dir.join(name), b"probe").expect("写占位文件");
        }
        let files = asr_files(&dir).expect("应解析出模型文件");
        assert!(files.encoder.ends_with("encoder-epoch-99-avg-1.onnx"));
        assert_eq!(
            files.encoder.file_name().and_then(|n| n.to_str()),
            Some("encoder-epoch-99-avg-1.onnx")
        );
        assert!(files.decoder.ends_with("decoder-epoch-99-avg-1.onnx"));
        assert!(files.joiner.ends_with("joiner-epoch-99-avg-1.onnx"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
