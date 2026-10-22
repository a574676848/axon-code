//! Shell JobRuntime 行为基线测试（合同 AXON-DSH-JOB-001 §5.1.1）。
//!
//! 本文件建立**现状行为**的回归底线：M2 把 Shell 作业改造为第一个 JobProducer、
//! 并按 700 行红线拆分本模块时，下列语义必须逐条保持不变。测试只锁定既有事实，
//! 不引入测试专用生产分支，也不改变任何运行时行为。
//!
//! 隔离策略：`reap_once` 以注入的 `now` / `terminal_retention` / `idle_timeout`
//! 为唯一判据（源码注释明确「便于确定性测试」），因此回收类断言全部在
//! **不挂回收线程的隔离运行时**上执行：全局单例的回收线程按真实时间推进，
//! 无法注入时钟，且对它注入未来时刻会误伤并行用例的作业。

use super::*;
use crate::builtin_shell_tool::ShellCommandToolRuntime;
use crate::job_runtime::JOB_REASON_USER_STOP;

/// 立即结束的命令；用于确认作业确实被启动并进入终态。
const TERMINAL_COMMAND: &str = "echo job-baseline";
/// 首次读取的等待窗口：足够本地短命令结束。
const READ_YIELD_MS: u64 = 5_000;
/// 不等待，仅取当前增量与状态的读取窗口。
const NO_WAIT_MS: u64 = 0;

/// 构造隔离运行时：注册表与墓碑独立，且**不启动回收线程**。
///
/// 直接构造结构体而非调用 `new()`，是为了让回收断言完全由注入的时钟决定；
/// 这也是 M2 拆分注册表时唯一需要同步调整的测试脚手架。
fn isolated_runtime() -> JobRuntime {
    JobRuntime {
        jobs: Arc::new(Mutex::new(HashMap::new())),
        tombstones: Arc::new(Mutex::new(VecDeque::new())),
        next_id: AtomicU64::new(1),
    }
}

/// 构造作业规格；owner 与命令由调用方给出。
fn job_spec(owner: &str, script: &str) -> JobSpec {
    JobSpec {
        owner_id: owner.to_owned(),
        label: script.to_owned(),
        command: ShellCommandToolRuntime::command_from_script(script, false)
            .expect("测试命令应规范化成功"),
        cwd: std::env::current_dir().expect("测试工作目录应可解析"),
        timeout_ms: None,
        cancel_token: None,
        tee_root: Some(axon_test_support::test_temp_root()),
    }
}

/// 启动作业并等待其进入终态，返回作业 ID。
fn start_and_settle(runtime: &JobRuntime, owner: &str, script: &str) -> JobId {
    let job_id = runtime
        .start(job_spec(owner, script))
        .expect("启动作业必须成功");
    let output = runtime
        .read(job_id, owner, Some(READ_YIELD_MS))
        .expect("刚启动的作业必须可按 owner 读取");
    assert!(
        matches!(output.status, JobStatus::Completed | JobStatus::Failed),
        "短命令必须在等待窗口内进入终态：{:?}",
        output.status
    );
    job_id
}

/// 超时命令：用于验证终止路径作用在**仍在运行**的作业上。
#[cfg(windows)]
const SLEEP_COMMAND: &str = "Start-Sleep -Seconds 30";
/// 超时命令：用于验证终止路径作用在**仍在运行**的作业上。
#[cfg(unix)]
const SLEEP_COMMAND: &str = "sleep 30";

/// 把错误渲染为可断言的调试文本；错误说明是回执的一部分，必须稳定。
fn error_text(error: &AxonError) -> String {
    format!("{error:?}")
}

// ---------- 父 turn 结束即释放终态 ----------

