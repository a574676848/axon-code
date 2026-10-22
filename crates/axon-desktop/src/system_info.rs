//! 系统信息：运行态服务、后台健康快照、远控状态与版本。
//!
//! 服务面一律经网关的 HTTP 协议读取：桌面端用已在依赖白名单内的 `ureq` 直接查询，
//! 既不依赖 daemon crate，也不去直接读运行态数据库。
//!
//! 网关应答只说明进程在跑；后台 Worker 是否还在推进会话，只有 daemon 自己的健康
//! 快照说得清（租约与排队读数）。因此本模块额外调用一次 `axon daemon health`：
//! 这是唯一的子进程调用，只读快照，不改服务状态，失败原因原样上屏。
//!
//! 所有探测失败都如实呈现为「未监听」「读不到」或错误说明，绝不伪造成在线。
//! 发布渠道与更新检测不在这里，见 [`crate::release_channel`]。

use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use axon_config::ConfigLoader;
use serde::{Deserialize, Serialize};

use crate::gateway;
use crate::release_channel::{GithubInfoDto, github_info};

/// 单次探测超时，避免界面被不可达端点挂住。
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 远控 adapter 状态文件名。
const GATEWAY_STATUS_FILE: &str = "gateway-status.json";

/// 网关配置键。
const KEY_HTTP_BIND: &str = "remote.gateway.http_bind";
/// 网关 WebSocket 配置键。
const KEY_WEBSOCKET_BIND: &str = "remote.gateway.websocket_bind";

/// 网关监听地址探测结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceProbeDto {
    /// 面名称，例如 HTTP 网关。
    pub name: String,
    /// 监听地址。
    pub target: String,
    /// 是否已在监听。
    pub listening: bool,
    /// 补充说明：版本、就绪状态或失败原因。
    pub detail: String,
}

/// 键值对展示项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValueDto {
    /// 名称。
    pub key: String,
    /// 值。
    pub value: String,
}

/// 远控 adapter 状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteStatusDto {
    /// adapter 是否已连接。
    pub connected: bool,
    /// 连接状态的中文说明。
    pub state_label: String,
    /// 一行补充说明，例如运行时长与最近错误。
    pub detail: String,
}

/// daemon 后台健康快照。
///
/// 取自 `axon daemon health`：租约与排队读数是 daemon 自身 IPC 的权威口径，
/// 网关 HTTP 面没有等价字段。快照读不到时为 None，界面据此明说读不到，
/// 而不是把缺读数当成「一切正常」。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonHealthDto {
    /// 后台 host 代号，形如 `axon-daemon-29296`。
    pub host_id: String,
    /// 租约剩余毫秒；0 表示租约已失效、后台 Worker 已停机。
    pub lease_remaining_ms: u64,
    /// 已接收但尚未被领取的会话数。
    pub interactions_pending: u64,
    /// 已被领取、正在处理的会话数。
    pub interactions_processing: u64,
    /// 待投递的回复数。
    pub deliveries_pending: u64,
}

/// 系统信息汇总。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoDto {
    /// 桌面端版本。
    pub app_version: String,
    /// daemon 是否已注册为服务。
    pub daemon_installed: bool,
    /// daemon 是否在运行。
    pub daemon_running: bool,
    /// daemon 状态的补充说明。
    pub daemon_detail: String,
    /// daemon 状态结论：`running`、`workers_stopped`、`degraded` 或 `stopped`。
    pub daemon_state: String,
    /// daemon 后台健康快照；读不到时为 None。
    pub daemon_health: Option<DaemonHealthDto>,
    /// 后台健康快照读不到的原因；成功时为空串。
    pub daemon_health_error: String,
    /// 网关各面的探测结果。
    pub services: Vec<ServiceProbeDto>,
    /// 飞书远控状态；无状态文件时为空。
    pub remote_feishu: Option<RemoteStatusDto>,
    /// 发布渠道坐标：关于面板的仓库、发布页与反馈入口都由它派生。
    pub github: GithubInfoDto,
    /// 配置目录。
    pub runtime_root: String,
}

