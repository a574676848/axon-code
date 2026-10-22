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
