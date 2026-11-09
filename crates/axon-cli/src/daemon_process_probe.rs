//! 守护进程进程身份探测。
//!
//! 进程"存活"与"仍是守护进程"是两件事：PID 会被系统回收，陈旧 pid 文件里的数字
//! 很快可能属于无关进程，只按存活判定会让守护进程误以为已有实例在运行。

/// 判断 PID 是否仍由 Axon 守护进程持有。
///
/// Windows 上必须同时核对映像名，否则 PID 被回收给无关进程后，`daemon.pid` 会被
/// 误当成「已有实例在运行」，守护进程将永久拒绝启动。POSIX 上 PID 回收同样存在，
/// 但读取映像名需要额外依赖，仍以存活与僵尸状态为准。
pub(crate) fn daemon_process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    return super::windows_definition::process_image_is_axon(pid);
    #[cfg(not(windows))]
    return super::process_alive(pid);
}
