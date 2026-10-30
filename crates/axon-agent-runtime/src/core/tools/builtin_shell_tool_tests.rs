use super::*;
use axon_domain::ToolCallInput;
use std::fs;
use std::{
    thread,
    time::{Duration, Instant},
};

#[path = "builtin_shell_tool_pty_readiness.rs"]
mod pty_readiness;
use pty_readiness::{HANG_GUARD, poll_until_exit, poll_until_ready};

/// PTY/tty 用例：终端语义与管道会话的观察点不同，单独成组以便定位。
#[path = "builtin_shell_tool_pty_tests.rs"]
mod pty_tests;

const SHORT_YIELD_MS: u64 = 100;
const COMPLETION_YIELD_MS: u64 = 3_000;
const TEST_OWNER_ID: &str = "shell-tool-tests";
/// 多行承载命令写出的唯一标记；命令与断言共用一处定义。
const CARRIER_MARKER: &str = "carrier-ok";
/// 终态回执补读输出的窗口：输出线程可能晚于子进程退出才把字节写进共享缓冲。
const OUTPUT_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
/// 上述补读的轮询间隔。
const OUTPUT_SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// 大输出用例的输出行数：显著超出输出预算即可验证截断与产物保全。
const LARGE_OUTPUT_LAST_LINE: usize = 300;

#[test]
fn shell_command_rejects_non_canonical_wrapper_at_execution_boundary() {
    let result = ShellCommandToolRuntime.execute(execution_input(vec![
        "pwsh.exe".to_owned(),
        "-c".to_owned(),
        "echo nope".to_owned(),
    ]));

    assert!(matches!(
        result,
        Err(AxonError::Tool(axon_domain::ToolError::InvalidArguments(reason)))
            if reason == UNTRUSTED_SHELL_ERROR
    ));
}

