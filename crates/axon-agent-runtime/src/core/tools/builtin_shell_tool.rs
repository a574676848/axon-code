//! 内置持久 Shell 工具的校验、执行与回执。

#[path = "builtin_shell_tool/output_buffer.rs"]
pub(crate) mod output_buffer;

#[path = "builtin_shell_tool/process_backend.rs"]
pub(crate) mod process_backend;

#[path = "builtin_shell_tool/process_manager.rs"]
pub(crate) mod process_manager;

#[path = "builtin_shell_tool/receipt.rs"]
mod receipt;

#[path = "builtin_shell_tool/session.rs"]
mod session;

#[path = "builtin_shell_tool/session_reclaim.rs"]
pub(crate) mod session_reclaim;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axon_domain::{
    AxonError, AxonResult, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};

pub use self::receipt::{
    ShellCommandBoundedStream, ShellCommandExecutionReceipt, ShellCommandExecutionStatus,
    ShellCommandInteraction, ShellCommandInteractionKind,
};
pub use self::session_reclaim::{
    ShellSessionReclaim, ShellSessionReclaimHandler, ShellSessionReclaimSubscription,
    subscribe_shell_session_reclaims,
};
use self::{process_manager::ShellProcessManager, receipt::blocked_receipt};
use crate::{
    ToolRegistry,
    builtin_shell_script::{
        UNTRUSTED_SHELL_ERROR, command_from_script, command_from_script_at_cwd,
        shell_program_denied, validate_shell_environment,
    },
    job_runtime::{JobId, JobRuntime, JobSpec, bounded_job_label},
    shell_command_static_analyzer::ShellCommandStaticAnalyzer,
    tool_runtime_budget::{ToolBudgetKind, tool_runtime_budget},
};

/// 内置 shell command tool 名称。
pub const SHELL_COMMAND_TOOL_NAME: &str = "shell_command";
/// shell command tool schema version。
pub const SHELL_COMMAND_TOOL_SCHEMA_VERSION: u32 = 12;
/// shell command tool ID。
pub const SHELL_COMMAND_TOOL_ID: ToolId = ToolId(11);
/// shell command tool 必填 command 字段。
pub const SHELL_COMMAND_TOOL_COMMAND_FIELD: &str = "command";
/// shell command 可选工作目录字段。
pub const SHELL_COMMAND_TOOL_WORKDIR_FIELD: &str = "workdir";
/// shell command 可选最长等待时间字段。
pub const SHELL_COMMAND_TOOL_TIMEOUT_FIELD: &str = "timeout_ms";
/// shell command 可选 PTY 字段。
pub const SHELL_COMMAND_TOOL_TTY_FIELD: &str = "tty";
/// shell command 可选后台作业字段。
pub const SHELL_COMMAND_TOOL_RUN_IN_BACKGROUND_FIELD: &str = "run_in_background";
/// shell command tool 可选让出等待字段。
pub const SHELL_COMMAND_TOOL_YIELD_TIME_FIELD: &str = "yield_time_ms";
/// Shell 工具单次回执的可选输出 token 预算字段。
pub const SHELL_COMMAND_TOOL_MAX_OUTPUT_TOKENS_FIELD: &str = "max_output_tokens";
/// shell command 必填目的概要字段；与所有声明 `intent` 的内置工具同名同义。
///
/// 只用于界面展示：折叠态显示这一句人话，展开态才显示命令原文。
/// 不参与执行语义；必填是为了让折叠态永远是概要而不是命令截取。
pub const SHELL_COMMAND_TOOL_INTENT_FIELD: &str = crate::TOOL_INTENT_FIELD;
/// write_stdin tool 名称。
pub const WRITE_STDIN_TOOL_NAME: &str = "write_stdin";
/// write_stdin tool schema version。
pub const WRITE_STDIN_TOOL_SCHEMA_VERSION: u32 = 6;
/// write_stdin tool ID。
pub const WRITE_STDIN_TOOL_ID: ToolId = ToolId(13);
/// write_stdin 必填会话字段。
pub const WRITE_STDIN_TOOL_SESSION_ID_FIELD: &str = "session_id";
/// write_stdin 可选输入字符字段。
pub const WRITE_STDIN_TOOL_CHARS_FIELD: &str = "chars";
/// write_stdin 可选 Ctrl-C 字段。
pub const WRITE_STDIN_TOOL_CTRL_C_FIELD: &str = "ctrl_c";
/// Shell 工具默认等待进程输出的时长。
pub const DEFAULT_SHELL_YIELD_TIME_MS: u64 = 10_000;
/// write_stdin 默认请求的等待时长；空输入会提升到后台终端最小等待窗口。
pub const DEFAULT_WRITE_STDIN_YIELD_TIME_MS: u64 = 250;
/// Shell 工具拒绝远控原因。
pub const SHELL_COMMAND_REASON_REMOTE_TRIGGER_CLOSED: &str = "shell_command:remote_trigger_closed";
/// Shell 工具通过本机进程执行原因。
pub const SHELL_COMMAND_REASON_HOST_PROCESS_USED: &str = "shell_command:host_process_used";
/// `run_in_background` 与 `tty` 同时为 true 时的拒绝原因。
///
/// 两条通道提供的是**不同**的句柄：后台作业给 `job_id`，交互会话给 `session_id`。
/// 同时请求两者只能二选一，静默忽略其中一个会让模型拿着一个并不存在的 ID 继续操作，
/// 因此这里 fail-closed 拒绝，而不是替模型做选择。
pub const SHELL_COMMAND_BACKGROUND_TTY_CONFLICT_ERROR: &str =
    "run_in_background 与 tty 不能同时为 true：后台作业不提供终端，交互会话不提供 job_id";