/// 父 turn 结束后，终态作业立即释放：读取区分「已回收」，运行中作业不受影响。
#[test]
fn release_terminal_for_owner_drops_finished_jobs_immediately() {
    let runtime = isolated_runtime();
    let owner = "turn-end-release";
    let finished = start_and_settle(&runtime, owner, TERMINAL_COMMAND);
    let running = runtime
        .start(job_spec(owner, SLEEP_COMMAND))
        .expect("启动运行中作业");
    // 越过一个 reaper 周期：终态通知已由回收线程投递，可以安全释放。
    std::thread::sleep(JOB_REAPER_INTERVAL + Duration::from_millis(50));
    let released = runtime.release_terminal_for_owner(owner, Instant::now());
    assert_eq!(released, 1, "只应释放终态作业");
    let error = runtime
        .read(finished, owner, Some(NO_WAIT_MS))
        .expect_err("已释放作业不得再按 owner 读取");
    assert!(
        error_text(&error).contains("超出保留窗口"),
        "释放必须登记墓碑以区分「已回收」与「从未存在」：{}",
        error_text(&error)
    );
    let running_output = runtime
        .read(running, owner, Some(NO_WAIT_MS))
        .expect("运行中作业不得被释放路径误伤");
    assert!(
        matches!(running_output.status, JobStatus::Running),
        "运行中作业必须保持运行：{:?}",
        running_output.status
    );
}

/// 刚进入终态、尚未越过 reaper 周期的作业不被立即释放（避免吞掉完成通知）。
#[test]
fn freshly_terminal_jobs_are_left_to_the_reaper() {
    let runtime = isolated_runtime();
    let owner = "fresh-terminal";
    let job_id = start_and_settle(&runtime, owner, TERMINAL_COMMAND);
    let released = runtime.release_terminal_for_owner(owner, Instant::now());
    assert_eq!(released, 0, "不足一个 reaper 周期不得释放");
    let output = runtime
        .read(job_id, owner, Some(NO_WAIT_MS))
        .expect("未释放的作业仍应可读");
    assert!(
        matches!(output.status, JobStatus::Completed | JobStatus::Failed),
        "作业应保持终态可读：{:?}",
        output.status
    );
}

// ---------- 身份与入参校验 ----------

/// 作业 ID 必须从 1 开始单调递增：它是模型逐字复制的句柄。
#[test]
fn job_ids_start_at_one_and_increase_monotonically() {
    let runtime = isolated_runtime();
    let owner = "baseline-identity";

    let first = runtime.start(job_spec(owner, TERMINAL_COMMAND)).unwrap();
    let second = runtime.start(job_spec(owner, TERMINAL_COMMAND)).unwrap();

    assert_eq!(first, JobId(1), "首个作业 ID 必须是 1");
    assert_eq!(second, JobId(2), "作业 ID 必须单调递增");
    assert_eq!(first.to_string(), "1", "作业 ID 必须以十进制展示");
}

/// 空 owner 必须被拒绝：owner 由运行时注入，缺失意味着归属不可判定。
#[test]
fn empty_owner_is_rejected() {
    let runtime = isolated_runtime();
    let error = runtime
        .start(job_spec("   ", TERMINAL_COMMAND))
        .expect_err("空 owner 必须 fail-closed 拒绝");

    assert!(
        error_text(&error).contains(JOB_OWNER_EMPTY_ERROR),
        "拒绝原因必须是 owner 缺失：{}",
        error_text(&error)
    );
}

/// 空命令必须被拒绝：不产生无意义的进程与归档文件。
#[test]
fn empty_command_is_rejected() {
    let runtime = isolated_runtime();
    let mut spec = job_spec("baseline-empty-command", TERMINAL_COMMAND);
    spec.command = Vec::new();

    let error = runtime.start(spec).expect_err("空命令必须被拒绝");
    assert!(
        error_text(&error).contains(JOB_COMMAND_EMPTY_ERROR),
        "拒绝原因必须是命令缺失：{}",
        error_text(&error)
    );
}

// ---------- owner 隔离 ----------

