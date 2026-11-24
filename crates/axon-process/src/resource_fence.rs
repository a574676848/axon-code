//! 子进程资源围栏：把「进程能消耗多少磁盘、进程数、句柄、地址空间」变成内核硬边界。
//!
//! 静态命令黑名单（`shell_command_policy_evaluator`）是软门禁，一旦审批策略放行即失效；
//! 本模块不依赖任何判定逻辑，失控进程只能被内核拒绝或终止。
//!
//! # 平台差异
//!
//! - Unix：`setrlimit` 设置 `RLIMIT_FSIZE` / `RLIMIT_NPROC` / `RLIMIT_NOFILE` / `RLIMIT_AS`。
//! - Windows：Job Object 设置 `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`（对应进程数）与
//!   `JOB_OBJECT_LIMIT_PROCESS_MEMORY`（对应地址空间）。Windows 没有文件大小限额的
//!   等价物，`max_file_bytes` 与 `max_open_files` 在该平台不生效。
//!
//! # 失败方向是有意选择的（两平台并不一致）
//!
//! - **Unix 硬失败**：`pre_exec` 内 `setrlimit` 出错会让 `spawn` 直接失败。这是安全的，
//!   因为本模块只**降低**软限制、永不抬高 `rlim_max`，因此不需要 `CAP_SYS_RESOURCE`，
//!   唯一可能的失败来源是「请求了不可能的值」：那种情况下宁可让调用方看到错误。
//! - **Windows 尽力而为**：`AssignProcessToJobObject` 失败时子进程照常启动，只是少一层
//!   边界（见 [`fence_spawned_child`]）。代价不对称：少一层边界只是风险敞口，
//!   而拒绝启动会直接破坏正常流程：本 WP 的最大风险正是误伤正常流程。
//! - **Linux parent-death 尽力而为**：`PR_SET_PDEATHSIG` 被沙箱过滤时不阻断启动，
//!   理由同上（见 `apply_parent_death_signal`）。
//!
//! # 唯一的 unsafe 边界
//!
//! 本模块是全仓唯一允许 `unsafe` 的模块（见 `Cargo.toml` 的 `[lints.rust]`）：
//! Unix 侧的 `setrlimit` 只能经 `pre_exec`（`unsafe fn`）在 fork 后、exec 前设置，
//! Windows 侧的 Job Object 也没有安全封装。每个 `unsafe` 块都带 `// SAFETY:` 说明，
//! 且 `crates/axon-process/src/lib.rs` 的测试会断言 `unsafe` 只出现在本文件。

use std::{io, process::Command};

/// 子进程文件大小上限（字节）；超出时内核发送 SIGXFSZ 终止进程。
///
/// 取值需兼顾：既要能容纳正常的构建产物与日志，又要能阻断 `yes > file` 类写满磁盘。
/// 4 GiB 远高于单次构建产物量级（本仓 `target/` 中最大单文件约 166 MiB）。
pub const DEFAULT_RLIMIT_FSIZE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 子进程数上限（含线程）；防止 fork 炸弹。
///
/// 必须显著高于正常负载：`cargo test` 的并行测试可能启动数百进程。
pub const DEFAULT_RLIMIT_NPROC: u64 = 4_096;

/// 打开文件描述符上限。
pub const DEFAULT_RLIMIT_NOFILE: u64 = 4_096;

/// 地址空间上限；`None` 表示不限制。
///
/// 默认不限制以避让 rustc/LLVM：现代编译器在个别 crate 上会用到数 GiB 地址空间，
/// 设限会误伤正常构建。
pub const DEFAULT_RLIMIT_AS_BYTES: Option<u64> = None;

/// PTY 交互式会话的文件大小上限（字节）。
///
/// 比一次性命令更宽：交互式会话里跑构建、下载、解包都属正常用法，
/// 而它的失控风险低于模型拼接的一次性命令。
pub const PTY_RLIMIT_FSIZE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// PTY 交互式会话的进程数上限。
///
/// 交互式会话会自建作业编排（`make -j`、并行测试、包管理器），进程数天然高于一次性命令。
pub const PTY_RLIMIT_NPROC: u64 = 16_384;

/// PTY 交互式会话的打开文件描述符上限。
///
/// 比默认更宽：终端多路复用、watch 模式与语言服务器会同时持有很多句柄。
pub const PTY_RLIMIT_NOFILE: u64 = 8_192;