/// 执行入口与 `run_in_background` 不匹配时的拒绝原因。
const SHELL_COMMAND_EXECUTION_MODE_ENTRY_ERROR: &str = "run_in_background 决定执行入口：为 true 时必须用 start_background 启动作业，为 false 时走前台会话";
/// 近似字符/token 比；Shell 与作业回执共用同一折算口径。
pub(crate) const APPROX_CHARS_PER_TOKEN: usize = 4;
const SHELL_COMMAND_TIMEOUT_EXIT_CODE: i32 = crate::runtime_limits::SHELL_COMMAND_TIMEOUT_EXIT_CODE;
const SHELL_COMMAND_CANCELLED_EXIT_CODE: i32 = 130;
const SHELL_COMMAND_OUTPUT_CHUNK_BYTES: usize =
    crate::runtime_limits::SHELL_COMMAND_OUTPUT_CHUNK_BYTES;
const SHELL_COMMAND_EMPTY_ERROR: &str = "shell_command 命令不能为空";
const SHELL_COMMAND_TIMEOUT_OUT_OF_RANGE_ERROR: &str =
    "shell_command timeout_ms 超出运行时预算边界";
const SHELL_COMMAND_CWD_ERROR: &str = "shell_command cwd 不可访问";
const SHELL_COMMAND_IO_ERROR: &str = "Shell 进程启动、交互或读取失败";
const SHELL_COMMAND_OWNER_EMPTY_ERROR: &str = "Shell 进程 owner_id 不能为空";
const SHELL_SESSION_NOT_FOUND_ERROR: &str = "Shell session 不存在";
/// 会话曾真实存在，但已超出保留窗口被回收。
///
/// 与 `SHELL_SESSION_NOT_FOUND_ERROR` 区分：前者说明模型持有的 ID 有效只是过期了，
/// 应当重新执行命令；后者说明该 ID 从未产生过。
const SHELL_SESSION_EXPIRED_ERROR: &str = "Shell session 已结束并超出保留窗口，请重新执行命令";
const SHELL_SESSION_OWNER_ERROR: &str = "Shell session 不属于当前 Agent run";
const WRITE_STDIN_SESSION_EMPTY_ERROR: &str = "write_stdin session_id 不能为空";
const WRITE_STDIN_INPUT_CONFLICT_ERROR: &str = "write_stdin chars 与 ctrl_c 不能同时提供";
/// shell command 输出流。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommandOutputStream {
    /// 标准输出。
    Stdout,
    /// 标准错误。
    Stderr,
}