/// 解析运行时根目录。
fn axon_root() -> Result<PathBuf, String> {
    ConfigLoader::default_user_axon_root_path()
        .map_err(|error| format!("无法解析 Axon 运行时根目录：{error:?}"))
}

/// 纯 TCP 探测：能建立连接即视为在监听。
fn tcp_listening(target: &str) -> bool {
    let Ok(mut addresses) = target.to_socket_addrs() else {
        return false;
    };
    addresses.any(|address| TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok())
}

/// daemon 结论标识：网关有响应，且后台租约有效。
const DAEMON_STATE_RUNNING: &str = "running";
/// daemon 结论标识：网关有响应，但后台租约失效、Worker 已停机。
const DAEMON_STATE_WORKERS_STOPPED: &str = "workers_stopped";
/// daemon 结论标识：网关有响应但协议不可用（凭据、版本或状态码）。
const DAEMON_STATE_DEGRADED: &str = "degraded";
/// daemon 结论标识：网关无响应。
const DAEMON_STATE_STOPPED: &str = "stopped";

/// daemon 运行状态：结论标识与一句话说明。
///
/// 判定只用两件事实：网关 HTTP 是否应答（进程在不在），以及后台健康快照里的租约
/// （Worker 还在不在推进会话）。「网关应答但租约失效」正是界面可用、会话不回的那种
/// 停摆，必须与正常态分开报，否则用户没有任何线索。
fn daemon_status(health: &Result<DaemonHealthDto, String>) -> (bool, bool, &'static str, String) {
    let bind = gateway::config_text(gateway::KEY_HTTP_BIND).unwrap_or_default();
    if bind.trim().is_empty() {
        return (
            false,
            false,
            DAEMON_STATE_STOPPED,
            "未配置网关监听地址".to_string(),
        );
    }
    let target = gateway::probe_target(&bind);
    match gateway::get_raw("/workspaces") {
        Ok((status, _)) if (200..300).contains(&status) => match health {
            Ok(snapshot) if snapshot.lease_remaining_ms > 0 => {
                (true, true, DAEMON_STATE_RUNNING, running_detail(snapshot))
            }
            Ok(_) => (
                true,
                true,
                DAEMON_STATE_WORKERS_STOPPED,
                "网关有响应，但后台租约已失效：新会话不会被领取，重启服务即可恢复".to_string(),
            ),
            Err(_) => (
                true,
                true,
                DAEMON_STATE_RUNNING,
                "网关已连接，服务在运行".to_string(),
            ),
        },
        Ok((401 | 403, _)) => (
            false,
            true,
            DAEMON_STATE_DEGRADED,
            "网关已监听但访问凭据缺失或不匹配".to_string(),
        ),
        Ok((404, _)) => (
            false,
            true,
            DAEMON_STATE_DEGRADED,
            "网关已监听但当前服务不含工作台接口，通常是版本过旧".to_string(),
        ),
        Ok((status, _)) => (
            false,
            true,
            DAEMON_STATE_DEGRADED,
            format!("网关已监听但返回状态码 {status}"),
        ),
        Err(_) => (
            false,
            false,
            DAEMON_STATE_STOPPED,
            format!("网关 {target} 无响应，后台服务可能未启动"),
        ),
    }
}

/// 正常态的一句话说明；后台代号缺失时退回不带代号的表述。
fn running_detail(snapshot: &DaemonHealthDto) -> String {
    if snapshot.host_id.is_empty() {
        return "网关已连接，服务在运行".to_string();
    }
    format!("网关已连接，后台 {} 正在续租", snapshot.host_id)
}

