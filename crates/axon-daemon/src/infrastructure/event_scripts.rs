//! 外部事件 provider 脚本桥。
//!
//! 仓库内不落任何 provider 业务脚本：每个 provider 在 `~/.axon/events/{provider}/`
//! 下自包含 `parser.py` 与 `sop.md`，Daemon 只按契约 fork 执行、校验并审计。
//! 脚本 stdin/stdout 契约见同目录 `event_script_schema.json`。

use crate::application::{
    MAX_EXTERNAL_EVENT_CORRELATION_KEY_LENGTH, MAX_EXTERNAL_EVENT_ID_LENGTH,
    MAX_EXTERNAL_EVENT_TITLE_LENGTH,
};
use axon_process::{
    configure_managed_process_group, non_interactive_command, terminate_managed_process_tree,
    track_managed_process,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

/// provider 目录名允许的最大字符数。
pub const EVENT_PROVIDER_NAME_MAX_LENGTH: usize = 64;
/// provider 解析脚本文件名。
pub const EVENT_PROVIDER_PARSER_FILE_NAME: &str = "parser.py";
/// provider SOP 文档文件名。
pub const EVENT_PROVIDER_SOP_FILE_NAME: &str = "sop.md";
/// 解析脚本输出契约字段，与 `event_script_schema.json` 保持同一事实来源。
pub const EVENT_SCRIPT_OUTPUT_FIELDS: [&str; 4] =
    ["event_id", "correlation_key", "title", "content"];
/// 解析脚本输出契约文件正文。
pub const EVENT_SCRIPT_SCHEMA_JSON: &str = include_str!("event_script_schema.json");
/// 解析脚本硬超时。
pub const EVENT_SCRIPT_TIMEOUT: Duration = Duration::from_secs(2);
/// 解释 provider 解析脚本的程序。
const EVENT_SCRIPT_INTERPRETER: &str = "python";
/// 解释器强制 UTF-8，避免中文 Windows 下按本地代码页输出而与契约冲突。
const EVENT_SCRIPT_INTERPRETER_UTF8_ENV: (&str, &str) = ("PYTHONUTF8", "1");
/// provider 脚本允许继承的环境变量；Provider 凭据一律不透传。
const EVENT_SCRIPT_ENV_ALLOWLIST: [&str; 9] = [
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "COMSPEC",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
];
/// 熔断轮询间隔。
const EVENT_SCRIPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// 审计摘要保留的 stderr 字符数。
const EVENT_SCRIPT_STDERR_AUDIT_CHARS: usize = 512;

/// provider 解析脚本输出的事件。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventScriptEvent {
    /// provider 命名空间化的稳定事件标识。
    pub event_id: String,
    /// 业务关联键；缺省时回落到 `event_id`。
    #[serde(default)]
    pub correlation_key: Option<String>,
    /// 事件简报标题。
    pub title: String,
    /// 事件正文。
    pub content: String,
}

impl EventScriptEvent {
    /// 校验事件符合 provider 契约：命名空间化 event_id、字段非空且不超长度上限。
    ///
    /// 脚本 stdout 与 provider 统一信封共用同一份校验，避免两条入口出现契约漂移。
    pub fn validate(provider: &str, event: Self) -> Result<Self, EventScriptError> {
        if event.content.trim().is_empty() {
            return Err(EventScriptError::InvalidOutput(
                "content 不能为空".to_owned(),
            ));
        }
        validate_field("event_id", &event.event_id, MAX_EXTERNAL_EVENT_ID_LENGTH)?;
        validate_field("title", &event.title, MAX_EXTERNAL_EVENT_TITLE_LENGTH)?;
        if let Some(correlation_key) = event.correlation_key.as_deref() {
            validate_field(
                "correlation_key",
                correlation_key,
                MAX_EXTERNAL_EVENT_CORRELATION_KEY_LENGTH,
            )?;
        }
        let namespace = format!("{provider}:");
        if !event.event_id.starts_with(&namespace) {
            return Err(EventScriptError::InvalidOutput(format!(
                "event_id 必须以 {namespace} 开头，保证 provider 命名空间化与幂等键稳定"
            )));
        }
        Ok(event)
    }