#[test]
fn shell_schemas_require_positive_output_token_budget() {
    let mut registry = ToolRegistry::new();
    registry
        .register(shell_command_tool_definition())
        .expect("shell_command 应注册成功");
    registry
        .register(write_stdin_tool_definition())
        .expect("write_stdin 应注册成功");
    let shell_command_qualified_name = format!(
        "{}.{}",
        crate::AGENT_TOOL_NAMESPACE,
        SHELL_COMMAND_TOOL_NAME
    );
    let write_stdin_qualified_name =
        format!("{}.{}", crate::AGENT_TOOL_NAMESPACE, WRITE_STDIN_TOOL_NAME);

    assert!(
        registry
            .validate_call_input(
                &shell_command_qualified_name,
                &ToolCallInput::json(serde_json::json!({
                    "command": "echo axon",
                    "max_output_tokens": 0
                }))
            )
            .is_err()
    );
    assert!(
        registry
            .validate_call_input(
                &shell_command_qualified_name,
                &ToolCallInput::json(serde_json::json!({
                    "command": "echo axon",
                    "intent": "验证命令行为",
                    "max_output_tokens": 1
                }))
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_call_input(
                &write_stdin_qualified_name,
                &ToolCallInput::json(serde_json::json!({
                    "session_id": "session-1",
                    "max_output_tokens": 0
                }))
            )
            .is_err()
    );
}

#[test]
fn short_shell_command_completes_and_still_reports_session_id() {
    let receipt = ShellCommandToolRuntime
        .execute(execution_input(test_command()))
        .unwrap();

    assert_eq!(
        receipt.status,
        ShellCommandExecutionStatus::Completed,
        "{receipt:?}"
    );
    assert_eq!(receipt.exit_code, Some(0));
    // 终态会话在保留窗口内仍可读输出与退出码，因此回执必须给出 ID；
    // 仅在 Running 时返回会让终态输出无法通过 write_stdin 取回。
    assert!(
        receipt.session_id.is_some(),
        "终态回执也必须返回 session_id：{receipt:?}"
    );
    assert_eq!(receipt.output_handler_used, "head_tail");
    assert!(receipt.full_output_path.is_some());
    assert!(receipt.stdout.preview.contains("rustc"));
}

/// 对齐 DSH 的端到端回归：多行承载形式必须真的能跑通。
///
/// 修复前 PowerShell here-string 会在权限判定层被判为「Unix shell 不接受 PowerShell
/// here-string」直接拒绝，Bash heredoc 又被方言预检拒绝：模型两种写法都被挡回单行
/// `python -c`，中文与引号随即被 shell 拆成独立 token。
#[test]
fn multiline_script_carrier_executes_end_to_end() {
    let mut receipt = ShellCommandToolRuntime
        .execute(execution_input(multiline_carrier_command()))
        .unwrap();
    let session_id = receipt
        .session_id
        .clone()
        .expect("多行承载命令必须返回 session_id");
    // 首次让出窗口只决定「先看到多少」，不决定成败：先把首段增量留下，再按子进程状态
    // 等到退出。轮询接口每次只返回增量，各段必须逐次累加，否则中途到达的输出会被丢掉。
    let mut stdout = receipt.stdout.preview.clone();
    if receipt.status == ShellCommandExecutionStatus::Running {
        receipt = poll_until_exit(&session_id);
        stdout.push_str(&receipt.stdout.preview);
    }

    assert_eq!(
        receipt.status,
        ShellCommandExecutionStatus::Completed,
        "{receipt:?}"
    );
    assert_eq!(receipt.exit_code, Some(0), "{receipt:?}");
    // 终态回执仍可能早于输出线程收口：会话在保留窗口内可继续取增量，故再补读到标记出现。
    stdout.push_str(&read_stdout_until_marker(
        &session_id,
        CARRIER_MARKER,
        &receipt,
    ));
    assert!(
        stdout.contains(CARRIER_MARKER),
        "多行承载的脚本必须真的执行：{receipt:?}"
    );
}

/// 等到会话输出里出现 `marker`，返回**累积**到的 stdout。
///
/// 终态回执只取一次增量：输出线程若晚于子进程退出才把字节写进共享缓冲（并行门禁下
/// 更容易发生），一次性终态回执就会是空的。会话在保留窗口内仍可继续读取增量，因此这里
/// 以「标记出现」为条件做有界等待，而不是拿单次回执当结论。
fn read_stdout_until_marker(
    session_id: &str,
    marker: &str,
    first: &ShellCommandExecutionReceipt,
) -> String {
    let started = Instant::now();
    let mut collected = first.stdout.preview.clone();
    while !collected.contains(marker) && started.elapsed() < OUTPUT_SETTLE_TIMEOUT {
        thread::sleep(OUTPUT_SETTLE_POLL_INTERVAL);
        let receipt = ShellCommandToolRuntime
            .write_stdin(write_input(session_id, None, false, 0))
            .expect("终态会话在保留窗口内必须仍可读取");
        collected.push_str(&receipt.stdout.preview);
    }
    collected
}

#[cfg(windows)]
fn multiline_carrier_command() -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(&format!("@'\n{CARRIER_MARKER}\n'@"), false)
        .unwrap()
}

#[cfg(unix)]
fn multiline_carrier_command() -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        &format!("cat <<'EOF'\n{CARRIER_MARKER}\nEOF"),
        false,
    )
    .unwrap()
}

#[test]
fn persistent_session_supports_polling_and_stdin() {
    let mut input = execution_input(interactive_command(false));
    input.yield_time_ms = SHORT_YIELD_MS;
    let running = ShellCommandToolRuntime.execute(input).unwrap();
    let session_id = running.session_id.expect("长命令必须返回 session_id");
    assert_eq!(running.status, ShellCommandExecutionStatus::Running);

    let polled = ShellCommandToolRuntime
        .write_stdin(write_input(&session_id, None, false, 0))
        .unwrap();
    assert_eq!(polled.status, ShellCommandExecutionStatus::Running);
    assert_eq!(
        polled.interaction.as_ref().map(|value| value.kind),
        Some(ShellCommandInteractionKind::Wait)
    );

    let echoed = ShellCommandToolRuntime
        .write_stdin(write_input(
            &session_id,
            Some(input_newline("axon-input")),
            false,
            SHORT_YIELD_MS,
        ))
        .unwrap();
    assert_eq!(
        echoed.interaction.as_ref().map(|value| value.kind),
        Some(ShellCommandInteractionKind::Input)
    );
    // 回显可能已在上一次让出窗口里到达，也可能更晚：把已读走的增量作为累积起点，
    // 再以零让出窗口轮询到可见，两种时序都不会丢。
    let visible = poll_until_ready(&session_id, HANG_GUARD, Some(&echoed), |receipt| {
        receipt.stdout.preview.contains("got:axon-input")
    });
    assert!(
        visible.stdout.preview.contains("got:axon-input"),
        "{visible:?}"
    );
}