/// 读取 daemon 后台健康快照。
///
/// 租约与排队读数只存在于 daemon 自身的 IPC，网关 HTTP 面没有等价接口，因此这是
/// 本模块唯一一处子进程调用：只读快照、不改服务状态，失败原因原样交给界面。
fn daemon_health_snapshot() -> Result<DaemonHealthDto, String> {
    let cli = crate::service_recovery::installed_cli()
        .ok_or_else(|| "未找到已安装的 axon 命令".to_string())?;
    let output =
        crate::process::run_output(std::process::Command::new(cli).args(["daemon", "health"]))
            .map_err(|error| format!("启动 axon daemon health 失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "daemon 未响应健康查询".to_string()
        } else {
            stderr
        });
    }
    parse_daemon_health(&String::from_utf8_lossy(&output.stdout))
}

/// 从 `axon daemon health` 的 JSON 快照里取出租约与排队读数。
///
/// 缺字段一律按 0 处理：界面据此显示「0」而不是编造一个读数，解析失败才算读不到。
fn parse_daemon_health(text: &str) -> Result<DaemonHealthDto, String> {
    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|_| "健康快照不是合法 JSON".to_string())?;
    Ok(DaemonHealthDto {
        host_id: value
            .pointer("/host/hostId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        lease_remaining_ms: value
            .pointer("/host/leaseRemainingMs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        interactions_pending: interaction_count(&value, "received"),
        interactions_processing: interaction_count(&value, "processing"),
        deliveries_pending: value
            .pointer("/deliveries/backlog/pending")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    })
}

/// 读取某一状态下 Interaction 的条数。
fn interaction_count(value: &serde_json::Value, status: &str) -> u64 {
    value
        .pointer(&format!("/interactions/byStatus/{status}"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// 飞书远控状态文件可能的位置。
///
/// 权威位置是各 adapter 自己的子目录（形如 `data/remote/<adapter>/gateway-status.json`），
/// 因此先按已知目录枚举，再兜底扫描 `data/remote` 下的一层子目录，
/// 避免新增 adapter 时又要回来改路径清单。
fn feishu_status_candidates(root: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![
        root.join("data")
            .join("remote")
            .join("feishu")
            .join(GATEWAY_STATUS_FILE),
        root.join("data").join(GATEWAY_STATUS_FILE),
        root.join(GATEWAY_STATUS_FILE),
    ];
    if let Ok(entries) = fs::read_dir(root.join("data").join("remote")) {
        for entry in entries.flatten() {
            let path = entry.path().join(GATEWAY_STATUS_FILE);
            if path.is_file() && !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }
    candidates
}

/// 读取飞书远控状态。
///
/// 状态文件由 adapter 自己写入，`state` 是权威字段；这里把它翻成中文，
/// 并把毫秒时间戳换算成可读的相对时间，界面不再出现原始字段名。
fn remote_feishu_status(root: &Path) -> Option<RemoteStatusDto> {
    for path in feishu_status_candidates(root) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let state = value
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let connected = state == "connected";
        let state_label = match state {
            "connected" => "已连接",
            "connecting" => "连接中",
            "disconnected" => "已断开",
            "" => "状态未知",
            other => other,
        }
        .to_string();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let mut parts: Vec<String> = Vec::new();
        if let Some(started) = value.get("startedAtMs").and_then(serde_json::Value::as_u64)
            && now_ms > started
        {
            parts.push(format!("已运行 {}", humanize_duration(now_ms - started)));
        }
        if let Some(updated) = value.get("updatedAtMs").and_then(serde_json::Value::as_u64)
            && now_ms > updated
        {
            parts.push(format!("{}前更新", humanize_duration(now_ms - updated)));
        }
        if let Some(attempts) = value
            .get("reconnectAttempts")
            .and_then(serde_json::Value::as_u64)
            && attempts > 0
        {
            parts.push(format!("重连 {attempts} 次"));
        }
        if let Some(active) = value
            .get("activeTaskCount")
            .and_then(serde_json::Value::as_u64)
        {
            parts.push(format!("进行中任务 {active}"));
        }
        if let Some(error) = value.get("lastError").and_then(serde_json::Value::as_str) {
            parts.push(format!("最近错误：{error}"));
        }

        return Some(RemoteStatusDto {
            connected,
            state_label,
            detail: parts.join(" · "),
        });
    }
    None
}

/// 把毫秒时长换算成中文可读时长。
fn humanize_duration(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds < 60 {
        return format!("{seconds} 秒");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} 分钟");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} 小时");
    }
    format!("{} 天", hours / 24)
}

/// Tauri IPC 命令：汇总系统信息。
#[tauri::command]
pub fn system_info() -> Result<SystemInfoDto, String> {
    let root = axon_root()?;

    // 网关各面只做监听探测：daemon 网关不提供 `/v1/health`（那是 serve 的路由），
    // 拿它去探必然 404，会把在监听的面误判为未监听。
    let http_bind =
        gateway::config_text(KEY_HTTP_BIND).unwrap_or_else(|| "127.0.0.1:3143".to_string());
    let ws_bind =
        gateway::config_text(KEY_WEBSOCKET_BIND).unwrap_or_else(|| "127.0.0.1:3144".to_string());

    let health = daemon_health_snapshot();
    let health_error = health.as_ref().err().cloned().unwrap_or_default();
    let (installed, running, daemon_state, detail) = daemon_status(&health);
    let http_listening = tcp_listening(&gateway::probe_target(&http_bind));
    let ws_listening = tcp_listening(&gateway::probe_target(&ws_bind));

    Ok(SystemInfoDto {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_installed: installed,
        daemon_running: running,
        daemon_detail: detail,
        daemon_state: daemon_state.to_string(),
        daemon_health: health.ok(),
        daemon_health_error: health_error,
        services: vec![
            ServiceProbeDto {
                name: "网关 HTTP".to_string(),
                target: http_bind,
                listening: http_listening,
                detail: if http_listening {
                    "已监听".to_string()
                } else {
                    "未监听".to_string()
                },
            },
            ServiceProbeDto {
                name: "网关 WebSocket".to_string(),
                target: ws_bind,
                listening: ws_listening,
                detail: if ws_listening {
                    "已监听".to_string()
                } else {
                    "未监听".to_string()
                },
            },
        ],
        remote_feishu: remote_feishu_status(&root),
        github: github_info(),
        runtime_root: root.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_daemon_health;

    /// 真实回执的字段形状：租约、排队与投递三处读数各取一条。
    const SNAPSHOT: &str = r#"{
        "deliveries": {"backlog": {"pending": 2}, "counts": {"byStatus": {}, "total": 0}},
        "host": {"hostId": "axon-daemon-29296", "leaseRemainingMs": 21300, "status": "running"},
        "interactions": {"byStatus": {"received": 1, "processing": 3, "interrupted": 8}, "total": 12}
    }"#;

    #[test]
    fn health_snapshot_reads_lease_queue_and_delivery_readings() {
        let snapshot = parse_daemon_health(SNAPSHOT).expect("合法快照应解析成功");
        assert_eq!(snapshot.host_id, "axon-daemon-29296");
        assert_eq!(snapshot.lease_remaining_ms, 21_300);
        assert_eq!(snapshot.interactions_pending, 1);
        assert_eq!(snapshot.interactions_processing, 3);
        assert_eq!(snapshot.deliveries_pending, 2);
    }

    #[test]
    fn health_snapshot_defaults_missing_readings_to_zero() {
        // 租约失效的后台仍然应答，但读数可能是缺的；缺读数按 0 报，不能猜。
        let snapshot = parse_daemon_health(r#"{"host": {"leaseRemainingMs": 0}}"#)
            .expect("字段不全的快照仍应解析成功");
        assert_eq!(
            snapshot,
            super::DaemonHealthDto {
                host_id: String::new(),
                lease_remaining_ms: 0,
                interactions_pending: 0,
                interactions_processing: 0,
                deliveries_pending: 0,
            }
        );
    }

    #[test]
    fn health_snapshot_rejects_non_json_output() {
        // daemon 未运行时 CLI 打印的是中文错误，不是 JSON，必须如实报读不到。
        assert!(parse_daemon_health("无法连接 Axon Daemon（IPC 端点 axon-daemon）").is_err());
    }
}
