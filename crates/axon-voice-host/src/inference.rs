//! 本地推理：调用 sherpa-onnx 官方命令行完成识别与合成。
//!
//! 不依赖 FFI：运行时目录来自运行态根下的 `voice/runtime/sherpa-onnx/<版本>/bin`，
//! 模型目录来自 `voice/models/<family>/<name>`。所有调用都带超时与临时文件清理。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::audio::wav;

/// 识别模型目录名。
pub const ASR_MODEL: &str = "sherpa-streaming-zh-en";
/// 合成模型目录名。
pub const TTS_MODEL: &str = "vits-zh-fanchen";
/// 识别命令行超时。
const ASR_TIMEOUT_MS: u64 = 30_000;
/// 合成命令行超时。
const TTS_TIMEOUT_MS: u64 = 30_000;

/// 运行时与模型布局。
#[derive(Debug, Clone)]
pub struct Inference {
    /// sherpa-onnx 可执行目录。
    pub bin_dir: PathBuf,
    /// ASR 模型目录。
    pub asr_dir: PathBuf,
    /// TTS 模型目录。
    pub tts_dir: PathBuf,
    /// 临时目录。
    pub temp_dir: PathBuf,
}

impl Inference {
    /// 从模型根解析布局；返回 `None` 表示运行时或模型不齐。
    #[must_use]
    pub fn discover(model_root: &Path) -> Option<Self> {
        let bin_dir = runtime_bin_dir(model_root)?;
        let asr_dir = model_root.join("asr").join(ASR_MODEL);
        let tts_dir = model_root.join("tts").join(TTS_MODEL);
        if !asr_dir.is_dir() || !tts_dir.is_dir() {
            return None;
        }
        let temp_dir = std::env::temp_dir().join(format!("axon-voice-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).ok()?;
        Some(Self {
            bin_dir,
            asr_dir,
            tts_dir,
            temp_dir,
        })
    }

    /// 预热探针：识别 0.5 s 静音，返回实测 RTF。
    ///
    /// # Errors
    ///
    /// 运行时缺失、命令失败或结果不可解析。
    pub fn warm_probe(&self) -> Result<f64, String> {
        let silence = vec![0i16; 8_000];
        let started = Instant::now();
        let text = self.transcribe(&silence)?;
        let elapsed = started.elapsed().as_secs_f64();
        if !text.is_empty() && text.chars().count() > 8 {
            return Err("静音探针返回了意外文本".to_owned());
        }
        Ok(elapsed / 0.5)
    }

    /// 识别一段 16 kHz 单声道 PCM。
    ///
    /// # Errors
    ///
    /// 写临时文件、执行命令或解析结果失败。
    pub fn transcribe(&self, samples: &[i16]) -> Result<String, String> {
        let wav_path = self.temp_dir.join("asr-input.wav");
        write_wav(&wav_path, samples)?;
        let output = run(
            &self.bin_dir.join(exe("sherpa-onnx")),
            &[
                format!("--tokens={}", self.asr_dir.join("tokens.txt").display()),
                format!(
                    "--encoder={}",
                    self.asr_dir.join("encoder-epoch-99-avg-1.onnx").display()
                ),
                format!(
                    "--decoder={}",
                    self.asr_dir.join("decoder-epoch-99-avg-1.onnx").display()
                ),
                format!(
                    "--joiner={}",
                    self.asr_dir.join("joiner-epoch-99-avg-1.onnx").display()
                ),
                "--num-threads=2".to_owned(),
                wav_path.display().to_string(),
            ],
            ASR_TIMEOUT_MS,
        )?;
        let _ = std::fs::remove_file(&wav_path);
        parse_transcript(&output)
    }

    /// 合成一段文本，返回 16 kHz 单声道 PCM。
    ///
    /// # Errors
    ///
    /// 写临时文件、执行命令或解析 WAV 失败。
    pub fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        self.synthesize_with(text, 0, 1.0)
    }

    /// 按音色参数合成：speaker 走多说话人编号，rate 走语速。
    ///
    /// # Errors
    ///
    /// 写临时文件、执行命令或解析 WAV 失败。
    pub fn synthesize_with(&self, text: &str, speaker: u32, rate: f32) -> Result<Vec<i16>, String> {
        let wav_path = self.temp_dir.join("tts-output.wav");
        let _ = std::fs::remove_file(&wav_path);
        let output = run(
            &self.bin_dir.join(exe("sherpa-onnx-offline-tts")),
            &[
                format!(
                    "--vits-model={}",
                    self.tts_dir.join("vits-zh-hf-fanchen-C.onnx").display()
                ),
                format!(
                    "--vits-lexicon={}",
                    self.tts_dir.join("lexicon.txt").display()
                ),
                format!(
                    "--vits-tokens={}",
                    self.tts_dir.join("tokens.txt").display()
                ),
                format!(
                    "--tts-rule-fsts={},{}",
                    self.tts_dir.join("date.fst").display(),
                    self.tts_dir.join("number.fst").display()
                ),
                format!("--output-filename={}", wav_path.display()),
                format!("--sid={speaker}"),
                format!("--speed={rate}"),
                text.to_owned(),
            ],
            TTS_TIMEOUT_MS,
        );
        if let Err(error) = output {
            let _ = std::fs::remove_file(&wav_path);
            return Err(error);
        }
        let bytes = std::fs::read(&wav_path).map_err(|error| error.to_string())?;
        let _ = std::fs::remove_file(&wav_path);
        let clip = wav::parse(&bytes).map_err(|error| error.to_string())?;
        Ok(clip.samples)
    }
}

fn runtime_bin_dir(model_root: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("AXON_VOICE_RUNTIME_BIN") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Some(path);
        }
    }
    let runtime_root = model_root.parent()?.join("runtime").join("sherpa-onnx");
    let entries = std::fs::read_dir(runtime_root).ok()?;
    let mut versions: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    versions.sort();
    versions.pop().map(|dir| dir.join("bin"))
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn run(binary: &Path, args: &[String], timeout_ms: u64) -> Result<String, String> {
    if !binary.is_file() {
        return Err(format!("缺少可执行文件：{}", binary.display()));
    }
    let mut child = Command::new(binary)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动失败：{error}"))?;
    let stdout = child.stdout.take().ok_or("缺少 stdout")?;
    let stderr = child.stderr.take().ok_or("缺少 stderr")?;
    let out_handle = std::thread::spawn(move || read_all(stdout));
    let err_handle = std::thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_handle.join();
                    let _ = err_handle.join();
                    return Err("推理超时".to_owned());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => return Err(format!("等待进程失败：{error}")),
        }
    };
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    if !status.success() {
        let text = String::from_utf8_lossy(&stderr);
        return Err(format!(
            "命令失败（{status}）：{}",
            text.chars().take(200).collect::<String>()
        ));
    }
    // sherpa-onnx 把识别 JSON 写到 stderr，日志写到 stdout，这里合并后解析。
    let mut combined = String::from_utf8_lossy(&stdout).into_owned();
    combined.push('\n');
    combined.push_str(&String::from_utf8_lossy(&stderr));
    Ok(combined)
}

fn read_all(mut reader: impl std::io::Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    buffer
}
fn parse_transcript(stdout: &str) -> Result<String, String> {
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with('{')
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(text) = value.get("text").and_then(|value| value.as_str())
        {
            return Ok(text.trim().to_owned());
        }
    }
    Err("识别结果不可解析".to_owned())
}

/// 写 16 kHz 单声道 PCM16 WAV。
///
/// # Errors
///
/// 写入失败。
pub fn write_wav(path: &Path, samples: &[i16]) -> Result<(), String> {
    std::fs::write(path, encode_wav(samples)).map_err(|e| format!("写 WAV 失败：{e}"))
}

/// 编码 16 kHz 单声道 PCM16 WAV。
#[must_use]
pub fn encode_wav(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&16_000u32.to_le_bytes());
    bytes.extend_from_slice(&32_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::parse_transcript;

    #[test]
    fn parses_json_line() {
        let stdout = "log line\n{\"text\": \"你好世界\", \"tokens\": []}\n";
        assert_eq!(parse_transcript(stdout).expect("应解析"), "你好世界");
    }

    #[test]
    fn rejects_missing_text() {
        assert!(parse_transcript("no json here").is_err());
    }
}