/// shell command 增量输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandOutputChunk {
    /// 输出流。
    pub stream: ShellCommandOutputStream,
    /// 单调递增序号。
    pub seq: u64,
    /// 原始字节。
    pub data: Vec<u8>,
}

/// shell command 取消 token。
pub type ShellCommandCancelToken = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

/// shell command tool 执行输入。
#[derive(Clone)]
pub struct ShellCommandExecutionInput {
    /// 当前 Agent run 的内部所有者 ID，不由模型提供。
    pub owner_id: String,
    /// 已由 Axon 规范化的命令 argv。
    pub command: Vec<String>,
    /// 命令工作目录。
    pub cwd: PathBuf,
    /// 可选超时毫秒；仅显式提供时终止进程。
    pub timeout_ms: Option<u64>,
    /// 首次调用等待进程输出的时长。
    pub yield_time_ms: u64,
    /// 是否分配 PTY。
    pub tty: bool,
    /// 是否把命令转为后台作业；与 `tty` 硬互斥。
    ///
    /// 为 true 时本输入只由 [`ShellCommandToolRuntime::start_background`] 消费：
    /// 回执返回 `job_id` 而**不返回** `session_id`。
    pub run_in_background: bool,
    /// 本次回执的可选输出 token 预算。
    pub max_output_tokens: Option<usize>,
    /// 失败输出归档根目录；为空时不写完整输出。
    pub tee_root: Option<PathBuf>,
    /// 协作式取消 token；触发后会终止正在运行的本机子进程。
    pub cancel_token: Option<ShellCommandCancelToken>,
    /// 可能被命令修改的路径，用于终态变更检测。
    pub write_paths: Vec<PathBuf>,
    /// 是否远控触发。
    pub remote_requested: bool,
}

/// write_stdin tool 执行输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteStdinExecutionInput {
    /// 当前 Agent run 的内部所有者 ID，不由模型提供。
    pub owner_id: String,
    /// shell_command 返回的持久会话 ID。
    pub session_id: String,
    /// 写入 stdin 的原始字符；为空表示仅轮询。
    pub chars: Option<String>,
    /// 是否发送 Ctrl-C。
    pub ctrl_c: bool,
    /// 本次等待增量输出的时长。
    pub yield_time_ms: u64,
    /// 本次回执的可选输出 token 预算。
    pub max_output_tokens: Option<usize>,
    /// 是否远控触发。
    pub remote_requested: bool,
}

/// shell command stdout/stderr 流式观察者。
pub type ShellCommandOutputObserver<'a> = dyn FnMut(ShellCommandOutputChunk) + 'a;

/// shell command tool runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ShellCommandToolRuntime;

impl ShellCommandToolRuntime {
    /// 注册 shell_command 与 write_stdin 工具。
    pub fn register_tools(registry: &mut ToolRegistry) -> AxonResult<Vec<ToolDefinition>> {
        let tools = vec![
            shell_command_tool_definition(),
            write_stdin_tool_definition(),
        ];
        let mut candidate = registry.clone();
        for tool in &tools {
            candidate.register(tool.clone())?;
        }
        *registry = candidate;
        Ok(tools)
    }

    /// 将 provider 脚本文本规范化为当前平台默认 shell 的命令。
    pub fn command_from_script(script: &str, tty: bool) -> AxonResult<Vec<String>> {
        command_from_script(script, SHELL_COMMAND_EMPTY_ERROR, tty)
    }