#[test]
fn pipe_ctrl_c_terminates_persistent_process() {
    let mut input = execution_input(sleep_command(false));
    input.yield_time_ms = SHORT_YIELD_MS;
    let running = ShellCommandToolRuntime.execute(input).unwrap();
    let session_id = running.session_id.expect("睡眠命令必须保持运行");

    let mut cancelled = ShellCommandToolRuntime
        .write_stdin(write_input(&session_id, None, true, SHORT_YIELD_MS))
        .unwrap();
    // 同一条纪律：Ctrl+C 之后等子进程自己的退出，不预测耗时。
    if cancelled.status == ShellCommandExecutionStatus::Running {
        cancelled = poll_until_exit(&session_id);
    }

    assert_eq!(cancelled.status, ShellCommandExecutionStatus::Completed);
    assert_eq!(cancelled.exit_code, Some(SHELL_COMMAND_CANCELLED_EXIT_CODE));
    assert!(cancelled.cancelled);
    assert_eq!(
        cancelled.interaction.as_ref().map(|value| value.kind),
        Some(ShellCommandInteractionKind::Interrupt)
    );
}

#[test]
fn explicit_timeout_terminates_process() {
    let mut input = execution_input(sleep_command(false));
    input.timeout_ms = Some(crate::runtime_limits::MIN_SHELL_TIMEOUT_MS);
    input.yield_time_ms = COMPLETION_YIELD_MS;

    let receipt = ShellCommandToolRuntime.execute(input).unwrap();

    assert_eq!(receipt.status, ShellCommandExecutionStatus::Failed);
    assert_eq!(receipt.exit_code, Some(SHELL_COMMAND_TIMEOUT_EXIT_CODE));
    assert!(receipt.timed_out);
}

#[test]
fn large_output_is_bounded_and_preserved_in_artifact() {
    const MAX_OUTPUT_TOKENS: usize = 64;
    const FINAL_OUTPUT: &str = "shell-output-finished\n";

    let archive_dir = axon_test_support::test_temp_dir();
    let artifact_path = archive_dir.path().join("shell-output.log");
    let output = output_buffer::SharedProcessOutput::new(
        fs::File::create(&artifact_path).expect("临时归档文件应创建成功"),
        crate::runtime_limits::SHELL_ARTIFACT_MAX_BYTES,
    );
    for index in 1..=LARGE_OUTPUT_LAST_LINE {
        output.append(
            ShellCommandOutputStream::Stdout,
            format!("axon-output-{index}\n").as_bytes(),
        );
    }

    let first = output.take_receipt(Some(MAX_OUTPUT_TOKENS));
    assert!(first.stdout.truncated);
    assert!(first.stdout.preview.contains("output omitted"));
    assert!(first.stdout.retained_chars.div_ceil(APPROX_CHARS_PER_TOKEN) <= MAX_OUTPUT_TOKENS);

    // 回执消费增量缓冲；最后一批短输出不截断，但归档必须保留所有批次。
    output.append(ShellCommandOutputStream::Stdout, FINAL_OUTPUT.as_bytes());
    let last = output.take_receipt(Some(MAX_OUTPUT_TOKENS));
    assert!(!last.stdout.truncated);
    assert_eq!(last.stdout.preview, FINAL_OUTPUT);
    assert!(
        output
            .take_receipt(Some(MAX_OUTPUT_TOKENS))
            .stdout
            .preview
            .is_empty()
    );
    assert!(!output.artifact_failed());
    let artifact = fs::read_to_string(&artifact_path).expect("完整归档应可读取");
    assert!(artifact.contains("axon-output-1"));
    assert!(artifact.contains(&format!("axon-output-{LARGE_OUTPUT_LAST_LINE}")));
    assert!(artifact.contains(FINAL_OUTPUT));
}

#[test]
fn shell_command_keeps_remote_execution_closed() {
    let mut input = execution_input(test_command());
    input.remote_requested = true;

    let receipt = ShellCommandToolRuntime.execute(input).unwrap();

    assert_eq!(receipt.status, ShellCommandExecutionStatus::Blocked);
    assert_eq!(receipt.exit_code, None);
}