    /// 返回参与防抖合并的业务关联键。
    #[must_use]
    pub fn debounce_key(&self) -> &str {
        self.correlation_key
            .as_deref()
            .unwrap_or(self.event_id.as_str())
    }
}

/// provider 脚本调用失败原因；任何失败都 fail-closed 丢弃事件并写审计。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EventScriptError {
    /// provider 名非法（含路径穿越字符或超长）。
    #[error("provider 名非法：{0}")]
    InvalidProvider(String),
    /// provider 目录不存在。
    #[error("provider 未注册：{0}")]
    UnknownProvider(String),
    /// provider 目录缺少解析脚本。
    #[error("provider 缺少 {EVENT_PROVIDER_PARSER_FILE_NAME}：{0}")]
    MissingParser(String),
    /// 解释器或脚本进程无法启动。
    #[error("provider 脚本无法启动：{0}")]
    Spawn(String),
    /// 脚本超过硬超时，已整树回收。
    #[error("provider 脚本超过 {EVENT_SCRIPT_TIMEOUT:?} 硬超时，已回收进程树")]
    Timeout,
    /// 脚本以非零状态退出。
    #[error("provider 脚本退出码 {code:?}，stderr：{stderr}")]
    Failed {
        /// 退出码。
        code: Option<i32>,
        /// stderr 审计摘要。
        stderr: String,
    },
    /// 脚本 stdout 不是合法契约 JSON。
    #[error("provider 脚本输出不是合法 JSON：{0}")]
    InvalidJson(String),
    /// 脚本输出缺少字段、字段为空或超出长度上限。
    #[error("provider 脚本输出不合契约：{0}")]
    InvalidOutput(String),
}

/// provider 注册表：解析 provider 目录、SOP 路径并执行解析脚本。
#[derive(Debug, Clone)]
pub struct EventProviderRegistry {
    root: PathBuf,
}

impl EventProviderRegistry {
    /// 使用 provider 根目录（`~/.axon/events`）创建注册表。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 返回 provider 根目录。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 校验 provider 名：只允许小写 ASCII 字母、数字、`-` 与 `_`，杜绝目录穿越与大小写歧义。
    pub fn validate_provider_name(provider: &str) -> Result<(), EventScriptError> {
        let valid = !provider.is_empty()
            && provider.chars().count() <= EVENT_PROVIDER_NAME_MAX_LENGTH
            && provider.chars().all(|value| {
                value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-' || value == '_'
            });
        valid
            .then_some(())
            .ok_or_else(|| EventScriptError::InvalidProvider(provider.to_owned()))
    }

    /// 判断 provider 目录是否已注册。
    #[must_use]
    pub fn is_registered(&self, provider: &str) -> bool {
        Self::validate_provider_name(provider).is_ok() && self.root.join(provider).is_dir()
    }

    /// 返回 provider 的 SOP 文档绝对路径；未注册时返回 `None`。
    #[must_use]
    pub fn sop_path(&self, provider: &str) -> Option<PathBuf> {
        Self::validate_provider_name(provider).ok()?;
        let path = self.root.join(provider).join(EVENT_PROVIDER_SOP_FILE_NAME);
        path.is_file().then_some(path)
    }

    /// 执行 provider 解析脚本，返回通过契约校验的事件。
    pub fn run_parser(
        &self,
        provider: &str,
        payload: &serde_json::Value,
        received_at_ms: u64,
    ) -> Result<EventScriptEvent, EventScriptError> {
        let parser = self.parser_path(provider)?;
        let input = serde_json::to_vec(&EventScriptInput {
            provider,
            received_at_ms,
            payload,
        })
        .map_err(|error| EventScriptError::Spawn(error.to_string()))?;
        let (status, stdout, stderr) = self.run_script(&parser, input)?;
        if !status.success() {
            return Err(EventScriptError::Failed {
                code: status.code(),
                stderr: audit_excerpt(&stderr),
            });
        }
        let event = serde_json::from_slice::<EventScriptEvent>(&stdout)
            .map_err(|error| EventScriptError::InvalidJson(error.to_string()))?;
        EventScriptEvent::validate(provider, event)
    }