    /// 根据有效 cwd 将 provider 脚本文本规范化为本地或 WSL 命令。
    pub fn command_from_script_at_cwd(
        script: &str,
        cwd: &Path,
        tty: bool,
    ) -> AxonResult<Vec<String>> {
        command_from_script_at_cwd(script, SHELL_COMMAND_EMPTY_ERROR, cwd, tty)
    }

    /// 执行已经通过共享工具权限边界的命令。
    pub fn execute(
        &self,
        input: ShellCommandExecutionInput,
    ) -> AxonResult<ShellCommandExecutionReceipt> {
        self.execute_with_output_observer(input, None)
    }

    /// 经审批后启动或复用持久进程，并同步回调本次增量输出。
    pub fn execute_with_output_observer(
        &self,
        mut input: ShellCommandExecutionInput,
        output_observer: Option<&mut ShellCommandOutputObserver<'_>>,
    ) -> AxonResult<ShellCommandExecutionReceipt> {
        validate_execution_mode(&input)?;
        if input.run_in_background {
            return Err(AxonError::tool_invalid_arguments(
                SHELL_COMMAND_EXECUTION_MODE_ENTRY_ERROR,
            ));
        }
        if input.owner_id.trim().is_empty() {
            return Err(AxonError::tool_invalid_arguments(
                SHELL_COMMAND_OWNER_EMPTY_ERROR,
            ));
        }
        if input.remote_requested {
            return Ok(blocked_receipt(
                input.command,
                input.cwd,
                input.tty,
                SHELL_COMMAND_REASON_REMOTE_TRIGGER_CLOSED,
            ));
        }
        validate_command(&input.command)?;
        input.cwd = canonicalize_cwd(&input.cwd)?;
        validate_shell_environment(&input.command)?;
        if let Some(timeout_ms) = input.timeout_ms {
            tool_runtime_budget(ToolBudgetKind::ShellCommand)
                .validate_timeout_ms(timeout_ms)
                .map_err(|_| {
                    AxonError::tool_invalid_arguments(SHELL_COMMAND_TIMEOUT_OUT_OF_RANGE_ERROR)
                })?;
        }
        ShellProcessManager::global()
            .execute(input, output_observer)
            .map_err(|error| AxonError::tool_execution_failed(error.to_string()))
    }

    /// 把命令转为后台作业，返回作业 ID。
    ///
    /// 作业与 Shell 会话**不共享**任何句柄：它注册在独立的作业注册表里，
    /// 只能通过 `job_output` / `job_kill` 操作，`session_id` 通道永远看不到它。
    /// 校验链与前台会话一致（命令、cwd、环境、超时预算），差别只在终点：
    /// 前台进 `ShellProcessManager`，后台进 `JobRuntime`。
    pub fn start_background(&self, mut input: ShellCommandExecutionInput) -> AxonResult<JobId> {
        validate_execution_mode(&input)?;
        if !input.run_in_background {
            return Err(AxonError::tool_invalid_arguments(
                SHELL_COMMAND_EXECUTION_MODE_ENTRY_ERROR,
            ));
        }
        if input.remote_requested {
            return Err(AxonError::tool_execution_failed(
                SHELL_COMMAND_REASON_REMOTE_TRIGGER_CLOSED,
            ));
        }
        if input.owner_id.trim().is_empty() {
            return Err(AxonError::tool_invalid_arguments(
                SHELL_COMMAND_OWNER_EMPTY_ERROR,
            ));
        }
        validate_command(&input.command)?;
        input.cwd = canonicalize_cwd(&input.cwd)?;
        validate_shell_environment(&input.command)?;
        if let Some(timeout_ms) = input.timeout_ms {
            tool_runtime_budget(ToolBudgetKind::ShellCommand)
                .validate_timeout_ms(timeout_ms)
                .map_err(|_| {
                    AxonError::tool_invalid_arguments(SHELL_COMMAND_TIMEOUT_OUT_OF_RANGE_ERROR)
                })?;
        }
        JobRuntime::global().start(JobSpec {
            owner_id: input.owner_id,
            label: bounded_job_label(&input.command.join(" ")),
            command: input.command,
            cwd: input.cwd,
            timeout_ms: input.timeout_ms,
            cancel_token: input.cancel_token,
            tee_root: input.tee_root,
        })
    }

