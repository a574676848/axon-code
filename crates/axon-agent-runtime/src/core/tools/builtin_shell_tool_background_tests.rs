//! `shell_command.run_in_background` 的硬互斥与作业分流回归。
//!
//! 覆盖模型实际能观察到的结局：`run_in_background` 与 `tty` 同时为 true 时被
//! fail-closed 拒绝；只设 `run_in_background` 时命令真正转为后台作业，且
//! 作业 ID 永远不会被 Shell 会话通道接受。

use super::*;
use crate::JobStatus;

/// 回收测试作业：`echo background` 可能在清理前已自然终态，此时生产终止通道
/// `kill` 会报告「已进入终态」：该错误同样证明作业曾被登记、回收目标已达成；
/// 只有「不存在 / 不属于当前 run」类错误才意味着登记失败。
fn reap_test_job(job_id: JobId, owner: &str) {
    // 先读一次触发终态标记：进程可能已退出但尚未被 poll，直接 terminate 会
    // 撞上平台层的「进程已退出」OS 错误，而非语义化的「已进入终态」。
    let _ = JobRuntime::global().read(job_id, owner, Some(0));
    if let Err(error) = JobRuntime::global().kill(job_id, owner) {
        let detail = format!("{error:?}");
        assert!(
            detail.contains("已进入终态"),
            "作业清理只容忍「已自然终态」，不得报告其他错误：{detail}"
        );
    }
}

/// 立即结束的命令；用于确认后台作业确实被启动。
const BACKGROUND_COMMAND: &str = "echo background";
/// 首次读取作业的等待窗口。
const BACKGROUND_YIELD_MS: u64 = 3_000;

/// 仅设 `run_in_background` 时必须被接受：否则「一律拒绝」也能骗过互斥用例。
#[test]
fn background_without_tty_starts_a_job() {
    // 真实派生子进程的用例走同一闸门：并行门禁下同时拉起的 pwsh 会把进程创建挤到失败边缘。
    let _gate = super::spawn_gate::acquire();
    let owner = "shell-background-accepted";
    let input = background_input(owner, BACKGROUND_COMMAND);

    let job_id = ShellCommandToolRuntime
        .start_background(input)
        .expect("仅设 run_in_background 启动作业必须成功");

    assert!(job_id.0 > 0, "作业 ID 必须是有效的十进制标识");
    let output = JobRuntime::global()
        .read(job_id, owner, Some(BACKGROUND_YIELD_MS))
        .expect("刚启动的作业必须可按 owner 读取");
    assert_eq!(output.job_id, job_id);
    // 用例结束时经生产终止通道回收自己启动的作业。
    reap_test_job(job_id, owner);
}

/// `run_in_background` 与 `tty` 同时为 true 时必须拒绝，且原因是互斥本身。
#[test]
fn background_conflicts_with_tty() {
    let mut input = background_input("shell-background-conflict", BACKGROUND_COMMAND);
    input.tty = true;

    let error = ShellCommandToolRuntime
        .start_background(input)
        .expect_err("后台作业与交互会话是互斥通道，必须 fail-closed 拒绝");

    assert!(
        matches!(
            &error,
            AxonError::Tool(axon_domain::ToolError::InvalidArguments(reason))
                if reason.as_str() == SHELL_COMMAND_BACKGROUND_TTY_CONFLICT_ERROR
        ),
        "拒绝原因必须是互斥说明，而不是其他参数错误：{error:?}"
    );
}

/// 前台入口不得消费 `run_in_background`，否则模型会拿到一个并不存在的作业。
#[test]
fn foreground_entry_rejects_the_background_flag() {
    let mut input = background_input("shell-background-foreground-entry", BACKGROUND_COMMAND);
    input.run_in_background = true;

    let error = ShellCommandToolRuntime
        .execute(input)
        .expect_err("run_in_background 只能经 start_background 消费");

    assert!(matches!(
        error,
        AxonError::Tool(axon_domain::ToolError::InvalidArguments(_))
    ));
}

/// 后台入口不得消费前台输入，避免两条通道被同一个调用同时走通。
#[test]
fn background_entry_requires_the_background_flag() {
    let mut input = background_input("shell-background-background-entry", BACKGROUND_COMMAND);
    input.run_in_background = false;

    let error = ShellCommandToolRuntime
        .start_background(input)
        .expect_err("run_in_background 为 false 时不得启动作业");

    assert!(matches!(
        error,
        AxonError::Tool(axon_domain::ToolError::InvalidArguments(_))
    ));
}

