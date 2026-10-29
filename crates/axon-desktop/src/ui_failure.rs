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

/// 内存阈值的下限：再小的机器也不能比这更早判定。
const HIGH_WATER_FLOOR: u64 = 768 * 1024 * 1024;

/// 内存阈值的上限：再大的机器也不等它涨到失控才接管。
const HIGH_WATER_CEILING: u64 = 4096 * 1024 * 1024;

/// 读不到物理内存时采用的阈值。
const HIGH_WATER_FALLBACK: u64 = 1400 * 1024 * 1024;

/// 追溯父链的层数上限，父链异常时也不会死循环。
const ANCESTRY_LIMIT: usize = 16;

/// 故障日志所在目录（相对 Axon 根），与 CLI 日志同处一室。
const FAILURE_LOG_SUBDIR: [&str; 2] = ["data", "logs"];

/// 故障日志文件名；一行一条，只追加不覆盖。
const FAILURE_LOG_FILE: &str = "desktop-ui-failure.jsonl";

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
    // 阈值只取决于物理内存，一次算清即可；内存条不会在运行中变多。
    let high_water = memory_high_water(total_memory());
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
                let reading = webview_tree_reading();
                let judged = reading.map_or(0, MemoryReading::judged);
                let (kind, fire) = if judged >= high_water {
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
                /*
                 * 先落日志再导航：引导页一旦接管，现场就只剩这一行记录。
                 * 日志失败不影响接管，界面该回哪一页与它无关。
                 */
                record_failure(kind, reading, high_water);
                let base = window
                    .url()
                    .ok()
                    .filter(|url| url.has_host())
                    .or(last_base.clone());
                let facts = if kind == "memory-pressure" {
                    reading
                } else {
                    None
                };
                if let Some(url) = error_page_url(base.as_ref(), kind, None, facts) {
                    let _ = window.navigate(url);
                }
            }));
        }
    });
}

/// 一份界面进程树的内存读数（字节），按进程角色分类。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MemoryReading {
    /// 渲染族：正文、预览、画布与图片位图都在这里，也是唯一参与判定的那一档。
    render: u64,
    /// 浏览器宿主进程：承载窗口、合成与网络调度的那一层。
    browser: u64,
    /// 工具进程：网络、音频、存储等服务。
    utility: u64,
    /// 崩溃上报进程。
    crashpad: u64,
}

impl MemoryReading {
    /// 参与判定的字节数。
    ///
    /// 只取渲染族。浏览器宿主与各工具进程的常驻集里混着大量与渲染族共享的映像页
    /// （WebView2 的二进制、V8 快照、字体与 ICU 数据），把每进程的常驻集合直接相加
    /// 会把同一份物理页记上好几遍：实测健康界面在「全加」口径下 424 MB，其中真正由
    /// 页面内容决定的只有渲染器那 205 MB，余下的差额主要就是共享页与外壳自身。
    /// 判定因此回到渲染器这一档；分项读数仍然全部记进日志与引导页，可观测性不丢。
    fn judged(self) -> u64 {
        self.render
    }

    /// 全加口径，只用于记录与展示，不参与判定。
    fn total(self) -> u64 {
        self.render
            .saturating_add(self.browser)
            .saturating_add(self.utility)
            .saturating_add(self.crashpad)
    }
}

/// Chromium 侧的进程角色；由命令行里的 `--type=` 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessRole {
    /// 页面渲染器。
    Renderer,
    /// 浏览器宿主（没有 `--type` 的那一层）。
    Browser,
    /// 工具进程。
    Utility,
    /// GPU 进程。
    Gpu,
    /// 崩溃上报。
    Crashpad,
}

/// 从一个进程的命令行判定它的角色；没有 `--type` 的那一层就是浏览器宿主。
fn role_of(process: &Process) -> ProcessRole {
    for argument in process.cmd() {
        let text = argument.to_string_lossy();
        let Some(kind) = text.strip_prefix("--type=") else {
            continue;
        };
        return match kind {
            "renderer" => ProcessRole::Renderer,
            "gpu-process" => ProcessRole::Gpu,
            "crashpad-handler" => ProcessRole::Crashpad,
            _ => ProcessRole::Utility,
        };
    }
    ProcessRole::Browser
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
    // 进程已经退出，此刻再读进程树只会得到一份残缺读数，因此不带内存分项。
    if let Some(url) = error_page_url(base.as_ref(), kind, reason, None) {
        let _ = window.navigate(url);
    }
}

/// 界面进程树的内存读数；取不到时返回 None。
///
/// 只数自己的后代：WebView2 的浏览器进程由宿主拉起，渲染进程与工具进程是它的子进程，
/// 因此「宿主为根的后代集合」正好就是「这个界面占用的全部内存」。
fn webview_tree_reading() -> Option<MemoryReading> {
    let self_pid = sysinfo::get_current_pid().ok()?;
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let processes = system.processes();
    let mut reading = MemoryReading::default();
    for process in processes.values() {
        if !is_descendant(processes, process.pid(), self_pid) {
            continue;
        }
        let bytes = process.memory();
        match role_of(process) {
            ProcessRole::Renderer => reading.render = reading.render.saturating_add(bytes),
            ProcessRole::Browser => reading.browser = reading.browser.saturating_add(bytes),
            ProcessRole::Utility => reading.utility = reading.utility.saturating_add(bytes),
            ProcessRole::Crashpad => reading.crashpad = reading.crashpad.saturating_add(bytes),
            /*
             * GPU 进程不计入：它记的是合成器纹理的高水位，与「页面失控」不是一回事。
             * 半透窗口与背景模糊会让它随窗口面积线性变大，算进阈值会在 4K 或高缩放下
             * 把健康的界面误切成兜底页。
             */
            ProcessRole::Gpu => {}
        }
    }
    Some(reading)
}

