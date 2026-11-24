use std::{
    fmt::Write as _,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

use axon_agent_runtime::{
    SHELL_COMMAND_CANCEL_POLL_BACKOFF_FACTOR, SHELL_COMMAND_CANCEL_POLL_MAX_INTERVAL_MS,
    SHELL_COMMAND_POLL_INTERVAL_MS, SHELL_COMMAND_TIMEOUT_EXIT_CODE,
};
use axon_config::ConfigLoader;
use axon_crypto::sha256_hex;
use axon_process::non_interactive_command;
use axon_time::duration_ms;
use serde::Serialize;

use crate::{
    CliRunResult, ExecCommand, ExecKind, OutputFormat,
    display_units::format_duration_millis_compact,
    management_commands::{cli_config_error, cli_error, json_result, resolved_config_path},
};

const EXEC_EMPTY_COMMAND_ERROR: &str = "axon exec 需要至少一个命令参数";
const EXEC_TIMEOUT_ERROR: &str = "axon exec timeout-ms 必须大于 0";
const EXEC_SHELL_DENIED_ERROR: &str = "axon exec 不允许通过 shell 包装执行；请直接传入程序和参数";
const EXEC_CWD_ERROR: &str = "axon exec 工作目录不存在或不可访问";
const EXEC_IO_ERROR: &str = "axon exec 启动或读取进程失败";
const EXEC_TOOL_NAME: &str = "axon.exec";
const EXEC_KIND_COMMAND: &str = "command";
const EXEC_KIND_TEST: &str = "test";
const EXEC_POLICY_HOST_PROCESS_NO_SHELL: &str = "host_process_no_shell";
const EXEC_SHELL_POLICY_DENIED: &str = "denied";
const EXEC_STDIN_POLICY_NULL: &str = "null";
const SHELL_PROGRAMS: &[&str] = &["sh", "bash", "zsh", "fish", "cmd", "powershell", "pwsh"];

/// 执行 `axon exec`。
pub(crate) fn run_exec(
    command: &ExecCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    if let Err(error) = ConfigLoader::load_from_path(&path) {
        return cli_config_error(&error);
    }
    let timeout_ms = command.timeout_ms;
    let request =
        match ExecRequest::from_cli(command.command.clone(), command.kind, cwd, timeout_ms) {
            Ok(request) => request,
            Err(error) => return cli_error(error),
        };
    let report = match StdCommandExecutor.execute(&request) {
        Ok(report) => report,
        Err(error) => return cli_error(error),
    };
    result_from_report(&report, format)
}

#[derive(Debug, Clone)]
struct ExecRequest {
    command: Vec<String>,
    kind: ExecKind,
    cwd: PathBuf,
    timeout_ms: Option<u64>,
}

impl ExecRequest {
    fn from_cli(
        command: Vec<String>,
        kind: ExecKind,
        cwd: Option<&Path>,
        timeout_ms: Option<u64>,
    ) -> Result<Self, &'static str> {
        if command.is_empty() {
            return Err(EXEC_EMPTY_COMMAND_ERROR);
        }
        validate_exec_timeout(timeout_ms)?;
        if shell_program_denied(&command[0]) {
            return Err(EXEC_SHELL_DENIED_ERROR);
        }
        let cwd = match cwd {
            Some(path) => path.canonicalize().map_err(|_| EXEC_CWD_ERROR)?,
            None => std::env::current_dir().map_err(|_| EXEC_CWD_ERROR)?,
        };
        Ok(Self {
            command,
            kind,
            cwd,
            timeout_ms,
        })
    }
}

fn validate_exec_timeout(timeout_ms: Option<u64>) -> Result<(), &'static str> {
    if timeout_ms == Some(0) {
        return Err(EXEC_TIMEOUT_ERROR);
    }
    Ok(())
}

trait CommandExecutor {
    fn execute(&self, request: &ExecRequest) -> Result<ExecReport, &'static str>;
}

#[derive(Debug, Default, Clone, Copy)]
struct StdCommandExecutor;

impl CommandExecutor for StdCommandExecutor {
    fn execute(&self, request: &ExecRequest) -> Result<ExecReport, &'static str> {
        let output = run_host_process(&request.command, &request.cwd, request.timeout_ms)
            .map_err(|_| EXEC_IO_ERROR)?;
        Ok(ExecReport::new(request, &output))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ExecReport {
    status: ExecStatus,
    tool: &'static str,
    command: Vec<String>,
    cwd: String,
    exit_code: i32,
    timed_out: bool,
    duration_ms: u64,
    timeout_ms: Option<u64>,
    kind: &'static str,
    network_allowed: bool,
    writes_files_allowed: bool,
    test_passed: Option<bool>,
    shell_used: bool,
    remote_trigger_allowed: bool,
    safety: ExecSafetyPlan,
    stdout: BoundedStream,
    stderr: BoundedStream,
}

impl ExecReport {
    fn new(request: &ExecRequest, output: &ExecOutput) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Self {
            status: if output.exit_code == 0 && !output.timed_out {
                ExecStatus::Completed
            } else {
                ExecStatus::Failed
            },
            tool: EXEC_TOOL_NAME,
            command: request.command.clone(),
            cwd: request.cwd.display().to_string(),
            exit_code: output.exit_code,
            timed_out: output.timed_out,
            duration_ms: output.duration_ms,
            timeout_ms: request.timeout_ms,
            kind: exec_kind_name(request.kind),
            network_allowed: true,
            writes_files_allowed: true,
            test_passed: test_passed(request.kind, output.exit_code, output.timed_out),
            shell_used: false,
            remote_trigger_allowed: false,
            safety: ExecSafetyPlan::host_process(),
            stdout: BoundedStream::from_text(&stdout),
            stderr: BoundedStream::from_text(&stderr),
        }
    }

