//! 界面卡死或被系统回收时的兜底引导。
//!
//! 界面每隔几秒会经一次 IPC 与宿主报到（网关探活命令）。宿主记下最后一次报到时间：
//! 窗口可见且未最小化时，若连续若干个心跳周期都没有报到，说明渲染进程已经卡死或
//! 被系统回收（WebView2 此时会自己画一张「此页存在问题」的错误页），于是把界面切到
//! 随包发布的错误引导页。
//!
//! 为什么不用 WebView2 的进程失败事件：那条路要在桌面端调原生 COM 接口并写 `unsafe`，
//! 而仓库对桌面端是 `unsafe_code = "forbid"`。心跳判定不需要 unsafe，同时覆盖
//! 「进程已退出」与「进程无响应」两种表现，三个平台共用同一套逻辑。
//!
//! 切换只发生一次：切走之后时钟归零，等界面重新报到（也就是真的活了）才会再次判定，
//! 因此不会形成「切过去又切回来」的循环。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sysinfo::{Pid, Process, ProcessesToUpdate, System};
use url::Url;

/// 错误引导页的文件名；随前端产物一起发布在应用根。
const ERROR_PAGE: &str = "error.html";

/// 看门狗的检查间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// 判定界面失去响应的静默时长。
///
/// 心跳周期是 5 秒；探活请求自身最长可达约 9 秒（预算加一次重试），期间仍会记录
/// 报到，因此实际间隔不会超过约 10 秒。20 秒留出两倍余量，避免把「网关忙」误判成
/// 「界面死了」。
const UI_UNRESPONSIVE_MS: u64 = 20_000;

/// 切换引导页后的冷却时长。
///
/// 引导页自己不会报到，所以切换之后不能一直保持武装（那会在引导页上反复导航），
/// 但也不能就此永久休眠：界面可能被用户重新加载后再次崩掉，那时仍要有人接管。
/// 冷却给出的是「同一现场只切一次，新现场照样接管」。
const FIRE_COOLDOWN_MS: u64 = 30_000;

/// 界面进程树的内存高水位（字节）。
///
/// 这一条与心跳互补：心跳看得见「界面不响应」，看不见「界面还在响应、内存却被吃光」。
/// 实测那条会话的渲染进程 6 秒内从 98MB 涨到 1174MB，而页面自身的 JS 堆只有 9MB，
/// 心跳全程正常；等浏览器自己画出错误页时已经晚了。水位取在系统回收渲染进程之前，
/// 引导页因此能赶在系统错误页之前出现。
const WEBVIEW_MEMORY_HIGH_WATER: u64 = 1_400 * 1024 * 1024;

/// 追溯父链的层数上限，父链异常时也不会死循环。
const ANCESTRY_LIMIT: usize = 16;

/// 最后一次界面报到时间（毫秒）；0 表示尚未报到或刚刚切换过引导页。
static LAST_SEEN_MS: AtomicU64 = AtomicU64::new(0);

/// 记录一次界面报到；由界面会周期性调用的 IPC 命令调用。
pub fn note_ui_alive() {
    LAST_SEEN_MS.store(now_ms(), Ordering::Release);
}

/// 安装看门狗；主窗口建好后调用一次。
pub fn install_watchdog(window: &tauri::WebviewWindow) {
    note_ui_alive();
    /*
     * 首选直接接管：关掉 WebView2 的内置错误页，并订阅进程失败事件。进程一死就精确
     * 回调，切引导页既不必等心跳静默，也不会先闪过系统那张「此页存在问题」。
     *
     * 两种判据并行而不是二选一：进程失败事件负责「死了立刻切」，下面那只看门狗负责
     * 「趁还活着（内存正在被吃光）先切」，后者能把卡死的那几秒也省掉。
     */
    let hooked = window.clone();
    let _ = axon_webview2_failure::install(window, move |detail| {
        // 死因由浏览器给出，直接带进引导页：内存不足与崩溃的处置方向完全不同。
        redirect_to_failure_page(&hooked, detail.kind.slug(), Some(detail.reason.slug()));
    });
    // 下面那条线程要长期持有窗口句柄，因此在这里取一份自有副本。
    let window = window.clone();
    std::thread::spawn(move || {
        /*
         * 地址必须每拍现记。
         *
         * 安装这一刻 WebView 还没有导航到应用地址，此刻取到的不是应用地址；而界面真的
         * 崩了之后又取不到。因此每拍把「最近一次取到的有效应用地址」记下来，切换时优先
         * 现取、取不到再用它兜底。
         */
        let mut last_base: Option<Url> = None;
        let mut cooldown_until = 0u64;
        loop {
            std::thread::sleep(CHECK_INTERVAL);
            /*
             * 每一拍都兜住 panic：宿主接口在界面已经崩掉时会返回错误、也可能直接
             * panic，而看门狗线程一旦死掉，这一整轮会话就再没有人接管了。
             */
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let now = now_ms();
                if now < cooldown_until {
                    return;
                }
                let watched = is_being_watched(&window);
                if let Ok(current) = window.url()
                    && current.has_host()
                {
                    remember_base(&current);
                    last_base = Some(current);
                }
                let last = LAST_SEEN_MS.load(Ordering::Acquire);
                if !watched || last == 0 {
                    return;
                }
                /*
                 * 两条判据，谁先成立谁接管：
                 * - 内存高水位：界面还在响应，但内存正在被吃光，此时不切就只剩系统错误页；
                 * - 心跳静默：界面已经卡死或已被系统回收。
                 */
                let memory = webview_tree_memory();
                let (kind, fire) = if memory.is_some_and(|bytes| bytes >= WEBVIEW_MEMORY_HIGH_WATER)
                {
                    ("memory-pressure", true)
                } else {
                    (
                        "render-unresponsive",
                        now.saturating_sub(last) >= UI_UNRESPONSIVE_MS,
                    )
                };
                if !fire {
                    return;
                }
                // 切换前归零并进入冷却：同一现场只切一次。
                LAST_SEEN_MS.store(0, Ordering::Release);
                cooldown_until = now.saturating_add(FIRE_COOLDOWN_MS);
                let base = window
                    .url()
                    .ok()
                    .filter(|url| url.has_host())
                    .or(last_base.clone());
                if let Some(url) = error_page_url(base.as_ref(), kind, None) {
                    let _ = window.navigate(url);
                }
            }));
        }
    });
}