/// 一次性命令的进程数下限。
///
/// fork 之后子进程只持有自身一个进程，64 足以让 `exec` 成功，
/// 同时远低于任何正常负载，不会把配置值顶高。
#[cfg(unix)]
const MIN_RLIMIT_NPROC: u64 = 64;

/// 一次性命令的文件描述符下限。
///
/// fork 之后子进程只持有少数几个描述符（标准流与 exec 自身所需），256 足以让 `exec`
/// 成功，同时远低于任何正常负载，不会把配置值顶高。
#[cfg(unix)]
const MIN_RLIMIT_NOFILE: u64 = 256;

/// 资源围栏的环境变量覆盖项：文件大小上限（字节），`off` 或 `0` 表示不限制。
pub const PROCESS_RLIMIT_FSIZE_ENV: &str = "AXON_PROCESS_RLIMIT_FSIZE_BYTES";

/// 资源围栏的环境变量覆盖项：进程数上限，`off` 或 `0` 表示不限制。
pub const PROCESS_RLIMIT_NPROC_ENV: &str = "AXON_PROCESS_RLIMIT_NPROC";

/// 资源围栏的环境变量覆盖项：打开文件描述符上限，`off` 或 `0` 表示不限制。
pub const PROCESS_RLIMIT_NOFILE_ENV: &str = "AXON_PROCESS_RLIMIT_NOFILE";

/// 资源围栏的环境变量覆盖项：地址空间上限（字节），`off` 或 `0` 表示不限制。
pub const PROCESS_RLIMIT_AS_ENV: &str = "AXON_PROCESS_RLIMIT_AS_BYTES";

/// 环境变量中表示「该维度不限制」的取值。
const RESOURCE_LIMIT_DISABLED_VALUE: &str = "off";

/// 终止 `prlimit` 选项解析、分隔包装参数与目标命令的标记。
const PTY_ARGUMENT_TERMINATOR: &str = "--";

#[path = "resource_fence_limits.rs"]
mod resource_fence_limits;

/// `getrlimit` / `setrlimit` 接受的具体资源类型。
///
/// glibc 与 uClibc 把它声明为无符号数，而 musl、BSD 与 macOS 声明为 `c_int`；
/// 因此这里必须按 libc 自身的 `__rlimit_resource_t` 定义跟随，不能统一成一种。
#[cfg(all(
    unix,
    any(
        all(target_os = "linux", target_env = "gnu"),
        all(target_os = "linux", target_env = "uclibc"),
    )
))]
type RlimitResource = libc::__rlimit_resource_t;

/// 见 [`RlimitResource`] 的说明；其余平台使用 `c_int`。
#[cfg(all(
    unix,
    not(any(
        all(target_os = "linux", target_env = "gnu"),
        all(target_os = "linux", target_env = "uclibc"),
    ))
))]
type RlimitResource = libc::c_int;

#[path = "pty_wrapper.rs"]
mod pty_wrapper;

pub use resource_fence_limits::ProcessResourceLimits;
// PTY 包装解析已移入 `pty_wrapper`；私有引入保持父模块内调用点写法不变。
use pty_wrapper::{pty_limit_arguments, resolved_pty_wrapper};

/// 为子进程配置资源围栏；须在 `spawn` 之前调用。
///
/// Unix 在 `pre_exec` 中设置 rlimit（fork 后、exec 前），失败即 `spawn` 失败。
/// Windows 的 Job Object 必须在进程启动之后绑定，因此这里不做任何事，
/// 由 [`fence_spawned_child`] 在 `spawn` 之后完成。
pub fn configure_process_resource_limits(command: &mut Command, limits: ProcessResourceLimits) {
    #[cfg(unix)]
    configure_unix_limits(command, limits);
    #[cfg(not(unix))]
    let _ = (command, limits);
}

/// 就地为当前进程设置资源围栏。
///
/// 供明确要接纳围栏的调用方使用（例如测试探针与未来自行派生进程的宿主）。
/// 与 [`configure_process_resource_limits`] 使用**完全相同**的折算逻辑，
/// 因此二者观察到的限额必然一致。
///
/// # Errors
///
/// 任一 `setrlimit` 失败时返回该 `errno` 对应的错误；此时已生效的维度不会回滚。
pub fn apply_process_resource_limits(limits: ProcessResourceLimits) -> io::Result<()> {
    #[cfg(unix)]
    {
        EffectiveUnixLimits::resolve(limits).apply()
    }
    #[cfg(not(unix))]
    {
        // 非 Unix 平台没有进程级限额原语可施加；Job Object 必须绑定到子进程。
        let _ = limits;
        Ok(())
    }
}