    fn to_text(&self) -> String {
        let mut output = format!(
            "axon exec {}：kind={} exit={} timeout={} duration={}\n",
            self.status.as_str(),
            self.kind,
            self.exit_code,
            self.timeout_ms
                .map_or_else(|| "none".to_owned(), format_duration_millis_compact,),
            format_duration_millis_compact(self.duration_ms)
        );
        // Invariant: writing formatted text into String is infallible.
        writeln!(
            output,
            "safety policy={} shell={} stdin={} remoteTriggerAllowed={}",
            self.safety.policy,
            self.safety.shell,
            self.safety.stdin,
            self.safety.remote_trigger_allowed
        )
        .expect("写入 String 不应失败");
        if let Some(test_passed) = self.test_passed {
            // Invariant: writing formatted text into String is infallible.
            writeln!(output, "testPassed={test_passed}").expect("写入 String 不应失败");
        }
        if !self.stdout.preview.is_empty() {
            output.push_str(&self.stdout.preview);
            if !output.ends_with('\n') {
                output.push('\n');
            }
        }
        if !self.stderr.preview.is_empty() {
            output.push_str("stderr:\n");
            output.push_str(&self.stderr.preview);
            if !output.ends_with('\n') {
                output.push('\n');
            }
        }
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ExecStatus {
    Completed,
    Failed,
}

impl ExecStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecSafetyPlan {
    policy: &'static str,
    shell: &'static str,
    stdin: &'static str,
    remote_trigger_allowed: bool,
}

impl ExecSafetyPlan {
    const fn host_process() -> Self {
        Self {
            policy: EXEC_POLICY_HOST_PROCESS_NO_SHELL,
            shell: EXEC_SHELL_POLICY_DENIED,
            stdin: EXEC_STDIN_POLICY_NULL,
            remote_trigger_allowed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BoundedStream {
    preview: String,
    truncated: bool,
    original_chars: usize,
    retained_chars: usize,
    sha256: String,
}

impl BoundedStream {
    fn from_text(text: &str) -> Self {
        let chars = text.chars().count();
        Self {
            preview: text.to_owned(),
            truncated: false,
            original_chars: chars,
            retained_chars: chars,
            sha256: sha256_hex(text.as_bytes()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecOutput {
    exit_code: i32,
    timed_out: bool,
    duration_ms: u64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_host_process(
    command: &[String],
    cwd: &Path,
    timeout_ms: Option<u64>,
) -> std::io::Result<ExecOutput> {
    let (program, args) = command.split_first().ok_or_else(io_error)?;
    let start = Instant::now();
    let mut child = non_interactive_command(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(io_error)?;
    let stderr = child.stderr.take().ok_or_else(io_error)?;
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));
    let deadline = timeout_ms.map(Duration::from_millis);
    let mut timed_out = false;
    let mut poll_delay = Duration::from_millis(SHELL_COMMAND_POLL_INTERVAL_MS);

    while child.try_wait()?.is_none() {
        if deadline.is_some_and(|deadline| start.elapsed() >= deadline) {
            timed_out = true;
            child.kill()?;
            break;
        }
        thread::sleep(poll_delay);
        poll_delay = poll_delay
            .saturating_mul(SHELL_COMMAND_CANCEL_POLL_BACKOFF_FACTOR)
            .min(Duration::from_millis(
                SHELL_COMMAND_CANCEL_POLL_MAX_INTERVAL_MS,
            ));
    }

    let status = child.wait()?;
    let stdout = stdout_reader.join().map_err(|_| io_error())??;
    let stderr = stderr_reader.join().map_err(|_| io_error())??;
    Ok(ExecOutput {
        exit_code: if timed_out {
            SHELL_COMMAND_TIMEOUT_EXIT_CODE
        } else {
            status.code().unwrap_or(1)
        },
        timed_out,
        duration_ms: duration_ms(start.elapsed()),
        stdout,
        stderr,
    })
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn result_from_report(report: &ExecReport, format: OutputFormat) -> CliRunResult {
    let mut result = match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: report.to_text(),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(report),
    };
    if report.status == ExecStatus::Failed {
        result.exit_code = 1;
    }
    result
}

fn shell_program_denied(program: &str) -> bool {
    Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            let normalized = stem.to_ascii_lowercase();
            SHELL_PROGRAMS.contains(&normalized.as_str())
        })
}

const fn exec_kind_name(kind: ExecKind) -> &'static str {
    match kind {
        ExecKind::Command => EXEC_KIND_COMMAND,
        ExecKind::Test => EXEC_KIND_TEST,
    }
}

fn test_passed(kind: ExecKind, exit_code: i32, timed_out: bool) -> Option<bool> {
    (kind == ExecKind::Test).then_some(exit_code == 0 && !timed_out)
}

fn io_error() -> std::io::Error {
    std::io::Error::other(EXEC_IO_ERROR)
}

#[cfg(test)]
#[path = "../tests/exec_commands_tests.rs"]
mod exec_commands_tests;