/// 跨 owner 读取必须稳定拒绝，且不泄露状态或输出。
#[test]
fn cross_owner_read_is_rejected() {
    let runtime = isolated_runtime();
    let owner = "baseline-owner-read";
    let job_id = runtime.start(job_spec(owner, TERMINAL_COMMAND)).unwrap();

    let error = runtime
        .read(job_id, "baseline-owner-intruder", Some(NO_WAIT_MS))
        .expect_err("跨 owner 读取必须被拒绝");

    assert!(
        error_text(&error).contains(JOB_OWNER_ERROR),
        "拒绝原因必须是归属不符：{}",
        error_text(&error)
    );
}

/// 跨 owner 终止必须稳定拒绝，且不得真的终止作业。
#[test]
fn cross_owner_kill_is_rejected() {
    let runtime = isolated_runtime();
    let owner = "baseline-owner-kill";
    let job_id = runtime.start(job_spec(owner, TERMINAL_COMMAND)).unwrap();

    let error = runtime
        .kill(job_id, "baseline-owner-intruder")
        .expect_err("跨 owner 终止必须被拒绝");
    assert!(
        error_text(&error).contains(JOB_OWNER_ERROR),
        "拒绝原因必须是归属不符：{}",
        error_text(&error)
    );

    // 反例对照：真正的 owner 仍然可达，证明上面的拒绝没有造成副作用。
    runtime
        .read(job_id, owner, Some(READ_YIELD_MS))
        .expect("真正的 owner 必须仍可读取自己的作业");
}

// ---------- 读取与终态投影 ----------

/// 终态必须在保留窗口内可读，并携带退出码与完整归档标记。
#[test]
fn terminal_state_is_readable_with_exit_code_and_artifact() {
    let runtime = isolated_runtime();
    let owner = "baseline-terminal-read";
    let job_id = start_and_settle(&runtime, owner, TERMINAL_COMMAND);

    let output = runtime
        .read(job_id, owner, Some(NO_WAIT_MS))
        .expect("终态在保留窗口内必须可读");

    assert_eq!(output.job_id, job_id);
    assert_eq!(output.status, JobStatus::Completed, "echo 必须成功结束");
    assert_eq!(output.exit_code, Some(0), "成功结束必须带退出码 0");
    assert_eq!(output.label, TERMINAL_COMMAND, "回执必须回带作业标签");
    assert!(
        output.full_output_available,
        "短输出不得被判定为截断或归档失败"
    );
    assert!(
        output.artifact_path.exists(),
        "作业必须创建输出归档：{}",
        output.artifact_path.display()
    );
}

/// 标签回带必须有界：超长命令不得把回执撑爆。
#[test]
fn job_label_is_bounded_by_character_count() {
    let long_label = "头".repeat(JOB_LABEL_MAX_CHARS + 50);
    let bounded = bounded_job_label(&long_label);

    assert_eq!(
        bounded.chars().count(),
        JOB_LABEL_MAX_CHARS,
        "标签必须按字符数截断"
    );
    assert!(long_label.starts_with(&bounded), "截断必须保留原标签前缀");
}

/// 等待窗口必须收敛到上界：模型不能用一个超大 yield 卡死整批工具。
#[test]
fn read_yield_is_bounded_by_max_window() {
    assert_eq!(bounded_yield(None), Duration::ZERO, "缺省不得等待");
    assert_eq!(
        bounded_yield(Some(1_000)),
        Duration::from_millis(1_000),
        "界内取值必须原样保留"
    );
    assert_eq!(
        bounded_yield(Some(u64::MAX)),
        JOB_MAX_READ_YIELD,
        "超界取值必须收敛到上界"
    );
}

// ---------- 终止语义 ----------