/// 最近一次取到的有效应用地址。
///
/// 进程失败事件到达时页面已经没了，`window.url()` 未必还给得出地址，因此看门狗每一拍
/// 都把取到的地址记在这里，接管时用它兜底。
static BASE_URL: OnceLock<Mutex<Option<Url>>> = OnceLock::new();

/// 记下当前应用地址。
fn remember_base(url: &Url) {
    let slot = BASE_URL.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(url.clone());
    }
}

/// 取最近一次记下的应用地址。
fn known_base() -> Option<Url> {
    BASE_URL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()?
        .clone()
}

/// 切到引导页。
///
/// 地址优先现取，取不到用最近一次记下的：进程失败事件到达时页面已经不可用，
/// 单靠现取会拿不到地址，引导页也就切不过去。
fn redirect_to_failure_page(window: &tauri::WebviewWindow, kind: &str, reason: Option<&str>) {
    let base = window
        .url()
        .ok()
        .filter(|url| url.has_host())
        .inspect(remember_base)
        .or_else(known_base);
    if let Some(url) = error_page_url(base.as_ref(), kind, reason) {
        let _ = window.navigate(url);
    }
}

/// 界面进程树的内存总量（字节）；取不到时返回 None。
/// 只数自己的后代：WebView2 的浏览器进程由宿主拉起，渲染进程与 GPU 进程是它的子进程，
/// 因此「宿主为根的后代集合」正好就是「这个界面占用的全部内存」。
fn webview_tree_memory() -> Option<u64> {
    let self_pid = sysinfo::get_current_pid().ok()?;
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let processes = system.processes();
    let mut total = 0u64;
    for process in processes.values() {
        if !is_descendant(processes, process.pid(), self_pid) {
            continue;
        }
        total = total.saturating_add(process.memory());
    }
    Some(total)
}

/// 这个进程是不是给定祖先的后代（自身也算）。
fn is_descendant(processes: &HashMap<Pid, Process>, pid: Pid, ancestor: Pid) -> bool {
    let mut current = pid;
    for _ in 0..ANCESTRY_LIMIT {
        if current == ancestor {
            return true;
        }
        match processes.get(&current).and_then(Process::parent) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    false
}

/// 兜底引导页的地址。
///
/// 只换路径，保留应用自己的源：真机上应用地址可能是带尾斜杠的目录、不带尾斜杠的
/// 源，甚至自定义协议。实测 `window.url()` 返回的是后者，`Url::join` 对它会直接
/// 失败，引导页因此从未被切过去；按源重设路径两种形态都成立。
fn error_page_url(base: Option<&Url>, kind: &str, reason: Option<&str>) -> Option<Url> {
    let mut url = base?.clone();
    // 没有宿主的地址（例如自定义协议的空壳）没有可换的路径。
    if !url.has_host() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    url.set_path(&format!("/{ERROR_PAGE}"));
    let query = match reason {
        Some(reason) => format!("kind={kind}&reason={reason}"),
        None => format!("kind={kind}"),
    };
    url.set_query(Some(&query));
    Some(url)
}

/// 窗口是否正被用户看着。
///
/// 隐藏或最小化时不做判定：浏览器会主动节流后台页面的定时器，心跳自然变慢，
/// 那不代表界面坏了；而且这时用户也看不到错误页。
fn is_being_watched(window: &tauri::WebviewWindow) -> bool {
    window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
}

/// 当前时间（毫秒）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_report_rearms_the_watchdog() {
        LAST_SEEN_MS.store(0, Ordering::Release);
        note_ui_alive();
        let seen = LAST_SEEN_MS.load(Ordering::Acquire);
        assert!(seen > 0, "报到必须留下时间戳");
        assert!(
            now_ms().saturating_sub(seen) < UI_UNRESPONSIVE_MS,
            "刚刚报到过的界面不得被判为失去响应"
        );
    }

    #[test]
    fn the_error_page_keeps_the_application_origin() {
        // 真机上的两种形态：带尾斜杠的目录与不带尾斜杠的源。
        for base in ["http://tauri.localhost/", "http://tauri.localhost"] {
            let url = error_page_url(
                Some(&Url::parse(base).expect("基址可解析")),
                "render-unresponsive",
                None,
            )
            .expect("两种形态都要能拼出引导页地址");
            assert_eq!(url.path(), "/error.html", "基址 {base}");
            assert_eq!(url.host_str(), Some("tauri.localhost"), "基址 {base}");
            assert_eq!(url.query(), Some("kind=render-unresponsive"), "基址 {base}");
        }
    }

    #[test]
    fn a_custom_scheme_origin_is_kept_as_is() {
        let url = error_page_url(
            Some(&Url::parse("tauri://localhost").expect("自定义协议可解析")),
            "render-exited",
            Some("out-of-memory"),
        )
        .expect("有宿主就必须能拼出引导页地址");
        assert_eq!(url.scheme(), "tauri");
        assert_eq!(url.path(), "/error.html");
        assert_eq!(
            url.query(),
            Some("kind=render-exited&reason=out-of-memory"),
            "死因要带进引导页"
        );
    }
}