#[test]
fn owner_cleanup_terminates_only_matching_sessions() {
    // 零让出窗口让两个长命令的启动窗口重叠，清理语义不受影响。
    let mut first_input = execution_input(sleep_command(false));
    first_input.owner_id = "owner-a".to_owned();
    first_input.yield_time_ms = 0;
    let first = ShellCommandToolRuntime.execute(first_input).unwrap();
    let first_id = first.session_id.expect("owner-a 命令必须保持运行");

    let mut second_input = execution_input(sleep_command(false));
    second_input.owner_id = "owner-b".to_owned();
    second_input.yield_time_ms = 0;
    let second = ShellCommandToolRuntime.execute(second_input).unwrap();
    let second_id = second.session_id.expect("owner-b 命令必须保持运行");

    let mut wrong_owner = write_input(&first_id, None, false, 0);
    wrong_owner.owner_id = "owner-b".to_owned();
    let error = ShellCommandToolRuntime
        .write_stdin(wrong_owner)
        .unwrap_err();
    assert!(matches!(
        error,
        AxonError::Tool(axon_domain::ToolError::ExecutionFailed(reason))
            if reason == SHELL_SESSION_OWNER_ERROR
    ));
    assert_eq!(
        ShellCommandToolRuntime::terminate_sessions_for_owner("owner-a"),
        1
    );
    let mut removed = write_input(&first_id, None, false, 0);
    removed.owner_id = "owner-a".to_owned();
    let error = ShellCommandToolRuntime.write_stdin(removed).unwrap_err();
    assert!(matches!(
        error,
        AxonError::Tool(axon_domain::ToolError::ExecutionFailed(reason))
            if reason == SHELL_SESSION_NOT_FOUND_ERROR
    ));

    let mut second_poll = write_input(&second_id, None, false, 0);
    second_poll.owner_id = "owner-b".to_owned();
    let second = ShellCommandToolRuntime.write_stdin(second_poll).unwrap();
    assert_eq!(second.status, ShellCommandExecutionStatus::Running);
    assert_eq!(
        ShellCommandToolRuntime::terminate_sessions_for_owner("owner-b"),
        1
    );
}

fn execution_input(command: Vec<String>) -> ShellCommandExecutionInput {
    ShellCommandExecutionInput {
        owner_id: TEST_OWNER_ID.to_owned(),
        command,
        cwd: std::env::current_dir().unwrap(),
        timeout_ms: None,
        yield_time_ms: COMPLETION_YIELD_MS,
        tty: false,
        run_in_background: false,
        max_output_tokens: None,
        remote_requested: false,
        tee_root: Some(axon_test_support::test_temp_root()),
        cancel_token: None,
        write_paths: Vec::new(),
    }
}

fn write_input(
    session_id: &str,
    chars: Option<String>,
    ctrl_c: bool,
    yield_time_ms: u64,
) -> WriteStdinExecutionInput {
    WriteStdinExecutionInput {
        owner_id: TEST_OWNER_ID.to_owned(),
        session_id: session_id.to_owned(),
        chars,
        ctrl_c,
        yield_time_ms,
        max_output_tokens: None,
        remote_requested: false,
    }
}

fn input_newline(value: &str) -> String {
    #[cfg(windows)]
    {
        format!("{value}\r\n")
    }
    #[cfg(unix)]
    {
        format!("{value}\n")
    }
}

fn test_command() -> Vec<String> {
    vec!["rustc".to_owned(), "-V".to_owned()]
}

#[cfg(windows)]
fn interactive_command(tty: bool) -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        "while (($line = [Console]::In.ReadLine()) -ne $null) { Write-Output \"got:$line\"; if ($line -eq 'quit') { break } }",
        tty,
    )
    .unwrap()
}

#[cfg(unix)]
fn interactive_command(tty: bool) -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        "while IFS= read -r line; do echo \"got:$line\"; [ \"$line\" = quit ] && break; done",
        tty,
    )
    .unwrap()
}

#[cfg(windows)]
fn sleep_command(tty: bool) -> Vec<String> {
    ShellCommandToolRuntime::command_from_script("Start-Sleep -Seconds 30", tty).unwrap()
}

#[cfg(unix)]
fn sleep_command(tty: bool) -> Vec<String> {
    ShellCommandToolRuntime::command_from_script("sleep 30", tty).unwrap()
}