/// 终止仍在运行的作业必须成功并投影为 Killed（无退出码）。
#[test]
fn kill_running_job_projects_killed_without_exit_code() {
    let runtime = isolated_runtime();
    let owner = "baseline-kill-running";
    let job_id = runtime.start(job_spec(owner, SLEEP_COMMAND)).unwrap();

    let status = runtime
        .kill(job_id, owner)
        .expect("终止运行中的作业必须成功");
    assert_eq!(status, JobStatus::Killed, "终止必须投影为 Killed");

    let output = runtime
        .read(job_id, owner, Some(NO_WAIT_MS))
        .expect("被终止的作业在保留窗口内仍可读");
    assert_eq!(output.status, JobStatus::Killed);
    assert_eq!(output.exit_code, None, "被终止的作业没有退出码");
}

/// 用户手动停止：终态原因必须是用户来源，且能解析出作业归属。
#[test]
fn user_stop_records_user_reason_and_owner_is_resolvable() {
    let runtime = isolated_runtime();
    let owner = "baseline-user-stop";
    let job_id = runtime.start(job_spec(owner, SLEEP_COMMAND)).unwrap();

    assert_eq!(
        runtime.owner_of(job_id).as_deref(),
        Some(owner),
        "宿主必须先能解析归属，才能把 owner 原样传回终止入口"
    );

    let status = runtime
        .kill_with_reason(job_id, owner, Some(JOB_REASON_USER_STOP))
        .expect("用户手动停止运行中的作业必须成功");
    assert_eq!(status, JobStatus::Killed);
    assert_eq!(
        runtime.terminal_reason(job_id).as_deref(),
        Some(JOB_REASON_USER_STOP),
        "终态原因必须与 Agent 自己发起的 job_kill 区分开"
    );
}

/// 对已终态作业重复终止必须报错而非二次终止。
#[test]
fn kill_terminal_job_is_rejected() {
    let runtime = isolated_runtime();
    let owner = "baseline-kill-terminal";
    let job_id = start_and_settle(&runtime, owner, TERMINAL_COMMAND);

    let error = runtime
        .kill(job_id, owner)
        .expect_err("已终态的作业不得被重复终止");
    assert!(
        error_text(&error).contains(JOB_KILLED_STATUS_ERROR),
        "拒绝原因必须是已进入终态：{}",
        error_text(&error)
    );
}

// ---------- 回收、墓碑与容量 ----------

/// 越过终态保留窗口后作业被回收，读取必须区分「已回收」与「从未存在」。
#[test]
fn job_is_reaped_after_terminal_retention_and_reported_as_expired() {
    let runtime = isolated_runtime();
    let owner = "baseline-retention";
    let job_id = start_and_settle(&runtime, owner, TERMINAL_COMMAND);

    // 注入越过保留窗口的时刻；墓碑以同一时刻登记，因此不会被同轮剪除。
    let reaped_at = Instant::now() + JOB_TERMINAL_RETENTION + Duration::from_secs(1);
    reap_once(
        &runtime.jobs,
        &runtime.tombstones,
        reaped_at,
        JOB_TERMINAL_RETENTION,
        JOB_MAX_READ_YIELD,
    );

    assert!(
        runtime
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "越过保留窗口的作业必须被移出注册表"
    );
    let error = runtime
        .read(job_id, owner, Some(NO_WAIT_MS))
        .expect_err("已回收的作业不得再可读");
    assert!(
        error_text(&error).contains(JOB_EXPIRED_ERROR),
        "已回收必须报告超出保留窗口：{}",
        error_text(&error)
    );

    // 从未存在的 ID 必须给出另一种原因，否则「曾存在但已回收」不可诊断。
    let never_existed = runtime
        .read(JobId(9_999), owner, Some(NO_WAIT_MS))
        .expect_err("不存在的作业不得可读");
    assert!(
        error_text(&never_existed).contains(JOB_NOT_FOUND_ERROR),
        "从未存在必须报告不存在：{}",
        error_text(&never_existed)
    );
}

