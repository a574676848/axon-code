//! 网关可用性探测与修复引导。
//!
//! 应用启动即探测一次，探测失败进入未连接态并给出可执行的修复动作，
//! 而不是让侧栏静默空白。
//!
//! 探测口径说明：daemon 网关**不提供**免鉴权的 `/v1/health`（那是
//! `axon serve` 的路由），且它的全部路由都要求凭据。因此这里用两段式探测：
//! 先做监听连通性判断，再用一次带鉴权的真实接口调用确认协议确实可用。
//! 只有两段都通过才算已连接，避免把「端口有人监听但协议不通」误报成可用。

use serde::{Deserialize, Serialize};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use crate::gateway;

/// 单次连接探测超时。
///
/// 网关的合法应答实测可到 1.9 秒，后台运行态存储的等锁上限是 5 秒；1.5 秒的预算
/// 会把「网关正忙」误报成「网关无响应」。3 秒叠加一次退避重试，既能滤掉瞬时排队，
/// 又能在真正不可用时及时给出提示。
const PROBE_TIMEOUT: Duration = Duration::from_millis(3000);
/// 协议确认失败后的重试次数。
const PROBE_RETRY_LIMIT: usize = 1;
/// 重试前的退避时长。
const PROBE_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// 网关可用性。
///
/// 运行态**只由 HTTP 响应推断**，不起任何子进程：
/// 客户端问「服务在不在」就应该直接问网关，而不是去唤醒一个 CLI 进程来转述。
/// 能应答即视为服务在运行；连接被拒即为未运行；状态码再细分凭据与版本问题。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayHealthDto {
    /// 协议是否可用：监听且鉴权调用成功。
    pub connected: bool,
    /// 监听地址是否有人接受连接。
    pub listening: bool,
    /// 网关是否有 HTTP 响应（即便状态码不是 2xx）。
    pub responding: bool,
    /// 网关监听地址。
    pub target: String,
    /// 一句话原因，直接展示给用户。
    pub reason: String,
    /// 修复建议，按推荐顺序排列。
    pub actions: Vec<String>,
}

/// 判断监听地址是否有人接受连接。
fn is_listening(target: &str) -> bool {
    let Ok(mut addresses) = target.to_socket_addrs() else {
        return false;
    };
    addresses.any(|address| TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok())
}

