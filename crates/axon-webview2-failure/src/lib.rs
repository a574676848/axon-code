//! WebView2 进程失败与内置错误页的隔离层。
//!
//! 两件事都只能经 COM 接口完成：
//!
//! - **关闭内置错误页**（`IsBuiltInErrorPageEnabled(false)`）：界面被系统回收时，
//!   浏览器不再画它自己那张「此页存在问题」，改由宿主接管；
//! - **订阅进程失败事件**（`ProcessFailed`）：进程一死就精确回调，宿主据此立刻
//!   切到随包发布的引导页，不必等心跳静默，也不必猜内存水位。
//!
//! 桌面端是 `unsafe_code = "forbid"`，因此未经安全封装的调用全部隔离在这里，
//! 桌面端只消费本 crate 的安全 API。非 Windows 平台一律返回 false，由宿主的
//! 兜底判据接管。

/// 进程失败的类别；与引导页的 `kind` 查询串一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 主渲染进程退出（包含内存不足被回收）。
    RenderExited,
    /// 主渲染进程无响应。
    RenderUnresponsive,
    /// 浏览器进程退出。
    BrowserExited,
    /// 子框架渲染进程退出。
    FrameRenderExited,
    /// GPU 进程退出。
    GpuExited,
    /// 其余进程（工具、沙箱助手等）退出。
    OtherUtility,
}

impl FailureKind {
    /// 引导页的 `kind` 取值。
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Self::RenderExited => "render-exited",
            Self::RenderUnresponsive => "render-unresponsive",
            Self::BrowserExited => "browser-exited",
            Self::FrameRenderExited => "frame-render-exited",
            Self::GpuExited | Self::OtherUtility => "utility-exited",
        }
    }
}

/// 浏览器给出的失败原因（权威判定，不是宿主猜的）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    /// 内存不足被回收。
    OutOfMemory,
    /// 进程崩溃。
    Crashed,
    /// 进程无响应后被杀。
    Unresponsive,
    /// 被外部终止。
    Terminated,
    /// 进程启动失败。
    LaunchFailed,
    /// 用户数据目录被删除。
    ProfileDeleted,
    /// 其余未归类原因。
    Unexpected,
    /// 平台没有给出原因。
    Unknown,
}

impl FailureReason {
    /// 引导页的 `reason` 取值。
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Self::OutOfMemory => "out-of-memory",
            Self::Crashed => "crashed",
            Self::Unresponsive => "unresponsive",
            Self::Terminated => "terminated",
            Self::LaunchFailed => "launch-failed",
            Self::ProfileDeleted => "profile-deleted",
            Self::Unexpected => "unexpected",
            Self::Unknown => "unknown",
        }
    }
}

/// 一次失败的完整描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureDetail {
    /// 出事的进程类别。
    pub kind: FailureKind,
    /// 浏览器给出的原因。
    pub reason: FailureReason,
    /// 退出码；平台没给时为 None。
    pub exit_code: Option<i32>,
}

/// 判断权限请求的来源是否为桌面工作台自身的本地受信任源。
///
/// 桌面端只对自身内嵌源自动放行麦克风权限，从而消除浏览器式的
/// `tauri.localhost 想要使用您的麦克风` 顶部地址栏提示，外部未知站点一律不放行。
#[must_use]
pub fn is_trusted_app_origin(uri: &str) -> bool {
    let trimmed = uri.trim().to_ascii_lowercase();
    for prefix in [
        "https://tauri.localhost",
        "http://tauri.localhost",
        "tauri://localhost",
        "https://127.0.0.1",
        "http://127.0.0.1",
        "https://localhost",
        "http://localhost",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with('/') || rest.starts_with(':'))
        {
            return true;
        }
    }
    false
}

