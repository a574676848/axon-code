//! Shell command tool 的执行回执与有界输出。

use axon_crypto::sha256_hex;
use std::path::PathBuf;

/// shell command tool 执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommandExecutionStatus {
    /// 进程仍在运行，可通过 write_stdin 继续交互。
    Running,
    /// 已完成且退出码为 0。
    Completed,
    /// 已执行但命令失败。
    Failed,
    /// 被安全门禁阻断。
    Blocked,
}

/// 后台终端续接调用的语义类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommandInteractionKind {
    /// 未写入 stdin，仅等待增量输出或终态。
    Wait,
    /// 向 stdin 写入字符。
    Input,
    /// 发送 Ctrl-C 中断。
    Interrupt,
}

/// 一次后台终端等待或交互。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandInteraction {
    /// 交互类型。
    pub kind: ShellCommandInteractionKind,
    /// 实际写入 stdin 的字符；等待与中断时为空。
    pub stdin: String,
}

impl ShellCommandInteraction {
    pub(super) fn from_input(chars: Option<&str>, ctrl_c: bool) -> Self {
        if ctrl_c {
            return Self {
                kind: ShellCommandInteractionKind::Interrupt,
                stdin: String::new(),
            };
        }
        match chars.filter(|value| !value.is_empty()) {
            Some(stdin) => Self {
                kind: ShellCommandInteractionKind::Input,
                stdin: stdin.to_owned(),
            },
            None => Self {
                kind: ShellCommandInteractionKind::Wait,
                stdin: String::new(),
            },
        }
    }
}

/// shell command tool 执行回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandExecutionReceipt {
    /// 状态。
    pub status: ShellCommandExecutionStatus,
    /// 持久进程会话 ID；短命令在首次等待内结束时为空。
    pub session_id: Option<String>,
    /// 命令 argv。
    pub command: Vec<String>,
    /// 工作目录。
    pub cwd: String,
    /// 退出码。
    pub exit_code: Option<i32>,
    /// 是否超时。
    pub timed_out: bool,
    /// 是否因取消、Ctrl-C 或空闲回收终止。
    pub cancelled: bool,
    /// 是否通过 PTY 执行。
    pub tty: bool,
    /// 执行耗时毫秒。
    pub duration_ms: u64,
    /// stdout 摘要。
    pub stdout: ShellCommandBoundedStream,
    /// stderr 摘要。
    pub stderr: ShellCommandBoundedStream,
    /// 原始输出估算 token。
    pub original_output_tokens: usize,
    /// 过滤后输出估算 token。
    pub filtered_output_tokens: usize,
    /// 输出过滤节省比例。
    pub output_savings_pct: u8,
    /// 使用的命令输出处理器。
    pub output_handler_used: String,
    /// 完整输出归档路径。
    pub full_output_path: Option<String>,
    /// 归档是否因达到容量上限而不完整；为真时 `full_output_path` 只是部分输出。
    pub artifact_truncated: bool,
    /// 归档**写入是否失败**（磁盘写满、数据目录不可写、卷只读等）；为真时
    /// `full_output_path` 指向的文件只含前缀。
    ///
    /// 与 `artifact_truncated` **并列**构成「完整输出是否可用」的判据：任一处为真
    /// 都意味着归档不完整。作业通道（`job_runtime`）本就两处都查，此处补齐同口径。
    pub artifact_failed: bool,
    /// 会话输出代数：每次输出追加或状态跃迁时递增（`SharedProcessOutput` 的
    /// change generation）。
    ///
    /// 归档调用方据此判别「tee 归档内容自上次归档以来是否变化」：代数未变即可
    /// 复用上次归档引用，无需再做全量读取与 SHA-256（R2 审计 N-B-02，O(N·S)→O(1)）。
    pub output_generation: u64,
    /// 命令终态时检测到的实际改动路径。
    pub changed_paths: Vec<PathBuf>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 本次是否由后台终端等待或 stdin 交互产生。
    pub interaction: Option<ShellCommandInteraction>,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 有界输出流。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandBoundedStream {
    /// 预览文本。
    pub preview: String,
    /// 是否截断。
    pub truncated: bool,
    /// 原始字符数。
    pub original_chars: usize,
    /// 保留字符数。
    pub retained_chars: usize,
    /// 原始内容 hash。
    pub sha256: String,
}