/// 终态保留窗口内的作业不得被回收。
#[test]
fn job_inside_terminal_retention_is_kept() {
    let runtime = isolated_runtime();
    let owner = "baseline-retention-inside";
    let job_id = start_and_settle(&runtime, owner, TERMINAL_COMMAND);

    // 注入仍在保留窗口内的时刻：终态保留是「窗口」而不是「立即回收」。
    reap_once(
        &runtime.jobs,
        &runtime.tombstones,
        Instant::now() + JOB_TERMINAL_RETENTION / 2,
        JOB_TERMINAL_RETENTION,
        JOB_MAX_READ_YIELD,
    );

    runtime
        .read(job_id, owner, Some(NO_WAIT_MS))
        .expect("保留窗口内的终态必须仍可读");
}

/// 墓碑越过墓碑保留窗口后被剪除：区分能力本身也必须有界。
#[test]
fn tombstone_is_pruned_after_tombstone_retention() {
    let runtime = isolated_runtime();
    let owner = "baseline-tombstone-prune";
    let job_id = start_and_settle(&runtime, owner, TERMINAL_COMMAND);

    let reaped_at = Instant::now() + JOB_TERMINAL_RETENTION + Duration::from_secs(1);
    reap_once(
        &runtime.jobs,
        &runtime.tombstones,
        reaped_at,
        JOB_TERMINAL_RETENTION,
        JOB_MAX_READ_YIELD,
    );
    assert_eq!(
        runtime
            .tombstones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1,
        "回收必须留下墓碑"
    );

    // 再推进一个墓碑窗口：此时无名册条目，走仅剪枝的分支。
    reap_once(
        &runtime.jobs,
        &runtime.tombstones,
        reaped_at + JOB_TOMBSTONE_RETENTION + Duration::from_secs(1),
        JOB_TERMINAL_RETENTION,
        JOB_MAX_READ_YIELD,
    );
    assert!(
        runtime
            .tombstones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "越过墓碑保留窗口后墓碑必须被剪除"
    );

    // 墓碑剪除后同一 ID 退化为「从未存在」，这是有界区分能力的既定代价。
    let error = runtime
        .read(job_id, owner, Some(NO_WAIT_MS))
        .expect_err("剪除后不得再可读");
    assert!(
        error_text(&error).contains(JOB_NOT_FOUND_ERROR),
        "墓碑剪除后必须退化为不存在：{}",
        error_text(&error)
    );
}

/// 墓碑容量超限必须按 FIFO 淘汰最旧记录，容量不得无界增长。
#[test]
fn tombstone_capacity_evicts_oldest_first() {
    let runtime = isolated_runtime();
    let owner = "baseline-tombstone-capacity";
    let job_id = start_and_settle(&runtime, owner, TERMINAL_COMMAND);

    let reaped_at = Instant::now() + JOB_TERMINAL_RETENTION + Duration::from_secs(1);
    {
        // 预置满容量墓碑；登记时刻取 reaped_at，确保同轮剪枝不会先清掉它们。
        let mut tombstones = runtime
            .tombstones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for offset in 0..JOB_TOMBSTONE_CAPACITY {
            tombstones.push_back(JobTombstone {
                job_id: JobId(1_000 + offset as u64),
                retired_at: reaped_at,
            });
        }
    }

    reap_once(
        &runtime.jobs,
        &runtime.tombstones,
        reaped_at,
        JOB_TERMINAL_RETENTION,
        JOB_MAX_READ_YIELD,
    );

    let tombstones = runtime
        .tombstones
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        tombstones.len(),
        JOB_TOMBSTONE_CAPACITY,
        "容量必须被收敛到上界"
    );
    assert_eq!(
        tombstones.front().map(|tombstone| tombstone.job_id),
        Some(JobId(1_001)),
        "最旧墓碑必须被 FIFO 淘汰"
    );
    assert_eq!(
        tombstones.back().map(|tombstone| tombstone.job_id),
        Some(job_id),
        "最新回收的墓碑必须位于队尾"
    );
}