/// 进程启动后完成平台侧绑定，并返回需与子进程同生命周期的围栏句柄。
///
/// Unix 的限额已在 `pre_exec` 中生效，此处只报告 PTY 包装程序路径（若有）。
/// Windows 在此把子进程绑定到 Job Object；绑定失败返回 `Ok(None)` 且**不阻断**子进程：
/// 围栏缺失只是少一层硬边界，而拒绝启动会直接破坏正常流程，两者代价不对等。
///
/// # 平台差异（伙伴机制，据实记录）
///
/// Windows：Job Object 置了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，宿主进程退出（正常或
/// 崩溃）时句柄关闭，作业内子进程由内核回收：**不会留下孤儿**。
///
/// Linux：`pre_exec` 中置 `PR_SET_PDEATHSIG(SIGKILL)`（见 `apply_parent_death_signal`），
/// 宿主死亡时由内核回收子进程，与 Windows 对称：后台作业在 Daemon 存活期间照常跨
/// turn 读取（句柄与线程都随宿主存活），只有宿主真正消失才会被回收。
///
/// 残留缺口（两处，均为已知）：
/// - **PTY 会话**：`portable_pty` 不暴露 `pre_exec`，其子进程只能靠 Unix 的 `prlimit`
///   包装施加限额，因此没有 parent-death 保护（Windows 侧 PTY 同样无 Job Object）。
/// - **macOS / BSD**：无 `PR_SET_PDEATHSIG` 等价接口（`procctl` 仅 FreeBSD），子进程仍只
///   被放入独立进程组（`process_group(0)`）；宿主被强杀时须由仍然活着的进程调用
///   `terminate_managed_process_tree` 回收。
pub fn fence_spawned_child(
    child: &std::process::Child,
    limits: ProcessResourceLimits,
) -> io::Result<Option<ProcessFenceHandle>> {
    #[cfg(windows)]
    {
        Ok(create_job_object_and_assign(child, limits)
            .ok()
            .map(|job| ProcessFenceHandle { job: Some(job) }))
    }
    #[cfg(unix)]
    {
        let _ = (child, limits);
        Ok(resolved_pty_wrapper().map(|wrapper| ProcessFenceHandle {
            pty_wrapper: Some(wrapper),
        }))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (child, limits);
        Ok(None)
    }
}

/// 把 PTY 会话命令包装为受限执行；返回实际应启动的程序路径。
///
/// `portable_pty` 不暴露 `pre_exec` 钩子，无法在 PTY 子进程上直接 `setrlimit`，
/// 因此改用 `prlimit` 包装：它先设置 rlimit 再 `exec` 目标程序，进程号保持不变，
/// 既有的进程树回收与 PTY 会话语义不受影响。当限额全关或系统无 `prlimit` 时返回 `None`，
/// 调用方保持原命令、不做包装。
pub fn fence_pty_command(
    command: &mut Vec<String>,
    limits: ProcessResourceLimits,
) -> Option<String> {
    if limits.is_off() {
        return None;
    }
    let wrapper = resolved_pty_wrapper()?;
    let mut wrapped = vec![wrapper.clone()];
    wrapped.extend(pty_limit_arguments(limits));
    wrapped.push(PTY_ARGUMENT_TERMINATOR.to_owned());
    wrapped.append(command);
    *command = wrapped;
    Some(wrapper)
}

/// 与子进程同生命周期的围栏句柄；`Drop` 时关闭平台句柄。
#[derive(Debug)]
pub struct ProcessFenceHandle {
    /// Unix PTY 包装程序路径；仅作为「本会话命令已被包装」的事实记录。
    ///
    /// 字段对调用方公开：Unix 侧没有需要释放的句柄，该值本身就是句柄唯一可观测的
    /// 内容，用于诊断「围栏是否真的包装上了」。
    #[cfg(unix)]
    pub pty_wrapper: Option<String>,
    /// Windows Job Object 句柄；随本结构释放，子进程退出后 Job 自动销毁。
    #[cfg(windows)]
    job: Option<windows_sys::Win32::Foundation::HANDLE>,
}