impl ShellCommandBoundedStream {
    pub(super) fn from_bytes(bytes: &[u8]) -> Self {
        let text = String::from_utf8_lossy(bytes);
        Self::from_text(&text, sha256_hex(bytes))
    }

    pub(super) fn from_capture(
        preview: String,
        truncated: bool,
        original_chars: usize,
        retained_chars: usize,
        sha256: String,
    ) -> Self {
        Self {
            preview,
            truncated,
            original_chars,
            retained_chars,
            sha256,
        }
    }

    /// 空流：尚无任何输出。
    pub(crate) fn empty() -> Self {
        Self::from_capture(String::new(), false, 0, 0, sha256_hex([]))
    }

    /// 以内存文本构造有界流；超过上限时保留头部与尾部，中间以省略标记连接。
    ///
    /// 子代理结果没有增量过程，只有一次性终局正文；超限时必须仍让模型看到
    /// **结论的开头与结尾**，因此不做单侧截断。保留总量（含省略标记）不超过
    /// `max_chars`，上限是真上界而不是近似值。
    pub(crate) fn from_bounded_text(text: &str, max_chars: usize) -> Self {
        let original_chars = text.chars().count();
        let sha256 = sha256_hex(text.as_bytes());
        if original_chars <= max_chars {
            return Self::from_capture(
                text.to_owned(),
                false,
                original_chars,
                original_chars,
                sha256,
            );
        }
        let marker_overhead = STREAM_TRUNCATION_MARKER.chars().count() + 2;
        if max_chars <= marker_overhead {
            let preview = text.chars().take(max_chars).collect::<String>();
            let retained_chars = preview.chars().count();
            return Self::from_capture(preview, true, original_chars, retained_chars, sha256);
        }
        let content_budget = max_chars - marker_overhead;
        let head_chars = content_budget / 2;
        let tail_chars = content_budget - head_chars;
        let head = text.chars().take(head_chars).collect::<String>();
        let tail = text
            .chars()
            .skip(original_chars.saturating_sub(tail_chars))
            .collect::<String>();
        let preview = format!("{head}\n{STREAM_TRUNCATION_MARKER}\n{tail}");
        let retained_chars = preview.chars().count();
        Self::from_capture(preview, true, original_chars, retained_chars, sha256)
    }

    fn from_text(text: &str, sha256: String) -> Self {
        Self {
            preview: text.to_owned(),
            truncated: false,
            original_chars: text.chars().count(),
            retained_chars: text.chars().count(),
            sha256,
        }
    }
}

/// 有界流中间被省略时的稳定标记。
const STREAM_TRUNCATION_MARKER: &str = "...（中间内容已省略，完整内容见归档）...";

pub(super) fn blocked_receipt(
    command: Vec<String>,
    cwd: PathBuf,
    tty: bool,
    reason: &'static str,
) -> ShellCommandExecutionReceipt {
    ShellCommandExecutionReceipt {
        status: ShellCommandExecutionStatus::Blocked,
        session_id: None,
        command,
        cwd: cwd.display().to_string(),
        exit_code: None,
        timed_out: false,
        cancelled: false,
        tty,
        duration_ms: 0,
        stdout: ShellCommandBoundedStream::from_bytes(&[]),
        stderr: ShellCommandBoundedStream::from_bytes(&[]),
        original_output_tokens: 0,
        filtered_output_tokens: 0,
        output_savings_pct: 0,
        output_handler_used: "none".to_owned(),
        full_output_path: None,
        artifact_truncated: false,
        artifact_failed: false,
        output_generation: 0,
        changed_paths: Vec::new(),
        remote_trigger_allowed: false,
        interaction: None,
        reasons: vec![reason.to_owned()],
    }
}