/// 禁止双通道：作业 ID 送入 `write_stdin` 必须报告「不存在」，而不是被当成会话。
#[test]
fn job_id_is_never_accepted_by_the_session_channel() {
    // 与同模块的派生用例互斥：作业登记依赖真实 spawn 成功，不能被并发派生挤掉。
    let _gate = super::spawn_gate::acquire();
    let owner = "shell-background-cross-channel";
    let job_id = ShellCommandToolRuntime
        .start_background(background_input(owner, BACKGROUND_COMMAND))
        .expect("后台作业应启动成功");

    let error = ShellCommandToolRuntime
        .write_stdin(WriteStdinExecutionInput {
            owner_id: owner.to_owned(),
            session_id: job_id.to_string(),
            chars: None,
            ctrl_c: false,
            yield_time_ms: 0,
            max_output_tokens: None,
            remote_requested: false,
        })
        .expect_err("作业 ID 不得被 write_stdin 接受");

    assert!(
        format!("{error:?}").contains(SHELL_SESSION_NOT_FOUND_ERROR),
        "作业 ID 在会话通道里必须表现为从未存在过的会话：{error:?}"
    );
    reap_test_job(job_id, owner);
}

/// run 拆除**不得**回收后台作业（与 DSH 对齐）。
///
/// DSH 把作业生命周期绑定在 **owner 的 scope** 上，而不是 turn 上：
/// `jobs.ownerCleanup()` 在 owner scope 销毁时
/// 「cancels live jobs, awaits settlement, drops snapshots」
/// （`packages/jobs/jobs-local`），并在 owner 注册时**只挂一次**。
///
/// Axon 的 run 拆除路径调用 `terminate_sessions_for_owner` 清理**交互会话**；
/// 作业必须跨 run 存活，否则「启动长任务 → 后续轮次读取」这一本功能的核心
/// 用法直接失效（作业刚启动就被自己所在的 run 杀掉）。
///
/// 本用例的价值在于防止后人「顺手」把作业回收补进 run 拆除路径：
/// 那种改动编译通过、多数测试也通过，只会静默废掉整个后台能力。
#[test]
fn run_teardown_of_sessions_leaves_background_jobs_alive() {
    let _gate = super::spawn_gate::acquire();
    let owner = "shell-background-run-teardown";

    let job_id = ShellCommandToolRuntime
        .start_background(background_input(owner, BACKGROUND_COMMAND))
        .expect("必须能启动后台作业");

    // 模拟 run 结束时的拆除：它只清理交互会话。
    ShellCommandToolRuntime::terminate_sessions_for_owner(owner);

    let output = JobRuntime::global()
        .read(job_id, owner, Some(BACKGROUND_YIELD_MS))
        .expect("run 拆除后作业必须仍然可读，否则后台任务无法跨轮使用");
    assert_eq!(output.job_id, job_id);

    // 反例对照：作业通道**自身**的终止入口确实能终止它。
    // 缺了这一半，上面那句在「作业压根没被登记」的实现下也会通过。
    // 「已进入终态」同样证明登记成功：「从未存在」才是登记缺失的症状。
    reap_test_job(job_id, owner);
    // 终止后的作业进入保留窗口内的终态：仍可读取（与 job_kill 工具的保留窗口
    // 语义一致）。`echo background` 可能早于 reap 自然完成（Completed），也可能
    // 被 reap 终止（Killed）：两者都是「已登记且终态」；Running 或读取缺失
    // （「回收为不可见」是移除式回收的行为，会话通道才是这个语义）才是失败。
    let output = JobRuntime::global()
        .read(job_id, owner, Some(BACKGROUND_YIELD_MS))
        .expect("终止后的作业在保留窗口内必须仍可读终态");
    assert!(
        matches!(output.status, JobStatus::Killed | JobStatus::Completed),
        "读取到的必须是终态（被终止或自然完成）：{:?}",
        output.status
    );
}

fn background_input(owner_id: &str, script: &str) -> ShellCommandExecutionInput {
    ShellCommandExecutionInput {
        owner_id: owner_id.to_owned(),
        command: ShellCommandToolRuntime::command_from_script(script, false)
            .expect("命令应规范化成功"),
        cwd: std::env::current_dir().expect("测试工作目录应可解析"),
        timeout_ms: None,
        yield_time_ms: 0,
        tty: false,
        run_in_background: true,
        max_output_tokens: None,
        tee_root: Some(axon_test_support::test_temp_root()),
        cancel_token: None,
        write_paths: Vec::new(),
        remote_requested: false,
    }
}