impl Drop for ProcessFenceHandle {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            // SAFETY: `job` 来自本结构内 `CreateJobObjectW` 的成功返回值，未被复制到
            // 其他所有者；`Drop` 只执行一次，因此这里不会重复关闭同一个句柄。
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
        }
    }
}

// SAFETY: Windows 内核句柄是进程级引用，本身不绑定线程，可在任意线程使用与关闭；
// windows-sys 把 `HANDLE` 声明为裸指针，因此需要手动声明所有权语义。
// `ProcessFenceHandle` 独占该句柄（仅 `Drop` 中消费一次），跨线程转移所有权是安全的。
#[cfg(windows)]
unsafe impl Send for ProcessFenceHandle {}

// SAFETY: 共享引用不提供任何访问句柄的方法，也不存在内部可变性。
#[cfg(windows)]
unsafe impl Sync for ProcessFenceHandle {}

#[cfg(unix)]
fn configure_unix_limits(command: &mut Command, limits: ProcessResourceLimits) {
    use std::os::unix::process::CommandExt as _;

    // 全部 syscall 与分配都在 fork 之前完成：闭包内只允许 async-signal-safe 的
    // `prctl` / `getppid` / `_exit` / `setrlimit`，不得分配内存、加锁、格式化字符串
    // 或构造 `io::Error` 文本。
    let effective = EffectiveUnixLimits::resolve(limits);
    // fork 之前记录宿主 PID，用于闭合「父线程在 fork 与 prctl 之间已退出」的竞态窗口。
    // Linux 的 pid 上限（默认 2^22）远小于 i32::MAX，转换不会失败。
    let host_pid = libc::pid_t::try_from(std::process::id()).unwrap_or(0);

    // SAFETY: `pre_exec` 的契约是「闭包内只做 async-signal-safe 的事」。
    // 本闭包只读取两个已算好的 `Copy` 值（四个 `Option<u64>` 与宿主 pid，均按值 move
    // 进闭包，无共享可变状态），并调用 async-signal-safe 的 `prctl` / `getppid` /
    // `_exit` / `setrlimit`；不分配内存、不加锁、不做 I/O、不格式化字符串。
    // 错误一律经 `io::Error::last_os_error` 以裸 errno 传递。
    unsafe {
        command.pre_exec(move || {
            apply_parent_death_signal(host_pid);
            effective.apply()
        });
    }
}

/// 宿主进程消失时子进程被终止的退出码；仅在「父线程已先于 `prctl` 退出」的竞态里使用。
#[cfg(target_os = "linux")]
const HOST_GONE_EXIT_CODE: i32 = 1;

/// Linux：让内核在本进程的父线程退出时向本进程发送 `SIGKILL`。
///
/// 与 Windows 的 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 对称：宿主被 `SIGKILL` 强杀时
/// 没有机会执行任何清理代码，只有内核级机制能避免子进程变成无人可管的孤儿。
///
/// # 约束：必须由**进程级**线程派生命令
///
/// `PR_SET_PDEATHSIG` 的触发条件是**父线程**退出（不是父进程），且该设置会在 `fork`
/// 后自动清除。当前两条派生路径都满足「线程与进程同生命周期」：
/// Daemon 的 Primary Turn 跑在常驻 worker 池线程上（`primary_turn_worker_pool`），
/// CLI 走主线程；临时线程（如 `spawn_blocking` 的空闲即回收线程）**不满足**。
/// 若将来改为在临时线程中派生，本机制会误杀子进程：届时须改为「由常驻 spawner
/// 线程统一派生」或移除本调用。
#[cfg(target_os = "linux")]
fn apply_parent_death_signal(host_pid: libc::pid_t) {
    // SAFETY: `prctl` / `getppid` / `_exit` 均为 async-signal-safe 的系统调用；本调用只
    // 设置一个常量选项并读取一个整数，不传指针、不分配内存，可在 `pre_exec` 闭包内执行。
    let configured = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) };
    if configured != 0 {
        // 尽力而为：`prctl` 可能被沙箱的 seccomp 策略过滤，或被裁剪过的内核拒绝。缺了它
        // 只是少一层孤儿保护，而在这里让 `pre_exec` 失败会让整条命令根本无法启动：
        // 代价不对称，与 Windows 侧「Job 绑定失败不阻断子进程」同一口径。
        // 注意：`pre_exec` 闭包内不能分配内存，因此这里**不能**记录任何日志。
        return;
    }
    // 竞态收口：若父线程在 `fork` 与上面的 `prctl` 之间就已退出，内核不会再补发信号，
    // 这里用 fork 前记录的宿主 pid 比对；已被领养说明宿主已消失，立即退出。
    if host_pid != 0 && unsafe { libc::getppid() } != host_pid {
        // SAFETY: `_exit` 立即终止进程且不运行析构，是 async-signal-safe 的。
        unsafe { libc::_exit(HOST_GONE_EXIT_CODE) };
    }
}