    /// 向持久会话写入字符、发送 Ctrl-C，或仅轮询增量输出。
    pub fn write_stdin(
        &self,
        input: WriteStdinExecutionInput,
    ) -> AxonResult<ShellCommandExecutionReceipt> {
        self.write_stdin_with_output_observer(input, None)
    }

    /// 与持久会话交互，并同步回调本次增量输出。
    pub fn write_stdin_with_output_observer(
        &self,
        input: WriteStdinExecutionInput,
        output_observer: Option<&mut ShellCommandOutputObserver<'_>>,
    ) -> AxonResult<ShellCommandExecutionReceipt> {
        validate_write_stdin_input(&input)?;
        if input.remote_requested {
            return Ok(blocked_receipt(
                Vec::new(),
                PathBuf::new(),
                false,
                SHELL_COMMAND_REASON_REMOTE_TRIGGER_CLOSED,
            ));
        }
        ShellProcessManager::global()
            .write_stdin(input, output_observer)
            .map_err(|error| AxonError::tool_execution_failed(error.to_string()))
    }

    /// 终止并移除指定 Agent run 持有的全部 Shell 会话。
    pub fn terminate_sessions_for_owner(owner_id: &str) -> usize {
        ShellProcessManager::global().terminate_owner(owner_id)
    }
}

pub(crate) fn shell_command_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        SHELL_COMMAND_TOOL_SCHEMA_VERSION,
        [
            SHELL_COMMAND_TOOL_COMMAND_FIELD,
            SHELL_COMMAND_TOOL_INTENT_FIELD,
        ],
        vec![
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_COMMAND_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "要在当前平台 shell 中运行的裸脚本。直接写命令，不要自行包一层 pwsh/powershell -Command、bash/sh/zsh -c 或 cmd /c。JSON 中脚本内的双引号必须转义；多行脚本可直接使用 heredoc 或 here-string —— Axon 不做方言预检，语法与方言错误由 shell 自身回报，并保持 Python 缩进。仅当 shell_command 回执明确返回非空 session_id 时，才可将该完整字符串原样传给 write_stdin；不得自行生成、猜测或复用其他命令的 ID。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_INTENT_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "必填：一句话说明这次命令要达成什么目的，例如「校验代码格式」「确认依赖版本」。写目标而不是复述命令，不加句号，不超过 40 字，不要写命令原文。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_WORKDIR_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "可选启动目录；省略时使用当前工作区。相对路径从当前工作区计算。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_TIMEOUT_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选命令 deadline，单位毫秒；最小 1000，省略时不设置 deadline。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_YIELD_TIME_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选首次等待输出时长，单位毫秒；普通命令使用默认窗口，tty 默认为立即返回，超过前台让出窗口后返回 session_id。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_TTY_FIELD,
                ToolSchemaFieldKind::Boolean,
                "命令需要真正的交互终端时设为 true。此时工具会立即返回 session_id，随后用 write_stdin 继续交互。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_RUN_IN_BACKGROUND_FIELD,
                ToolSchemaFieldKind::Boolean,
                "默认设为 true：主线无需立即依赖该结果时，命令转为后台作业，主线继续推进其他工作。仅当下一步行动强依赖该结果时才设为 false 前台等待。为 true 时回执返回 job_id 而不返回 session_id，随后用 job_output / job_kill 操作。与 tty: true 互斥：同时为 true 会被拒绝，因为后台作业不提供终端，交互会话不提供 job_id。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_MAX_OUTPUT_TOKENS_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选本次回执输出预算，单位 token；默认 2000。",
            ),
        ],
    );
    schema.input_schema["properties"][SHELL_COMMAND_TOOL_TIMEOUT_FIELD]["minimum"] =
        serde_json::json!(crate::runtime_limits::MIN_SHELL_TIMEOUT_MS);
    schema.input_schema["properties"][SHELL_COMMAND_TOOL_MAX_OUTPUT_TOKENS_FIELD]["minimum"] =
        serde_json::json!(1);
    schema.input_schema["examples"] = serde_json::json!([
        { "command": "git status --short" },
        { "command": "python -c \"from pathlib import Path; print(Path('docs').exists())\"" },
        { "command": "python -c \"import json; print(json.dumps({'ok': True}))\"" }
    ]);
    ToolDefinition {
        id: SHELL_COMMAND_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: SHELL_COMMAND_TOOL_NAME.to_owned(),
        description: "在本机 shell 中运行命令；可指定工作目录、超时与回执输出预算。command 必须是当前默认 shell 的裸脚本，JSON 字符串内的双引号要转义；intent 是每次调用必填的一句话目的，写法见该字段描述。需要交互终端时设 tty 为 true，长时间运行或主线无需立即依赖其结果时设 run_in_background 为 true。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::WorkspaceCoordinated,
        output_policy: ToolOutputPolicy::RefOnOverflow,
        permission_policy: ToolPermissionPolicy::CallEvaluated,
    }
}