#[test]
fn shell_cross_call_guidance_lives_in_the_fragment_not_the_schema() {
    let tool = shell_command_tool_definition();
    let guidance = axon_prompts::render_tool_usage_guidance();

    // 双向断言：既确认已迁出 schema，也确认没有在迁移中丢失语义。
    for phrase in [
        "在外层嵌套",
        "两种通道的 ID 不可互传",
        "会话在结束 5 分钟后回收",
    ] {
        assert!(
            !tool.description.contains(phrase),
            "跨调用约束「{phrase}」只由稳定 developer 片段承载，不得回流到工具 description"
        );
    }
    assert!(
        guidance.contains("在外层嵌套 `bash -c`、`zsh -c`、`powershell` 或 `cmd /c`"),
        "命令形态规约必须保留在 <tool_execution_and_receipt_protocol> 片段中"
    );
    assert!(
        !tool
            .description
            .contains("<tool_execution_and_receipt_protocol>"),
        "工具 description 不得引用内部片段标签，形态规约只由稳定 developer 片段承载"
    );
    // 参数形态仍属单次调用语义，必须留在 schema。
    assert!(
        tool.description.contains("JSON 字符串内的双引号要转义"),
        "参数转义规则属参数语义，必须留在 schema"
    );
}

#[test]
fn shell_intent_split_keeps_product_semantics_out_of_the_schema() {
    let tool = shell_command_tool_definition();
    let guidance = axon_prompts::render_tool_usage_guidance();
    let intent_description = tool.schema.input_schema["properties"]
        [SHELL_COMMAND_TOOL_INTENT_FIELD]["description"]
        .as_str()
        .expect("intent 字段必须声明 description");

    // 必填是模型可见契约：每次调用都拿到目的说明，不靠模型自觉。
    assert!(
        tool.schema
            .required_field_names()
            .iter()
            .any(|field| field == SHELL_COMMAND_TOOL_INTENT_FIELD),
        "intent 必须是必填字段，否则折叠态会退回命令截取"
    );
    // 字段描述只讲这条参数怎么写：写目标、不复述命令、长度与标点。
    for phrase in ["写目标而不是复述命令", "不超过 40 字", "不加句号"] {
        assert!(
            intent_description.contains(phrase),
            "intent 字段描述必须写清单次调用的写法：{phrase}"
        );
    }
    // 产品态语义（这次执行面向用户的说明）属跨调用承诺，只由稳定 developer 片段承载。
    for phrase in ["每次调用还必须给出 `intent`", "它是这次执行面向用户的说明"]
    {
        assert!(
            guidance.contains(phrase),
            "intent 的产品态语义必须留在 <tool_execution_and_receipt_protocol> 片段：{phrase}"
        );
    }
    // 界面机制不进模型可见契约：折叠/展开是某一端的呈现细节，不是产品态事实。
    for phrase in ["折叠态", "展开态", "用户默认只看到"] {
        assert!(
            !intent_description.contains(phrase)
                && !tool.description.contains(phrase)
                && !guidance.contains(phrase),
            "界面机制「{phrase}」不得进入工具 schema、description 或稳定 developer 片段"
        );
    }
}

#[test]
fn background_field_description_states_the_default_selection_rule() {
    let tool = shell_command_tool_definition();
    let field_description = tool.schema.input_schema["properties"]
        [SHELL_COMMAND_TOOL_RUN_IN_BACKGROUND_FIELD]["description"]
        .as_str()
        .expect("run_in_background 字段必须声明 description");

    // 选型判据属单次调用语义：后台是默认，同步等待是显式例外。
    for phrase in [
        "默认设为 true",
        "主线无需立即依赖该结果",
        "仅当下一步行动强依赖该结果时才设为 false",
    ] {
        assert!(
            field_description.contains(phrase),
            "run_in_background 字段描述必须写明默认分流判据：{phrase}"
        );
    }
    // 跨调用约束不得回流到 schema，字段描述也不能引用内部片段标签。
    for phrase in ["Busy-poll", "回流到后续轮次", "统一收口"] {
        assert!(
            !field_description.contains(phrase),
            "跨调用收口规约「{phrase}」只由 <tool_execution_and_receipt_protocol> 片段承载"
        );
    }
    assert!(
        !field_description.contains("<tool_execution_and_receipt_protocol>"),
        "字段描述不得引用内部片段标签"
    );
}