/// 其余 Unix（macOS / BSD）：无等价接口，保持原有行为。
#[cfg(all(unix, not(target_os = "linux")))]
fn apply_parent_death_signal(_host_pid: libc::pid_t) {}

/// 已折算完成的软限制；每个字段都是可以直接交给 `setrlimit` 的最终值。
///
/// 折算（读取硬限制、施加下限）必须发生在 fork **之前**，闭包内只做纯粹的 `setrlimit`。
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct EffectiveUnixLimits {
    file_bytes: Option<u64>,
    processes: Option<u64>,
    open_files: Option<u64>,
    address_space: Option<u64>,
}

#[cfg(unix)]
impl EffectiveUnixLimits {
    /// 把期望值折算成内核会接受的软限制。
    fn resolve(limits: ProcessResourceLimits) -> Self {
        Self {
            file_bytes: unix_soft_limit(UnixRlimitKind::FileSize, limits.max_file_bytes, None),
            processes: unix_soft_limit(
                UnixRlimitKind::Processes,
                limits.max_processes,
                Some(MIN_RLIMIT_NPROC),
            ),
            open_files: unix_soft_limit(
                UnixRlimitKind::OpenFiles,
                limits.max_open_files,
                Some(MIN_RLIMIT_NOFILE),
            ),
            address_space: unix_soft_limit(
                UnixRlimitKind::AddressSpace,
                limits.max_address_space_bytes,
                None,
            ),
        }
    }

    /// 施加全部软限制。
    ///
    /// 本方法体内只允许出现 async-signal-safe 的调用：它会在 `pre_exec` 闭包
    /// （fork 之后、exec 之前）中执行。
    fn apply(self) -> io::Result<()> {
        set_unix_limit(libc::RLIMIT_FSIZE, self.file_bytes)?;
        set_unix_limit(libc::RLIMIT_NPROC, self.processes)?;
        set_unix_limit(libc::RLIMIT_NOFILE, self.open_files)?;
        set_unix_limit(libc::RLIMIT_AS, self.address_space)?;
        Ok(())
    }
}

/// `getrlimit` 能读取的四类限额。
#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
enum UnixRlimitKind {
    FileSize,
    Processes,
    OpenFiles,
    AddressSpace,
}

#[cfg(unix)]
impl UnixRlimitKind {
    const fn resource(self) -> RlimitResource {
        match self {
            Self::FileSize => libc::RLIMIT_FSIZE,
            Self::Processes => libc::RLIMIT_NPROC,
            Self::OpenFiles => libc::RLIMIT_NOFILE,
            Self::AddressSpace => libc::RLIMIT_AS,
        }
    }
}

/// 读取当前进程某一维度的 `(软限制, 硬限制)`。
#[cfg(unix)]
fn unix_limit_pair(kind: UnixRlimitKind) -> (u64, u64) {
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `current` 是本栈帧上已初始化的 `libc::rlimit`，指针在整个调用期间有效；
    // `getrlimit` 只写入该结构，不保留指针。
    let read = unsafe { libc::getrlimit(kind.resource(), &mut current) };
    if read == 0 {
        (current.rlim_cur, current.rlim_max)
    } else {
        (libc::RLIM_INFINITY, libc::RLIM_INFINITY)
    }
}

/// 计算实际软限制。
///
/// `floor` 施加在**期望值**上（而非硬限制上），再对真实硬限制取 min：
/// `setrlimit` 只能降低硬限制，取 min 可让受限容器静默生效而不是报错；
/// 不存在的硬限制（`RLIM_INFINITY`）等价于「无上限」。
#[cfg(unix)]
fn unix_soft_limit(kind: UnixRlimitKind, desired: Option<u64>, floor: Option<u64>) -> Option<u64> {
    let mut desired = desired?;
    if let Some(floor) = floor {
        desired = desired.max(floor);
    }
    let (_, hard) = unix_limit_pair(kind);
    if hard == libc::RLIM_INFINITY {
        return Some(desired);
    }
    Some(desired.min(hard))
}

