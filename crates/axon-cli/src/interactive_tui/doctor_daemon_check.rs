//! doctor 的 Daemon 后台租约检查。
//!
//! 「网关有响应」只证明进程在跑；后台还在不在推进会话，只有 Daemon 自己的租约读数说得清。
//! 租约失效后后台 Worker 全部停下，网关照常应答、`daemon status` 照常报 running，用户侧却是
//! 每条消息都发不出去。doctor 其余各项都不看这一层，于是会给出「全绿但已经停摆」的结论；
//! 这一项就是补上的那道判据。
//!
//! 读数走 Daemon IPC 的 `GetHealth`，与 `axon daemon health` 同一口径：租约是进程内事实，
//! 运行态数据库里的过期时间戳只能反映最后一次成功续租，判断不出「现在还有没有在续租」。

use axon_daemon::infrastructure::ipc::JsonlIpcClient;
use axon_protocol::{DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest};

use super::doctor_commands::DoctorCheck;
use crate::DOCTOR_CHECK_DAEMON;

/// 健康快照里的 host 租约读数。
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostLease {
    /// 后台 host 代号，形如 `axon-daemon-23356`。
    host_id: String,
    /// 租约剩余毫秒；0 表示已失效。
    remaining_ms: u64,
}

/// 租约检查的三种结论。
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostLeaseVerdict {
    /// Daemon 没在运行或 IPC 不可达：本项不适用，不判失败。
    NotRunning(String),
    /// 后台 host 正在续租。
    Renewing(HostLease),
    /// 进程在跑但租约已失效：后台 Worker 已停机。
    Fenced(HostLease),
}

/// doctor 的 daemon 检查入口。
pub(super) fn doctor_daemon_check() -> DoctorCheck {
    match classify_host_lease(read_host_lease()) {
        HostLeaseVerdict::NotRunning(reason) => DoctorCheck::ok(
            DOCTOR_CHECK_DAEMON,
            format!("Daemon 未运行或 IPC 不可达（{reason}），本次跳过租约检查"),
        ),
        HostLeaseVerdict::Renewing(lease) => DoctorCheck::ok(
            DOCTOR_CHECK_DAEMON,
            format!(
                "后台 host 正在续租：host={} lease={}ms",
                lease.host_id, lease.remaining_ms
            ),
        ),
        // 这条结论必须显式给恢复动作：用户在这里能拿到的唯一线索就是它。
        HostLeaseVerdict::Fenced(lease) => DoctorCheck::failed(
            DOCTOR_CHECK_DAEMON,
            format!(
                "后台 host 租约已失效（host={}）：进程在跑但后台 Worker 已停机，\
                 新会话不会被领取；执行 `axon daemon restart` 恢复",
                lease.host_id
            ),
        ),
    }
}

/// 把健康快照读数归类成结论。
fn classify_host_lease(lease: Result<HostLease, String>) -> HostLeaseVerdict {
    match lease {
        Ok(lease) if lease.remaining_ms > 0 => HostLeaseVerdict::Renewing(lease),
        Ok(lease) => HostLeaseVerdict::Fenced(lease),
        Err(reason) => HostLeaseVerdict::NotRunning(reason),
    }
}

/// 读一次 Daemon 健康快照里的 host 租约。
fn read_host_lease() -> Result<HostLease, String> {
    let endpoint = crate::runtime_paths::resolve_daemon_endpoint(
        crate::daemon_commands::DEFAULT_ENDPOINT,
        None,
    )?;
    let request = DaemonRequest {
        message_type: "command".to_owned(),
        request_id: format!("doctor-daemon-{}", crate::daemon_commands::now_ms()),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: "axon-doctor".to_owned(),
        session_id: None,
        command: DaemonCommand::GetHealth,
    };
    let response = JsonlIpcClient::connect(&endpoint)
        .and_then(|mut client| {
            client.send_with_timeout(&request, crate::daemon_commands::DAEMON_IPC_TIMEOUT)
        })
        .map_err(|error| error.to_string())?;
    if !response.ok {
        return Err(format!("Daemon 拒绝健康查询：{}", response.body));
    }
    let remaining_ms = response
        .body
        .pointer("/host/leaseRemainingMs")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "健康快照缺少 host.leaseRemainingMs".to_owned())?;
    let host_id = response
        .body
        .pointer("/host/hostId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    Ok(HostLease {
        host_id,
        remaining_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(remaining_ms: u64) -> HostLease {
        HostLease {
            host_id: "axon-daemon-1".to_owned(),
            remaining_ms,
        }
    }

    #[test]
    fn expired_lease_is_reported_as_a_failure() {
        let verdict = classify_host_lease(Ok(lease(0)));
        assert_eq!(verdict, HostLeaseVerdict::Fenced(lease(0)));
    }

    #[test]
    fn renewing_lease_is_reported_as_healthy() {
        let verdict = classify_host_lease(Ok(lease(23_568)));
        assert_eq!(verdict, HostLeaseVerdict::Renewing(lease(23_568)));
    }

    /// Daemon 没装或没起来是常态，doctor 不能因此变红：没读数不等于有问题。
    #[test]
    fn unreachable_daemon_is_not_a_failure() {
        let verdict = classify_host_lease(Err("IPC 不可达".to_owned()));
        assert_eq!(
            verdict,
            HostLeaseVerdict::NotRunning("IPC 不可达".to_owned())
        );
    }
}