/// 已安装的 Axon CLI 或稳定 launcher 可执行文件路径。
///
/// 服务操作与后台健康快照共用这一处解析：桌面端调用的那个 `axon` 必须与
/// 安装树里的稳定入口是同一份，不能各自拼路径。
pub(crate) fn installed_cli() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for variable in ["USERPROFILE", "HOME"] {
        if let Ok(home) = std::env::var(variable) {
            let root = PathBuf::from(home).join(".axon").join("bin");
            candidates.push(root.join("launcher").join(if cfg!(windows) {
                "axon-launcher.exe"
            } else {
                "axon-launcher"
            }));
            candidates.push(root.join(if cfg!(windows) { "axon.exe" } else { "axon" }));
            candidates.push(root.join("axon.cmd"));
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let root = PathBuf::from(&local).join("Axon");
        candidates.push(root.join("launcher").join(if cfg!(windows) {
            "axon-launcher.exe"
        } else {
            "axon-launcher"
        }));
        candidates.push(
            root.join("bin")
                .join(if cfg!(windows) { "axon.exe" } else { "axon" }),
        );
        candidates.push(root.join("bin").join("axon.cmd"));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(&profile)
                .join(".cargo")
                .join("bin")
                .join("axon.exe"),
        );
        candidates.push(
            PathBuf::from(&profile)
                .join(".cargo")
                .join("bin")
                .join("axon"),
        );
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join(".cargo").join("bin").join("axon"));
        candidates.push(
            PathBuf::from(&home)
                .join(".local")
                .join("share")
                .join("Axon")
                .join("launcher")
                .join("axon-launcher"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// Tauri IPC 命令：探测网关是否可用。
///
/// **全程只做 HTTP 请求，不起任何子进程**：客户端询问服务状态就该直接问网关。
pub fn gateway_health_sync() -> GatewayHealthDto {
    // 探活是界面心跳的一部分：这里报到一次，宿主据此判断界面是否还活着。
    crate::ui_failure::note_ui_alive();
    let bind = gateway::config_text(gateway::KEY_HTTP_BIND).unwrap_or_default();
    let displayed = if bind.trim().is_empty() {
        "未配置".to_string()
    } else {
        bind.clone()
    };
    let target = gateway::probe_target(&displayed);
    let listening = !bind.trim().is_empty() && is_listening(&target);

    // 监听通过后必须再验证协议：端口有人监听不代表就是我们期待的网关。
    // 失败先重试一次：网关排队是瞬时的，一次重试就能把「偶发慢」与「真的不可用」分开。
    let mut failure: Option<gateway::ProbeFailure> = None;
    let mut probe = None;
    if listening {
        for attempt in 0..=PROBE_RETRY_LIMIT {
            if attempt > 0 {
                std::thread::sleep(PROBE_RETRY_BACKOFF);
            }
            match gateway::probe_raw_with_timeout("/workspaces", PROBE_TIMEOUT) {
                Ok(response) => {
                    probe = Some(response);
                    break;
                }
                Err(error) => failure = Some(error),
            }
        }
    }
    let responding = probe.is_some();
    let connected = matches!(&probe, Some((status, _)) if (200..300).contains(status));

    let (reason, actions) = if connected {
        ("网关已连接，运行态数据可用".to_string(), Vec::new())
    } else if !listening {
        (
            "网关未监听，Axon 后台服务可能未启动".to_string(),
            vec![
                "启动服务".to_string(),
                "打开配置文件".to_string(),
                "打开日志目录".to_string(),
                "重试".to_string(),
            ],
        )
    } else if !responding {
        (
            probe_failure_reason(failure),
            vec!["重试".to_string(), "打开配置文件".to_string()],
        )
    } else {
        // 已监听但接口不通：按状态码区分，两类问题的修复动作完全不同。
        match &probe {
            Some((401 | 403, _)) => (
                "网关已监听，但访问凭据缺失或不匹配".to_string(),
                vec![
                    "配置网关密钥".to_string(),
                    "打开配置文件".to_string(),
                    "重试".to_string(),
                ],
            ),
            Some((404, _)) => (
                "网关已监听，但当前运行的后台服务不含工作台接口，通常是服务版本过旧".to_string(),
                vec![
                    "升级后台服务".to_string(),
                    "打开配置文件".to_string(),
                    "重试".to_string(),
                ],
            ),
            Some((status, _)) => (
                format!("网关已监听，但接口返回状态码 {status}"),
                vec!["重试".to_string(), "打开配置文件".to_string()],
            ),
            None => (
                "网关已监听，但请求未能完成".to_string(),
                vec!["重试".to_string(), "打开配置文件".to_string()],
            ),
        }
    };

    GatewayHealthDto {
        connected,
        listening,
        responding,
        target,
        reason,
        actions,
    }
}

/// 探活失败的一句话原因；分类缺失时退回原口径。
fn probe_failure_reason(failure: Option<gateway::ProbeFailure>) -> String {
    match failure {
        Some(gateway::ProbeFailure::Timeout) => {
            "网关已监听，但请求未在探活预算内完成（网关正忙）".to_string()
        }
        Some(gateway::ProbeFailure::Unreachable) => "网关已监听，但请求未能送达".to_string(),
        Some(gateway::ProbeFailure::BodyRead) => "网关已响应，但响应体未读取完整".to_string(),
        None => "网关已监听，但请求未能完成".to_string(),
    }
}

/// Tauri IPC 命令：在线程池中探测网关，避免阻塞窗口线程。
#[tauri::command]
pub async fn gateway_health() -> GatewayHealthDto {
    tauri::async_runtime::spawn_blocking(gateway_health_sync)
        .await
        .unwrap_or_else(|_| GatewayHealthDto {
            connected: false,
            listening: false,
            responding: false,
            target: String::new(),
            reason: "后台探测线程未能完成".to_string(),
            actions: vec!["重试".to_string()],
        })
}

/// 服务操作结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceActionDto {
    /// 操作是否成功。
    pub success: bool,
    /// 面向用户的结论。
    pub message: String,
    /// 原始命令输出，便于排查。
    pub output: String,
}

/// 直接以当前用户身份执行 CLI 子命令。
fn run_cli(args: &[&str]) -> Result<std::process::Output, String> {
    let cli = installed_cli().ok_or_else(|| "未找到已安装的 axon 命令".to_string())?;
    crate::process::run_output(std::process::Command::new(cli).args(args))
        .map_err(|error| format!("执行 axon {} 失败：{error}", args.join(" ")))
}

/// 启动后台服务；服务尚未注册时先补齐用户服务定义。
fn run_start_with_install_fallback() -> Result<std::process::Output, String> {
    let started = run_cli(&["daemon", "start"]);
    if started.as_ref().is_ok_and(|output| output.status.success()) {
        return started;
    }
    let installed = run_cli(&["daemon", "install"])?;
    if !installed.status.success() {
        return Ok(installed);
    }
    run_cli(&["daemon", "start"])
}

/// 以提权方式执行 CLI 子命令。
///
/// 复用系统既有的 `-Verb RunAs` 自举方式：由 PowerShell 弹 UAC，
/// 用户确认后在新进程中执行，本进程等待其结束。
/// 用户取消提权时不会假装成功，而是如实返回失败。
#[cfg(windows)]
fn run_cli_elevated(args: &[&str]) -> Result<std::process::Output, String> {
    let cli = installed_cli().ok_or_else(|| "未找到已安装的 axon 命令".to_string())?;
    let argument_list = args
        .iter()
        .map(|item| format!("'{}'", item.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let command = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList @({argument_list}) -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
        cli.to_string_lossy().replace('\'', "''")
    );
    let encoded: String = {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new(&command)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut bytes = Vec::with_capacity(wide.len() * 2);
        for unit in &wide[..wide.len() - 1] {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        base64_encode(&bytes)
    };
    crate::process::run_output(std::process::Command::new("pwsh").args([
        "-NoProfile",
        "-EncodedCommand",
        &encoded,
    ]))
    .map_err(|error| format!("提权执行失败：{error}"))
}

/// 非 Windows 平台没有 UAC 提权通道：显式失败，不伪装成功。
#[cfg(not(windows))]
fn run_cli_elevated(_args: &[&str]) -> Result<std::process::Output, String> {
    Err("当前平台不支持 UAC 提权：请使用系统服务管理器安装守护进程".to_string())
}

/// 标准 base64 编码；避免为一次编码引入依赖。
#[cfg(windows)]
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Tauri IPC 命令：启动、注册或重启 Axon 后台服务。
///
/// 先以当前用户身份尝试，失败再走提权路径；两条路径的结果都如实返回，
/// 用户取消提权时不会谎报已恢复。
pub fn gateway_start_service_sync(mode: String) -> ServiceActionDto {
    let step: &[&str] = match mode.as_str() {
        "install" => &["daemon", "install"],
        "restart" => &["daemon", "restart"],
        _ => &["daemon", "start"],
    };

    let direct = if mode == "start" {
        run_start_with_install_fallback().map(|output| {
            (
                output.status.success(),
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            )
        })
    } else {
        run_cli(step).map(|output| {
            (
                output.status.success(),
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            )
        })
    };

    match direct {
        Ok((true, stdout, _)) => ServiceActionDto {
            success: true,
            message: "服务操作已完成".to_string(),
            output: stdout,
        },
        _ => {
            // 非提权执行未成功：改用提权路径再试一次。
            let elevated = if mode == "start" {
                run_cli_elevated(&["daemon", "install"]).and_then(|installed| {
                    if installed.status.success() {
                        run_cli_elevated(&["daemon", "start"])
                    } else {
                        Ok(installed)
                    }
                })
            } else {
                run_cli_elevated(step)
            };
            match elevated {
                Ok(output) if output.status.success() => ServiceActionDto {
                    success: true,
                    message: "已通过提权完成服务操作".to_string(),
                    output: String::from_utf8_lossy(&output.stdout).to_string(),
                },
                Ok(output) => ServiceActionDto {
                    success: false,
                    message: "提权操作未成功；若在弹窗中点了取消，服务状态不会改变".to_string(),
                    output: format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                },
                Err(error) => ServiceActionDto {
                    success: false,
                    message: error,
                    output: String::new(),
                },
            }
        }
    }
}

/// Tauri IPC 命令：在线程池中执行服务启动、安装或重启。
#[tauri::command]
pub async fn gateway_start_service(mode: String) -> ServiceActionDto {
    tauri::async_runtime::spawn_blocking(move || gateway_start_service_sync(mode))
        .await
        .unwrap_or_else(|_| ServiceActionDto {
            success: false,
            message: "后台服务操作线程未能完成".to_string(),
            output: String::new(),
        })
}

// 被测函数只在 Windows 编译（UAC 提权路径专用）：测试同样按平台收敛，
// 否则 Linux/macOS 的 `--all-targets` 会因符号被 cfg 掉而报 E0432。
#[cfg(all(test, windows))]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_known_vectors_without_padding_regressions() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    }
}