/// 设置单个 rlimit 的软限制，硬限制保持不变。
///
/// 只改 `rlim_cur`：`rlim_max` 必须原样保留，否则相当于把宿主给的上限抬回去。
#[cfg(unix)]
fn set_unix_limit(resource: RlimitResource, desired_soft_limit: Option<u64>) -> io::Result<()> {
    let Some(desired_soft_limit) = desired_soft_limit else {
        return Ok(());
    };
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: 同 `unix_limit_pair`：结构在本栈帧内、指针有效、调用不保留指针。
    if unsafe { libc::getrlimit(resource, &mut current) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let updated = libc::rlimit {
        rlim_cur: desired_soft_limit,
        rlim_max: current.rlim_max,
    };
    // SAFETY: `updated` 是本栈帧上已初始化的 `libc::rlimit`，指针有效；`setrlimit`
    // 只读取该结构。本调用位于 `pre_exec` 内，而 `setrlimit` 是 async-signal-safe 的。
    if unsafe { libc::setrlimit(resource, &updated) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// 创建带限额的 Job Object 并登记到命令上；命令启动后由 [`assign_child_to_job`] 绑定。
#[cfg(windows)]
fn create_job_object_and_assign(
    child: &std::process::Child,
    limits: ProcessResourceLimits,
) -> io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

    let job = create_job_object(limits)?;
    // SAFETY: `job` 是 `CreateJobObjectW` 的成功返回值，`child` 的进程句柄由标准库持有
    // 且在本调用期间有效；两个句柄都未被本调用转移所有权。
    let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) };
    if assigned == 0 {
        // SAFETY: 赋值失败时 Job 句柄没有其他所有者，这里必须立刻归还未使用的句柄。
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(job);
        }
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

/// 创建 Job Object 并写入进程数与内存限额；无限额的维度不设置对应标志。
#[cfg(windows)]
fn create_job_object(
    limits: ProcessResourceLimits,
) -> io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        },
    };

    // SAFETY: 两个空指针参数表示「默认安全属性、匿名 Job」，是 `CreateJobObjectW`
    // 文档允许的取值；返回值在本函数内被检查有效性。
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // 无条件置「Job 句柄全部关闭时终止作业内所有进程」。
    //
    // 语义正好是我们要的：句柄由 `ProcessFenceHandle` 持有，随会话/作业条目存活：
    // 因此 Daemon 存活期间后台作业照常跨 turn 读取；而 Axon 进程退出（正常或崩溃）时
    // 句柄随之关闭，子进程被内核回收，不会变成无人可管的孤儿进程（R2 验证 §5.2：
    // 修复前实测父进程已退出、作业子进程仍在运行，且回执只能报「Job 不存在」）。
    information.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if let Some(max_processes) = limits.max_processes {
        information.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        information.BasicLimitInformation.ActiveProcessLimit = u32::try_from(max_processes)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Windows 进程数上限超出 u32 范围",
                )
            })?;
    }
    if let Some(max_address_space_bytes) = limits.max_address_space_bytes {
        information.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        information.ProcessMemoryLimit =
            usize::try_from(max_address_space_bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Windows 内存上限超出 usize 范围",
                )
            })?;
    }
    if information.BasicLimitInformation.LimitFlags != 0 {
        let size = u32::try_from(std::mem::size_of_val(&information)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Job Object 结构长度超出 u32 范围",
            )
        })?;
        // SAFETY: `job` 是有效 Job 句柄；`information` 是本栈帧上已初始化的结构，
        // `size` 与其真实长度一致，`SetInformationJobObject` 只读取该缓冲区。
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&information).cast(),
                size,
            )
        };
        if configured == 0 {
            // SAFETY: 配置失败时 Job 句柄没有其他所有者，这里立刻归还。
            unsafe {
                CloseHandle(job);
            }
            return Err(io::Error::last_os_error());
        }
    }
    Ok(job)
}