/// 本机物理内存（字节）；读不到时为 0。
fn total_memory() -> u64 {
    let mut system = System::new();
    system.refresh_memory();
    system.total_memory()
}

/// 触发接管的内存阈值（字节）。
///
/// 固定值在两种机器上都不合适：8 GB 的机器被同样的数字放得太松，32 GB 的机器又显得
/// 太紧。阈值取物理内存的八分之一，并夹在下限与上限之间；读不到物理内存时退回固定
/// 值，判定不会因此失去上界。
fn memory_high_water(total: u64) -> u64 {
    if total == 0 {
        return HIGH_WATER_FALLBACK;
    }
    (total / 8).clamp(HIGH_WATER_FLOOR, HIGH_WATER_CEILING)
}

/// 字节换成 MiB，供查询串与日志使用。
fn mib(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

/// 把一次接管写进日志。
///
/// 这条路是事后复盘唯一的事实源：引导页只留在屏幕上，刷新即失。写盘失败不阻断接管，
/// 界面该回哪一页与日志无关。
fn record_failure(kind: &str, reading: Option<MemoryReading>, high_water: u64) {
    let Ok(root) = axon_config::ConfigLoader::default_user_axon_root_path() else {
        return;
    };
    let mut directory = root;
    for part in FAILURE_LOG_SUBDIR {
        directory.push(part);
    }
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    let record = serde_json::json!({
        "at_ms": now_ms(),
        "kind": kind,
        "high_water_mib": mib(high_water),
        "render_mib": reading.map_or(0, |value| mib(value.render)),
        "browser_mib": reading.map_or(0, |value| mib(value.browser)),
        "utility_mib": reading.map_or(0, |value| mib(value.utility)),
        "crashpad_mib": reading.map_or(0, |value| mib(value.crashpad)),
        "total_mib": reading.map_or(0, |value| mib(value.total())),
    });
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(FAILURE_LOG_FILE))
    else {
        return;
    };
    use std::io::Write as _;
    let _ = writeln!(file, "{record}");
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
///
/// `reading` 只在内存判据成立时给出：引导页把这几项原样带进「复制诊断信息」，
/// 用户把那一行贴回来就能判断是真失控还是口径问题。
fn error_page_url(
    base: Option<&Url>,
    kind: &str,
    reason: Option<&str>,
    reading: Option<MemoryReading>,
) -> Option<Url> {
    let mut url = base?.clone();
    // 没有宿主的地址（例如自定义协议的空壳）没有可换的路径。
    if !url.has_host() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    url.set_path(&format!("/{ERROR_PAGE}"));
    let mut query = format!("kind={kind}");
    if let Some(reason) = reason {
        query.push_str("&reason=");
        query.push_str(reason);
    }
    if let Some(reading) = reading {
        query.push_str(&format!(
            "&render={}&browser={}&utility={}&total={}",
            mib(reading.render),
            mib(reading.browser),
            mib(reading.utility),
            mib(reading.total()),
        ));
    }
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
            None,
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

    #[test]
    fn the_memory_reading_travels_with_the_error_page() {
        let reading = MemoryReading {
            render: 1_180 * 1024 * 1024,
            browser: 142 * 1024 * 1024,
            utility: 61 * 1024 * 1024,
            crashpad: 12 * 1024 * 1024,
        };
        let url = error_page_url(
            Some(&Url::parse("http://tauri.localhost/").expect("基址可解析")),
            "memory-pressure",
            None,
            Some(reading),
        )
        .expect("内存判据成立时必须能拼出地址");
        assert_eq!(
            url.query(),
            Some("kind=memory-pressure&render=1180&browser=142&utility=61&total=1395"),
            "分项读数要原样带进引导页"
        );
    }

    #[test]
    fn the_judged_bytes_ignore_shared_processes() {
        let reading = MemoryReading {
            render: 700,
            browser: 400,
            utility: 300,
            crashpad: 20,
        };
        assert_eq!(reading.judged(), 700, "判定只看渲染族");
        assert_eq!(reading.total(), 1420, "全加口径只用于记录");
    }

    #[test]
    fn the_high_water_follows_physical_memory_within_bounds() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(memory_high_water(4 * gib), HIGH_WATER_FLOOR, "小机器取下限");
        assert_eq!(memory_high_water(8 * gib), gib, "8 GB 取八分之一");
        assert_eq!(memory_high_water(16 * gib), 2 * gib, "16 GB 取八分之一");
        assert_eq!(
            memory_high_water(32 * gib),
            HIGH_WATER_CEILING,
            "大机器取上限"
        );
        assert_eq!(
            memory_high_water(0),
            HIGH_WATER_FALLBACK,
            "读不到物理内存时退回固定值"
        );
    }
}
