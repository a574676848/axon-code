//! 资源围栏的行为验证：通过独立探针进程观测内核是否真的接受了限额。
//!
//! 断言分两层：`/proc/self/limits` 证明内核**接受**了限额，副作用（超限写入被信号
//! 终止、超限 open 失败）证明限额**被执行**。探针是 `src/bin/axon-process-fence-probe.rs`，
//! `CARGO_BIN_EXE_*` 保证被测的就是当前源码构建出的可执行文件。
//!
//! Unix 断言依赖 Linux 的 `/proc/self/limits`，因此按平台裁剪；
//! Windows 断言直接验证 Job Object 是否真的被内核执行。

#[cfg(windows)]
use std::process::Command;
#[cfg(target_os = "linux")]
use std::{io, process::Command};

#[cfg(target_os = "linux")]
use axon_process::resource_fence::{
    PROCESS_RLIMIT_AS_ENV, PROCESS_RLIMIT_FSIZE_ENV, PROCESS_RLIMIT_NOFILE_ENV,
    PROCESS_RLIMIT_NPROC_ENV, fence_pty_command,
};
use axon_process::resource_fence::{
    ProcessResourceLimits, configure_process_resource_limits, fence_spawned_child,
};

/// 探针二进制的编译期路径。
#[cfg(target_os = "linux")]
const PROBE_BINARY: &str = env!("CARGO_BIN_EXE_axon-process-fence-probe");

/// 探针读取限额的内核接口。
#[cfg(target_os = "linux")]
const PROC_LIMITS_PATH: &str = "/proc/self/limits";

/// 探针自检命令的退出码输出前缀。
#[cfg(target_os = "linux")]
const ACTION_EXIT_PREFIX: &str = "action-exit=";

/// 写入超出 `RLIMIT_FSIZE` 的目标文件路径。
#[cfg(target_os = "linux")]
const OVERSIZED_WRITE_PATH: &str = "/tmp/axon-process-fence-oversized.bin";

/// 自检命令回显子进程退出状态时使用的字段名。
#[cfg(target_os = "linux")]
const OVERSIZED_WRITE_STATUS_FIELD: &str = "dd-status=";

/// `SIGXFSZ` 在 shell 中的 `128 + signal` 表现形式（25 号信号）。
#[cfg(target_os = "linux")]
const SIGXFSZ_EXIT_STATUS: i32 = 128 + 25;

/// `/proc/self/limits` 的无限限制字面值。
#[cfg(target_os = "linux")]
const PROC_LIMIT_UNLIMITED: &str = "unlimited";

/// 极小文件大小上限：足以让自检命令写出一个块后被信号终止。
#[cfg(target_os = "linux")]
const TEST_SMALL_FILE_BYTES: u64 = 4_096;

/// 自检命令尝试写入的字节数；必须远大于 [`TEST_SMALL_FILE_BYTES`]。
#[cfg(target_os = "linux")]
const TEST_OVERSIZED_WRITE_BYTES: u64 = 256 * 1_024;

/// 句柄上限：低到必然触发 EMFILE，又高到足以让 fork/exec 与管道正常工作。
#[cfg(target_os = "linux")]
const TEST_SMALL_OPEN_FILES: u64 = 300;

/// 自检命令在句柄上限内尝试打开的描述符数量。
#[cfg(target_os = "linux")]
const TEST_OPEN_ATTEMPTS: u64 = 2_048;

/// 地址空间上限：低到必然拒绝大块分配，又高到足以加载 shell。
#[cfg(target_os = "linux")]
const TEST_SMALL_ADDRESS_SPACE_BYTES: u64 = 256 * 1024 * 1024;

/// 进程数上限探针使用的取值。
#[cfg(target_os = "linux")]
const TEST_SMALL_PROCESSES: u64 = 64;

/// 进程数探针请求的上限：高于 [`TEST_SMALL_PROCESSES`]，但仍远低于宿主可能给的上限。
///
/// `RLIMIT_NPROC` 对 uid 0 不生效，因此该用例必须由降权子进程运行。
#[cfg(target_os = "linux")]
const TEST_FORK_LIMIT: u64 = 4_096;

/// 进程数探针最多尝试派生的子进程数量；必须显著超过 [`TEST_FORK_LIMIT`]。
#[cfg(target_os = "linux")]
const TEST_FORK_ATTEMPTS: u64 = 8_192;