/// 把子进程输出字节解码为文本：优先 UTF-8，非法序列按平台控制台代码页回退解码。
///
/// 为什么必须回退：Axon 以 `CREATE_NO_WINDOW` 启动子进程（见 crate 根
/// `configure_non_interactive_process`），子进程**没有控制台**，.NET/PowerShell 于是按
/// **系统 OEM 代码页**输出本地化文本（简体中文 Windows 为 936/GBK）。典型触发场景是
/// PowerShell 的**解析错误**：命令文本内的编码设置尚未执行就解析失败，于是错误文本以
/// GBK 送出。只按 UTF-8 解码会把诊断整段变成替换字符，模型据此无法自我修复：实测
/// 回执中 `“<”运算符保留供将来使用。` 被解成 `��<���…`（R2 验证 §5.4）。
///
/// 原生 Windows 工具（`ipconfig`、`chcp`、旧版构建工具等）同样输出 OEM 代码页，因此本
/// 函数对 stdout/stderr 两条流都适用。
///
/// 放在本模块是仓库约束所致：全仓 `unsafe` 只允许出现在此文件。
#[must_use]
pub fn decode_console_bytes(bytes: &[u8]) -> String {
    if let Some(text) = try_decode_console_bytes(bytes) {
        return text;
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// 与 [`decode_console_bytes`] 同一解码顺序，但**无法无损解码时返回 `None`** 而不是降级为
/// 替换字符。
///
/// 调用方需要「这段字节能否完整解码」这一事实时用它：例如流式解码器判断末尾是否为
/// 半个多字节字符（`None` 即说明尾巴不完整，应当留到下一块）。
#[must_use]
pub fn try_decode_console_bytes(bytes: &[u8]) -> Option<String> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some(text.to_owned());
    }
    try_decode_console_code_page(bytes)
}

/// **只**按平台控制台代码页解码，不尝试 UTF-8；无法无损解码时返回 `None`。
///
/// 已经判定「整条流不是 UTF-8」的调用方必须用它：代码页里存在恰好是合法 UTF-8 的字节对
/// （实测 `CA B9` 既是 GBK 的「使」也是合法的 UTF-8 `ʹ`），若在这里再试一次 UTF-8，就会
/// 把已经判定过的流重新按 UTF-8 切开。
///
/// 非 Windows 平台没有控制台代码页概念，直接返回 `None`（调用方按有损解码兜底）。
#[must_use]
pub fn try_decode_console_code_page(bytes: &[u8]) -> Option<String> {
    #[cfg(windows)]
    {
        // 先按 OEM 代码页（子进程无控制台时的默认），再按 ANSI 代码页兜底。
        if let Some(text) = decode_with_code_page(bytes, CODE_PAGE_OEM) {
            return Some(text);
        }
        if let Some(text) = decode_with_code_page(bytes, CODE_PAGE_ANSI) {
            return Some(text);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = bytes;
    }
    None
}

/// `CP_OEMCP`：系统 OEM 代码页（无控制台进程的默认输出代码页）。
#[cfg(windows)]
const CODE_PAGE_OEM: u32 = 1;
/// `CP_ACP`：系统 ANSI 代码页。
#[cfg(windows)]
const CODE_PAGE_ANSI: u32 = 0;

/// 用指定代码页解码；含无法映射的字节时返回 `None`（交由下一候选代码页处理）。
#[cfg(windows)]
fn decode_with_code_page(bytes: &[u8], code_page: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};

    let Ok(length) = i32::try_from(bytes.len()) else {
        return None;
    };
    // SAFETY: 首次调用传入空输出缓冲，仅用于查询所需宽字符数（Win32 约定）；
    // `bytes` 与 `length` 一致，函数只读取该范围。
    let wide_len = unsafe {
        MultiByteToWideChar(
            code_page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            length,
            std::ptr::null_mut(),
            0,
        )
    };
    let Ok(wide_len) = usize::try_from(wide_len) else {
        return None;
    };
    if wide_len == 0 {
        return None;
    }
    let Ok(capacity) = i32::try_from(wide_len) else {
        return None;
    };
    let mut wide = vec![0_u16; wide_len];
    // SAFETY: `wide` 恰好 `wide_len` 个 `u16`，与上一步查询到的长度一致，写入不越界。
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            length,
            wide.as_mut_ptr(),
            capacity,
        )
    };
    let Ok(written) = usize::try_from(written) else {
        return None;
    };
    if written == 0 {
        return None;
    }
    wide.truncate(written);
    Some(String::from_utf16_lossy(&wide))
}

#[cfg(test)]
#[path = "resource_fence_tests.rs"]
mod resource_fence_tests;
