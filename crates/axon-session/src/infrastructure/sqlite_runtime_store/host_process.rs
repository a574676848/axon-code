//! Daemon Host 本机进程存活探测。

#[cfg(any(unix, windows))]
use axon_process::non_interactive_command;

#[cfg(unix)]
pub(super) fn host_process_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(output) = non_interactive_command("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let state = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned();
    !state.is_empty() && !state.starts_with('Z')
}

#[cfg(windows)]
pub(super) fn host_process_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(output) = non_interactive_command("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .any(|token| token == pid.to_string())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn host_process_alive(_pid: i64) -> bool {
    true
}

/// 该 PID 是否仍是**同一个**进程。
///
/// 只按 PID 判活会在 PID 被复用时把无关进程当成宿主：租约永远不回收，新宿主也认领不到。
/// 有启动标识时用它做二次校验；取不到标识的平台上退回存活判定，不得据此判死。
pub(super) fn host_process_matches(pid: i64, process_start_id: Option<&str>) -> bool {
    let Some(expected) = process_start_id else {
        return host_process_alive(pid);
    };
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    match axon_process::process_start_id(pid) {
        Some(actual) => actual == expected,
        None => host_process_alive(i64::from(pid)),
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::host_process_alive;

    #[test]
    fn current_host_is_alive_and_invalid_pid_is_rejected() {
        assert!(host_process_alive(i64::from(std::process::id())));
        assert!(!host_process_alive(0));
        assert!(!host_process_alive(-1));
    }
}