/// 安装失败接管：关闭内置错误页，并在进程失败时回调。
///
/// 返回 true 表示接管请求已提交到界面线程（`with_webview` 是异步派发，这里无法给出
/// 同步结论）；false 表示平台不支持或派发失败，宿主应继续使用自己的兜底判据。
#[cfg(windows)]
pub fn install<F>(window: &tauri::WebviewWindow, on_failure: F) -> bool
where
    F: Fn(FailureDetail) + Send + Sync + 'static,
{
    use std::sync::Arc;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PROCESS_FAILED_KIND,
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
        COREWEBVIEW2_PROCESS_FAILED_REASON, COREWEBVIEW2_PROCESS_FAILED_REASON_CRASHED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_LAUNCH_FAILED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY,
        COREWEBVIEW2_PROCESS_FAILED_REASON_PROFILE_DELETED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_TERMINATED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_UNRESPONSIVE,
        ICoreWebView2PermissionRequestedEventArgs3, ICoreWebView2ProcessFailedEventArgs2,
    };
    use webview2_com::{PermissionRequestedEventHandler, ProcessFailedEventHandler, take_pwstr};
    use windows_core::{Interface, PWSTR};

    /// 把 WebView2 的失败类别映射成出事进程。
    fn classify(kind: COREWEBVIEW2_PROCESS_FAILED_KIND) -> FailureKind {
        if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED {
            FailureKind::RenderExited
        } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE {
            FailureKind::RenderUnresponsive
        } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
            FailureKind::BrowserExited
        } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED {
            FailureKind::FrameRenderExited
        } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED {
            FailureKind::GpuExited
        } else {
            FailureKind::OtherUtility
        }
    }

    /// 把 WebView2 的失败原因映射成引导页的原因。
    fn classify_reason(reason: COREWEBVIEW2_PROCESS_FAILED_REASON) -> FailureReason {
        if reason == COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY {
            FailureReason::OutOfMemory
        } else if reason == COREWEBVIEW2_PROCESS_FAILED_REASON_CRASHED {
            FailureReason::Crashed
        } else if reason == COREWEBVIEW2_PROCESS_FAILED_REASON_UNRESPONSIVE {
            FailureReason::Unresponsive
        } else if reason == COREWEBVIEW2_PROCESS_FAILED_REASON_TERMINATED {
            FailureReason::Terminated
        } else if reason == COREWEBVIEW2_PROCESS_FAILED_REASON_LAUNCH_FAILED {
            FailureReason::LaunchFailed
        } else if reason == COREWEBVIEW2_PROCESS_FAILED_REASON_PROFILE_DELETED {
            FailureReason::ProfileDeleted
        } else {
            FailureReason::Unexpected
        }
    }

    let callback = Arc::new(on_failure);
    window
        .with_webview(move |webview| {
            let controller = webview.controller();
            // SAFETY: 控制器由 WebView2 在窗口创建时提供，调用发生在这个窗口自己的回调里。
            let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
                return;
            };
            if let Ok(settings) = unsafe { core.Settings() } {
                // 关掉内置错误页：失败时不再画系统那张「此页存在问题」。
                let _ = unsafe { settings.SetIsBuiltInErrorPageEnabled(false) };
            }
            let handler = ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
                // SAFETY: 事件参数由 WebView2 在回调期间提供，下面全是只读读取。
                let detail = args.map_or(
                    FailureDetail {
                        kind: FailureKind::OtherUtility,
                        reason: FailureReason::Unknown,
                        exit_code: None,
                    },
                    |args| {
                        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND(0);
                        let _ = unsafe { args.ProcessFailedKind(&mut kind) };
                        // 死因由浏览器给出：内存不足与崩溃是两条完全不同的处置方向，
                        // 不能用宿主自己的猜测替代。
                        let mut reason = FailureReason::Unknown;
                        let mut exit_code = None;
                        if let Ok(second) = args.cast::<ICoreWebView2ProcessFailedEventArgs2>() {
                            let mut raw = COREWEBVIEW2_PROCESS_FAILED_REASON(0);
                            if unsafe { second.Reason(&mut raw) }.is_ok() {
                                reason = classify_reason(raw);
                            }
                            let mut code = 0i32;
                            if unsafe { second.ExitCode(&mut code) }.is_ok() {
                                exit_code = Some(code);
                            }
                        }
                        FailureDetail {
                            kind: classify(kind),
                            reason,
                            exit_code,
                        }
                    },
                );
                callback(detail);
                Ok(())
            }));
            let mut token = 0i64;
            // SAFETY: 处理器在窗口存活期间一直被 WebView2 持有，事件令牌无需保存。
            let _ = unsafe { core.add_ProcessFailed(&handler, &mut token) };

            let permission_handler =
                PermissionRequestedEventHandler::create(Box::new(|_sender, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut kind = COREWEBVIEW2_PERMISSION_KIND(0);
                    if unsafe { args.PermissionKind(&mut kind) }.is_err()
                        || kind != COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                    {
                        return Ok(());
                    }
                    let mut raw_uri = PWSTR::null();
                    let uri = if unsafe { args.Uri(&mut raw_uri) }.is_ok() {
                        take_pwstr(raw_uri)
                    } else {
                        String::new()
                    };
                    if is_trusted_app_origin(&uri) {
                        let _ = unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW) };
                        if let Ok(third) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>()
                        {
                            let _ = unsafe { third.SetSavesInProfile(true) };
                        }
                    }
                    Ok(())
                }));
            let mut permission_token = 0i64;
            // SAFETY: 注册窗口级麦克风权限静默放行处理器，避免弹出浏览器式权限栏。
            let _ =
                unsafe { core.add_PermissionRequested(&permission_handler, &mut permission_token) };
        })
        .is_ok()
}

/// 非 Windows 平台没有 WebView2：交回宿主的兜底判据。
#[cfg(not(windows))]
pub fn install<F>(_window: &tauri::WebviewWindow, _on_failure: F) -> bool
where
    F: Fn(FailureDetail) + Send + Sync + 'static,
{
    false
}

#[cfg(test)]
mod tests {
    use super::is_trusted_app_origin;

    #[test]
    fn trusted_origin_accepts_only_local_workbench_hosts() {
        assert!(is_trusted_app_origin("https://tauri.localhost"));
        assert!(is_trusted_app_origin("http://tauri.localhost/index.html"));
        assert!(is_trusted_app_origin("tauri://localhost"));
        assert!(is_trusted_app_origin("http://127.0.0.1:1420/"));
        assert!(is_trusted_app_origin("http://localhost:1420"));
        assert!(!is_trusted_app_origin("https://example.com"));
        assert!(!is_trusted_app_origin("http://localhost.evil.com"));
        assert!(!is_trusted_app_origin(""));
    }
}