    fn parser_path(&self, provider: &str) -> Result<PathBuf, EventScriptError> {
        Self::validate_provider_name(provider)?;
        let directory = self.root.join(provider);
        if !directory.is_dir() {
            return Err(EventScriptError::UnknownProvider(provider.to_owned()));
        }
        let parser = directory.join(EVENT_PROVIDER_PARSER_FILE_NAME);
        if !parser.is_file() {
            return Err(EventScriptError::MissingParser(provider.to_owned()));
        }
        Ok(parser)
    }

    fn run_script(
        &self,
        parser: &Path,
        input: Vec<u8>,
    ) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), EventScriptError> {
        let mut command = non_interactive_command(EVENT_SCRIPT_INTERPRETER);
        command
            .arg(parser)
            .current_dir(parser.parent().unwrap_or(self.root.as_path()))
            .env_clear()
            .env(
                EVENT_SCRIPT_INTERPRETER_UTF8_ENV.0,
                EVENT_SCRIPT_INTERPRETER_UTF8_ENV.1,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in EVENT_SCRIPT_ENV_ALLOWLIST {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        configure_managed_process_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| EventScriptError::Spawn(error.to_string()))?;
        let _guard = track_managed_process(child.id(), EVENT_SCRIPT_TIMEOUT);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EventScriptError::Spawn("脚本 stdin 管道不可用".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EventScriptError::Spawn("脚本 stdout 管道不可用".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| EventScriptError::Spawn("脚本 stderr 管道不可用".to_owned()))?;
        // stdin 写入与 stdout/stderr 读取都必须脱离等待线程：脚本未读满输入就输出
        // 大报文时，同线程顺序处理会与管道缓冲互等并绕过硬超时。
        let stdin_writer = thread::spawn(move || write_stdin(stdin, input));
        let stdout_reader = thread::spawn(move || read_stream(stdout));
        let stderr_reader = thread::spawn(move || read_stream(stderr));
        let status = wait_for_exit(&mut child)?;
        let _ = stdin_writer.join();
        Ok((
            status,
            stdout_reader.join().unwrap_or_default(),
            stderr_reader.join().unwrap_or_default(),
        ))
    }
}

/// 脚本 stdin 输入契约。
#[derive(Debug, Serialize)]
struct EventScriptInput<'a> {
    provider: &'a str,
    received_at_ms: u64,
    payload: &'a serde_json::Value,
}

fn wait_for_exit(child: &mut Child) -> Result<ExitStatus, EventScriptError> {
    let deadline = Instant::now() + EVENT_SCRIPT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => return Err(EventScriptError::Spawn(error.to_string())),
        }
        if Instant::now() >= deadline {
            let _ = terminate_managed_process_tree(child);
            let _ = child.wait();
            return Err(EventScriptError::Timeout);
        }
        thread::sleep(EVENT_SCRIPT_POLL_INTERVAL);
    }
}

fn write_stdin(mut stdin: ChildStdin, input: Vec<u8>) {
    let _ = stdin.write_all(&input);
}

fn read_stream(mut reader: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    buffer
}

fn audit_excerpt(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .chars()
        .take(EVENT_SCRIPT_STDERR_AUDIT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn validate_field(field: &str, value: &str, max_chars: usize) -> Result<(), EventScriptError> {
    if value.trim().is_empty() {
        return Err(EventScriptError::InvalidOutput(format!("{field} 不能为空")));
    }
    if value.chars().count() > max_chars {
        return Err(EventScriptError::InvalidOutput(format!(
            "{field} 不能超过 {max_chars} 个字符"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "event_scripts_tests.rs"]
mod event_scripts_tests;