/// 进程数探针每个子进程的存活时长（秒）；必须长到足以让子进程并发堆积。
#[cfg(target_os = "linux")]
const FORK_PROBE_HOLD_SECONDS: u32 = 5;

/// 内核拒绝 fork 时 shell 打印的错误片段。
#[cfg(target_os = "linux")]
const FORK_DENIED_MESSAGE: &str = "Resource temporarily unavailable";

/// 用于降权到非 root 身份的工具；`RLIMIT_NPROC` 对 root 不生效。
#[cfg(target_os = "linux")]
const SETPRIV_PROGRAM: &str = "/usr/bin/setpriv";

/// 降权后的 uid/gid：nobody / nogroup。
#[cfg(target_os = "linux")]
const UNPRIVILEGED_ID: &str = "65534";

/// 活跃进程限额命中时，测试脚本 `... && exit 0 || exit 9` 的失败分支退出码。
#[cfg(windows)]
const WINDOWS_JOB_LIMIT_DENIED_EXIT_CODE: i32 = 9;

/// 正常命令所需的活跃进程上限；远高于这两个 Windows 测试的实际进程用量。
#[cfg(windows)]
const TEST_PROCESS_LIMIT_FOR_NORMAL_COMMAND: u64 = 64;

#[test]
#[cfg(windows)]
fn windows_job_object_enforces_the_active_process_limit() {
    let limits = ProcessResourceLimits {
        max_processes: Some(1),
        ..ProcessResourceLimits::off()
    };
    // 必须使用**外部可执行文件**：`ver` 是 cmd 内建命令，不会创建新进程，
    // 因此观察不到活跃进程限额。同时必须用 `&&` 串联：`&` 会无条件执行 `exit 0`，
    // 把派生失败掩盖成成功。
    let mut command = Command::new("cmd.exe");
    command
        .args([
            "/d",
            "/s",
            "/c",
            "whoami.exe >nul 2>nul && exit 0 || exit 9",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    configure_process_resource_limits(&mut command, limits);
    let mut child = command.spawn().expect("cmd.exe 应可启动");
    let handle = fence_spawned_child(&child, limits)
        .expect("绑定 Job Object 不应报错")
        .expect("Windows 必须返回 Job Object 句柄，否则围栏从未生效");
    let status = child.wait().expect("子进程应可退出");

    assert_eq!(
        status.code(),
        Some(WINDOWS_JOB_LIMIT_DENIED_EXIT_CODE),
        "ACTIVE_PROCESS=1 时派生外部进程必须被内核拒绝，实际：{status:?}"
    );
    drop(handle);
}
#[test]
#[cfg(windows)]
fn windows_job_object_without_limits_lets_a_normal_command_finish() {
    let limits = ProcessResourceLimits {
        max_processes: Some(TEST_PROCESS_LIMIT_FOR_NORMAL_COMMAND),
        ..ProcessResourceLimits::off()
    };
    let mut command = Command::new("cmd.exe");
    command
        .args(["/d", "/s", "/c", "ver >nul & exit 0"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    configure_process_resource_limits(&mut command, limits);
    let mut child = command.spawn().expect("cmd.exe 应可启动");
    let handle = fence_spawned_child(&child, limits)
        .expect("绑定 Job Object 不应报错")
        .expect("Windows 必须返回 Job Object 句柄");
    let status = child.wait().expect("子进程应可退出");

    assert_eq!(
        status.code(),
        Some(0),
        "限额足够宽时正常命令必须照常完成，实际：{status:?}"
    );
    drop(handle);
}

#[test]
#[cfg(target_os = "linux")]
fn probe_binary_applies_the_production_path_and_reports_limits() {
    let limits = ProcessResourceLimits {
        max_file_bytes: Some(TEST_SMALL_FILE_BYTES),
        max_processes: Some(TEST_SMALL_PROCESSES),
        max_open_files: Some(TEST_SMALL_OPEN_FILES),
        max_address_space_bytes: Some(TEST_SMALL_ADDRESS_SPACE_BYTES),
    };
    let output = probe(limits, Some("true")).expect("探针应可启动");
    let stdout = output_text(&output);

    assert_reported_limit(&stdout, "fsize", TEST_SMALL_FILE_BYTES);
    assert_reported_limit(&stdout, "nofile", TEST_SMALL_OPEN_FILES);
    assert_reported_limit(&stdout, "as", TEST_SMALL_ADDRESS_SPACE_BYTES);
    assert_eq!(
        reported_action_exit(&stdout),
        Some(0),
        "正常命令在围栏下必须照常执行完成：\n{stdout}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn fence_kills_writes_beyond_the_file_size_limit() {
    let limits = ProcessResourceLimits {
        max_file_bytes: Some(TEST_SMALL_FILE_BYTES),
        ..ProcessResourceLimits::off()
    };
    // 必须写**常规文件**：`RLIMIT_FSIZE` 对管道/FIFO 不生效，写到 stdout 观察不到。
    // 不能用 `cmd; echo $?` 之外的写法判断子进程结局：bash 自己总是正常退出，
    // 只有显式回显 `$?` 才能区分「被信号终止」与「正常退出」。
    let action = format!(
        "rm -f {path}; dd if=/dev/zero of={path} bs=1024 count={kib} >/dev/null 2>&1; \
         echo \"dd-status=$?\"",
        path = OVERSIZED_WRITE_PATH,
        kib = TEST_OVERSIZED_WRITE_BYTES / 1_024,
    );
    let output = probe(limits, Some(&action)).expect("探针应可启动");
    let stdout = output_text(&output);

    assert_reported_limit(&stdout, "fsize", TEST_SMALL_FILE_BYTES);
    assert_eq!(
        reported_action_exit(&stdout),
        Some(0),
        "自检命令本身应正常结束，信号只落在被限制的写入进程上：\n{stdout}"
    );
    assert_eq!(
        reported_child_status(&stdout, OVERSIZED_WRITE_STATUS_FIELD),
        Some(SIGXFSZ_EXIT_STATUS),
        "写入超过 RLIMIT_FSIZE 的进程必须被 SIGXFSZ 终止（shell 记为 128+25）：\n{stdout}"
    );
    let written = std::fs::metadata(OVERSIZED_WRITE_PATH)
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    assert_eq!(
        written, TEST_SMALL_FILE_BYTES,
        "文件必须恰好停在 RLIMIT_FSIZE 上，实际写了 {written} 字节"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn fence_rejects_descriptors_beyond_the_open_file_limit() {
    let limits = ProcessResourceLimits {
        max_open_files: Some(TEST_SMALL_OPEN_FILES),
        ..ProcessResourceLimits::off()
    };
    let action = format!(
        "i=0; while [ $i -lt {attempts} ]; do exec {{fd}}>/dev/null 2>/dev/null || exit 0; \
         i=$((i+1)); done; exit 1",
        attempts = TEST_OPEN_ATTEMPTS,
    );
    let output = probe(limits, Some(&action)).expect("探针应可启动");
    let stdout = output_text(&output);

    assert_reported_limit(&stdout, "nofile", TEST_SMALL_OPEN_FILES);
    assert_eq!(
        reported_action_exit(&stdout),
        Some(0),
        "超过 RLIMIT_NOFILE 的 open 必须被内核拒绝：\n{stdout}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn address_space_limit_still_allows_a_normal_command() {
    let limits = ProcessResourceLimits {
        max_address_space_bytes: Some(TEST_SMALL_ADDRESS_SPACE_BYTES),
        ..ProcessResourceLimits::off()
    };
    let output = probe(limits, Some("true")).expect("探针应可启动");
    let stdout = output_text(&output);

    assert_reported_limit(&stdout, "as", TEST_SMALL_ADDRESS_SPACE_BYTES);
    assert_eq!(
        reported_action_exit(&stdout),
        Some(0),
        "地址空间限额不得阻止正常命令执行：\n{stdout}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn off_leaves_every_dimension_at_the_inherited_value() {
    let output = probe(ProcessResourceLimits::off(), Some("true")).expect("探针应可启动");
    let stdout = output_text(&output);

    // 关闭围栏的含义是「不做任何改动」，因此逐维度比对**继承值**，
    // 而不是假定 unlimited：宿主本身可能已给了有限限制，围栏必须原样保留它。
    for (label, field) in [
        ("Max file size", "fsize"),
        ("Max processes", "nproc"),
        ("Max open files", "nofile"),
        ("Max address space", "as"),
    ] {
        assert_eq!(
            reported_limit(&stdout, field).as_deref(),
            inherited_limit(label).as_deref(),
            "关闭围栏后 {field} 必须等于继承值，实际输出：\n{stdout}"
        );
    }
    assert_eq!(reported_action_exit(&stdout), Some(0));
}

#[test]
#[cfg(target_os = "linux")]
fn soft_limits_never_exceed_the_available_hard_limit() {
    let limits = ProcessResourceLimits {
        max_file_bytes: Some(TEST_SMALL_FILE_BYTES),
        max_processes: Some(TEST_SMALL_PROCESSES),
        max_open_files: Some(TEST_SMALL_OPEN_FILES),
        max_address_space_bytes: Some(TEST_SMALL_ADDRESS_SPACE_BYTES),
    };
    let output = probe(limits, Some("true")).expect("探针应可启动");
    assert!(
        output.status.success(),
        "限额不得超过可用硬限制，否则 setrlimit 会让 spawn 失败：{:?}",
        output.status
    );

    // 把期望值顶到父进程的硬限制：内核只可能接受「等于」它，不可能更高。
    let hard = hard_limit_from_proc_limits("Max open files").expect("必须能读到句柄硬限制");
    let limits = ProcessResourceLimits {
        max_open_files: Some(hard),
        ..ProcessResourceLimits::off()
    };
    let output = probe(limits, Some("true")).expect("探针应可启动");
    assert_reported_limit(&output_text(&output), "nofile", hard);
}

#[test]
#[cfg(target_os = "linux")]
fn pty_wrapper_uses_the_prlimit_program_and_keeps_the_command() {
    let mut command = vec![
        "/bin/bash".to_owned(),
        "-lc".to_owned(),
        "echo hi".to_owned(),
    ];
    let wrapper = fence_pty_command(&mut command, ProcessResourceLimits::interactive_default());
    let Some(wrapper) = wrapper else {
        return;
    };

    assert!(
        std::path::Path::new(&wrapper).is_file(),
        "选中的 PTY 包装程序必须真实存在：{wrapper}"
    );
    assert_eq!(
        std::path::Path::new(&wrapper).file_name(),
        Some(std::ffi::OsStr::new("prlimit")),
        "PTY 包装程序必须是 prlimit：{wrapper}"
    );
    assert_eq!(
        &command[command.len() - 3..],
        ["/bin/bash", "-lc", "echo hi"],
        "原始命令必须原样保留在包装参数之后"
    );
    assert!(
        command.iter().any(|argument| argument == "--"),
        "限额参数之后必须有 -- 终止选项"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn spawn_applies_the_fence_to_the_child_through_pre_exec() {
    let limits = ProcessResourceLimits {
        max_file_bytes: Some(TEST_SMALL_FILE_BYTES),
        ..ProcessResourceLimits::off()
    };
    let mut command = Command::new("/bin/true");
    configure_process_resource_limits(&mut command, limits);
    let mut child = command.spawn().expect("/bin/true 应可启动");

    // 直接读子进程的限额：这是 pre_exec 路径本身的证据，不依赖探针。
    assert_eq!(
        soft_limit_of_process(child.id(), "Max file size").as_deref(),
        Some(TEST_SMALL_FILE_BYTES.to_string().as_str()),
        "spawn 出来的子进程必须已带上 pre_exec 设置的限额"
    );
    let fence = fence_spawned_child(&child, limits).expect("围栏绑定不应报错");
    if fence.is_some() {
        assert!(
            std::path::Path::new("/usr/bin/prlimit").is_file()
                || std::path::Path::new("/bin/prlimit").is_file(),
            "返回句柄意味着 PTY 包装可用，此时 prlimit 必须存在"
        );
    }
    assert!(
        child.wait().expect("/bin/true 应可退出").success(),
        "RLIMIT_FSIZE 不得阻断一个正常命令"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn fence_rejects_fork_bursts_beyond_the_process_limit() {
    if !std::path::Path::new(SETPRIV_PROGRAM).is_file() {
        return;
    }
    let limits = ProcessResourceLimits {
        max_processes: Some(TEST_FORK_LIMIT),
        ..ProcessResourceLimits::off()
    };
    // RLIMIT_NPROC 只对非 root 生效，因此必须降权到 nobody 再派生进程。
    // 自检命令必须**并发**派生：串行 `sleep; wait` 只会有 1 个存活子进程，
    // 永远碰不到限额。批量 `kill $(jobs -p)` 会在数千个作业时因命令行过长失败，
    // 因此这里只等它们自然结束（每个只活 `FORK_PROBE_HOLD_SECONDS` 秒）。
    let action = format!(
        "for i in $(seq 1 {attempts}); do sleep {hold} & done; exit 0",
        attempts = TEST_FORK_ATTEMPTS,
        hold = FORK_PROBE_HOLD_SECONDS,
    );
    let output = probe_as_unprivileged_user(limits, Some(&action)).expect("探针应可启动");
    let stdout = output_text(&output);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // 容器里的硬限制可能低于请求值，此时实现会静默取 min，期望值必须跟着折算。
    assert_reported_limit(&stdout, "nproc", expected_process_limit(TEST_FORK_LIMIT));
    let denials = stderr.matches(FORK_DENIED_MESSAGE).count();
    assert!(
        denials > 0,
        "达到 RLIMIT_NPROC 后的 fork 必须被内核拒绝（stderr 应出现 `{FORK_DENIED_MESSAGE}`）：\n{stderr}"
    );
    assert!(
        reported_action_exit(&stdout).is_some(),
        "自检命令必须报告退出码，否则无法判断它是否真的跑到了派生循环：\n{stdout}"
    );
}

/// 按实现规则折算进程数上限：先取下限，再对可用硬限制取 min。
#[cfg(target_os = "linux")]
fn expected_process_limit(desired: u64) -> u64 {
    hard_limit_from_proc_limits("Max processes").map_or(desired, |hard| desired.min(hard))
}

/// 用生产函数启动探针，并把限额经环境变量交给探针自己施加。
#[cfg(target_os = "linux")]
fn probe(limits: ProcessResourceLimits, action: Option<&str>) -> io::Result<std::process::Output> {
    configure_probe(Command::new(PROBE_BINARY), limits, action).output()
}

/// 以非 root 身份启动探针：`RLIMIT_NPROC` 对 uid 0 不生效，必须降权后才可观测。
#[cfg(target_os = "linux")]
fn probe_as_unprivileged_user(
    limits: ProcessResourceLimits,
    action: Option<&str>,
) -> io::Result<std::process::Output> {
    let mut command = Command::new(SETPRIV_PROGRAM);
    command.args([
        &format!("--reuid={UNPRIVILEGED_ID}"),
        &format!("--regid={UNPRIVILEGED_ID}"),
        "--clear-groups",
        PROBE_BINARY,
    ]);
    configure_probe(command, limits, action).output()
}

/// 给探针命令装配生产路径的围栏与限额环境变量。
#[cfg(target_os = "linux")]
fn configure_probe(
    mut command: Command,
    limits: ProcessResourceLimits,
    action: Option<&str>,
) -> Command {
    configure_process_resource_limits(&mut command, limits);
    // 未设限的维度必须**不写入**环境变量：探针把「未设置」解释为不限制。
    for (name, value) in [
        (PROCESS_RLIMIT_FSIZE_ENV, limits.max_file_bytes),
        (PROCESS_RLIMIT_NPROC_ENV, limits.max_processes),
        (PROCESS_RLIMIT_NOFILE_ENV, limits.max_open_files),
        (PROCESS_RLIMIT_AS_ENV, limits.max_address_space_bytes),
    ] {
        if let Some(value) = value {
            command.env(name, value.to_string());
        }
    }
    if let Some(action) = action {
        command.arg(action);
    }
    command
}

/// 从任意进程的 `/proc/<pid>/limits` 读取某一维度的软限制。
#[cfg(target_os = "linux")]
fn soft_limit_of_process(pid: u32, label: &str) -> Option<String> {
    let contents = std::fs::read_to_string(format!("/proc/{pid}/limits")).ok()?;
    let line = contents.lines().find(|line| line.starts_with(label))?;
    let fields = line.split_whitespace().collect::<Vec<_>>();
    fields
        .len()
        .checked_sub(3)
        .and_then(|index| fields.get(index))
        .map(|value| (*value).to_owned())
}

/// 解析探针输出的 `<字段> <取值>` 行。
#[cfg(target_os = "linux")]
fn reported_limit(stdout: &str, field: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{field} ")))
        .map(str::to_owned)
}

/// 解析探针自检命令的退出码。
#[cfg(target_os = "linux")]
fn reported_action_exit(stdout: &str) -> Option<i32> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(ACTION_EXIT_PREFIX))
        .and_then(|value| value.parse().ok())
}

/// 解析自检命令自行回显的某个子进程状态字段。
///
/// 只有显式回显 `$?` 才能区分「被信号终止」与「正常退出」：bash 自身总是正常退出。
#[cfg(target_os = "linux")]
fn reported_child_status(stdout: &str, field: &str) -> Option<i32> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|value| value.trim().parse().ok())
}

/// 断言探针报告的某一维度等于期望值。
#[cfg(target_os = "linux")]
fn assert_reported_limit(stdout: &str, field: &str, expected: u64) {
    assert_eq!(
        reported_limit(stdout, field).as_deref(),
        Some(expected.to_string().as_str()),
        "探针应报告内核接受 {field}={expected}，实际输出：\n{stdout}"
    );
}

/// 探针的标准输出；限额报告全部走 stdout。
#[cfg(target_os = "linux")]
fn output_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// 从本进程的 `/proc/self/limits` 读取某一维度的软限制原文（含 `unlimited`）。
#[cfg(target_os = "linux")]
fn inherited_limit(label: &str) -> Option<String> {
    let contents = std::fs::read_to_string(PROC_LIMITS_PATH).ok()?;
    let line = contents.lines().find(|line| line.starts_with(label))?;
    let fields = line.split_whitespace().collect::<Vec<_>>();
    fields
        .len()
        .checked_sub(3)
        .and_then(|index| fields.get(index))
        .map(|value| (*value).to_owned())
}

/// 从本进程的 `/proc/self/limits` 读取某一维度的硬限制。
#[cfg(target_os = "linux")]
fn hard_limit_from_proc_limits(label: &str) -> Option<u64> {
    let contents = std::fs::read_to_string(PROC_LIMITS_PATH).ok()?;
    let line = contents.lines().find(|line| line.starts_with(label))?;
    let fields = line.split_whitespace().collect::<Vec<_>>();
    let hard = fields.get(fields.len().checked_sub(2)?)?;
    if *hard == PROC_LIMIT_UNLIMITED {
        return None;
    }
    hard.parse().ok()
}

/// 探针派生孙进程后立即退出的模式。
#[cfg(target_os = "linux")]
const SPAWN_CHILD_MODE: &str = "--spawn-fenced-child-and-exit";

/// 孙进程 PID 的输出前缀。
#[cfg(target_os = "linux")]
const CHILD_PID_PREFIX: &str = "child-pid=";

/// 等待内核回收孙进程的窗口。
#[cfg(target_os = "linux")]
const RECLAIM_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// 存活检查的轮询间隔。
#[cfg(target_os = "linux")]
const RECLAIM_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// 宿主死亡必须回收受围栏的子进程（Linux 侧由 `PR_SET_PDEATHSIG` 保证）。
///
/// 断的是**行为**而不是「有没有调用 `prctl`」：探针派生一个 300 秒的 `sleep` 后立即
/// 退出（模拟宿主被强杀：宿主没有机会运行任何清理代码），若子进程仍然存活，就说明
/// 它变成了孤儿。缺了这一层，`terminate_managed_process_tree` 与定时清理都要求宿主
/// 仍然活着，宿主崩溃后的子进程将无人可管。
///
/// Windows 侧的对称保证（Job Object kill-on-close）由 `resource_fence_tests.rs` 的
/// `closing_the_fence_handle_terminates_the_fenced_child` 覆盖。
#[cfg(target_os = "linux")]
#[test]
fn host_exit_reclaims_fenced_children() {
    let output = Command::new(PROBE_BINARY)
        .arg(SPAWN_CHILD_MODE)
        .output()
        .expect("探针应可运行");
    assert!(
        output.status.success(),
        "探针应正常退出，实际：{:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix(CHILD_PID_PREFIX))
        .and_then(|value| value.parse::<i32>().ok())
        .expect("探针必须报告孙进程 PID");

    // 探针（宿主）此刻已退出：孙进程若仍在运行，就是无人可管的孤儿。
    let deadline = std::time::Instant::now() + RECLAIM_WAIT;
    while std::time::Instant::now() < deadline {
        if !process_still_running(pid) {
            return;
        }
        std::thread::sleep(RECLAIM_POLL);
    }
    // 失败时清理现场，避免测试自己留下一个 300 秒的 sleep。
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    panic!("宿主退出后受围栏子进程必须被回收；pid={pid} 仍在运行");
}

/// 进程是否仍在运行；已退出或被回收都算「不在了」。
///
/// `/proc/<pid>/stat` 形如 `1234 (comm) S ...`：状态是**最后一个**右括号之后的第一个
/// 字段（`comm` 本身可能含空格与括号，故不能按固定下标切分）。`Z` 表示僵尸：
/// 进程已结束、只等回收，不再算「仍在运行」。
#[cfg(target_os = "linux")]
fn process_still_running(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, after_comm)) = stat.rsplit_once(')') else {
        return false;
    };
    !matches!(after_comm.split_whitespace().next(), Some("Z") | None)
}