/// 从 shell_command 调用参数中取出模型自述的目的概要。
///
/// 概要在参数被截断之前取出，因此再长的脚本也不会把这一句话挤掉；
/// 只裁剪展示长度，不改写内容。
#[must_use]
pub fn shell_command_intent(arguments_json: &str) -> Option<String> {
    crate::tool_call_intent(SHELL_COMMAND_TOOL_NAME, arguments_json)
}

/// 返回 write_stdin 工具定义。
pub(crate) fn write_stdin_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        WRITE_STDIN_TOOL_SCHEMA_VERSION,
        [WRITE_STDIN_TOOL_SESSION_ID_FIELD, crate::TOOL_INTENT_FIELD],
        vec![
            ToolSchemaField::new(
                WRITE_STDIN_TOOL_SESSION_ID_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "shell_command 返回的持久会话 ID。必须逐字复制最近一次 shell_command 回执中非空的 session_id；禁止臆造、改写、跨 Agent run 复用或使用已完成命令的 ID。若最近回执没有 session_id，不要调用 write_stdin，应重新调用 shell_command。",
            ),
            ToolSchemaField::new(
                crate::TOOL_INTENT_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                crate::TOOL_INTENT_FIELD_DESCRIPTION,
            ),
            ToolSchemaField::new(
                WRITE_STDIN_TOOL_CHARS_FIELD,
                ToolSchemaFieldKind::String,
                "可选原始输入字符；提供后表示向会话写入输入，换行符需由调用方显式提供。不要与 ctrl_c: true 同时提供。",
            ),
            ToolSchemaField::new(
                WRITE_STDIN_TOOL_CTRL_C_FIELD,
                ToolSchemaFieldKind::Boolean,
                "发送 Ctrl-C；仅在需要中断会话时设为 true。设为 true 时不要提供 chars。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_YIELD_TIME_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选等待增量输出时长，单位毫秒；仅轮询时按后台终端等待窗口约束，写入 stdin 时按交互窗口约束。",
            ),
            ToolSchemaField::new(
                SHELL_COMMAND_TOOL_MAX_OUTPUT_TOKENS_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选本次回执输出预算，单位 token；默认 2000。",
            ),
        ],
    );
    schema.input_schema["properties"][SHELL_COMMAND_TOOL_MAX_OUTPUT_TOKENS_FIELD]["minimum"] =
        serde_json::json!(1);
    schema.input_schema["not"] = serde_json::json!({
        "properties": {
            WRITE_STDIN_TOOL_CTRL_C_FIELD: { "const": true }
        },
        "required": [WRITE_STDIN_TOOL_CHARS_FIELD, WRITE_STDIN_TOOL_CTRL_C_FIELD]
    });
    schema.input_schema["examples"] = serde_json::json!([
        { "session_id": "sess_123" },
        { "session_id": "sess_123", "chars": "y\n" },
        { "session_id": "sess_123", "ctrl_c": true }
    ]);
    ToolDefinition {
        id: WRITE_STDIN_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: WRITE_STDIN_TOOL_NAME.to_owned(),
        description: "继续一个 shell 会话。session_id 必须逐字复制最近一次 shell_command 回执中的 session_id，不能臆造或跨命令复用；intent 是每次调用必填的一句话目的，写法见该字段描述。status 为 Running 时可写入 chars 或发送 ctrl_c: true；会话已结束时本工具只返回终态与退出码（幂等，重复调用结果一致），不会写入任何输入。若提示已超出保留窗口，说明会话已回收，应重新调用 shell_command。只想读取新输出时只传 session_id。纯等待会在输出、进程终态或等待窗口结束时返回。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        output_policy: ToolOutputPolicy::RefOnOverflow,
        permission_policy: ToolPermissionPolicy::CallEvaluated,
    }
}

/// 校验执行形态：`run_in_background` 与 `tty` 是硬互斥的两条通道。
///
/// 判定 fail-closed：同时为 true 时拒绝执行，而不是静默按其中一条通道执行：
/// 静默分流会让模型拿着一个它以为存在、实际并未创建的 ID 继续操作。
fn validate_execution_mode(input: &ShellCommandExecutionInput) -> AxonResult<()> {
    if input.run_in_background && input.tty {
        return Err(AxonError::tool_invalid_arguments(
            SHELL_COMMAND_BACKGROUND_TTY_CONFLICT_ERROR,
        ));
    }
    Ok(())
}

fn validate_command(command: &[String]) -> AxonResult<()> {
    let Some(program) = command.first() else {
        return Err(AxonError::tool_invalid_arguments(SHELL_COMMAND_EMPTY_ERROR));
    };
    if shell_program_denied(command, program) {
        return Err(AxonError::tool_invalid_arguments(UNTRUSTED_SHELL_ERROR));
    }
    ShellCommandStaticAnalyzer::validate_command(command)?;
    Ok(())
}

fn validate_write_stdin_input(input: &WriteStdinExecutionInput) -> AxonResult<()> {
    if input.owner_id.trim().is_empty() {
        return Err(AxonError::tool_invalid_arguments(
            SHELL_COMMAND_OWNER_EMPTY_ERROR,
        ));
    }
    if input.session_id.trim().is_empty() {
        return Err(AxonError::tool_invalid_arguments(
            WRITE_STDIN_SESSION_EMPTY_ERROR,
        ));
    }
    if input.ctrl_c && input.chars.is_some() {
        return Err(AxonError::tool_invalid_arguments(
            WRITE_STDIN_INPUT_CONFLICT_ERROR,
        ));
    }
    Ok(())
}

fn canonicalize_cwd(cwd: &Path) -> AxonResult<PathBuf> {
    cwd.canonicalize()
        .map_err(|_| AxonError::tool_invalid_arguments(SHELL_COMMAND_CWD_ERROR))
}

#[cfg(test)]
#[path = "builtin_shell_tool_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "builtin_shell_tool_artifact_tests.rs"]
mod artifact_tests;

#[cfg(test)]
#[path = "builtin_shell_tool_env_tests.rs"]
mod env_tests;

#[cfg(test)]
#[path = "builtin_shell_tool_session_lifecycle_tests.rs"]
mod session_lifecycle_tests;

#[cfg(test)]
#[path = "builtin_shell_tool_background_tests.rs"]
mod background_tests;
