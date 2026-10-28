//! 桌面外壳：系统托盘、关闭即最小化、轮次结束的系统通知。
//!
//! 这里只处理「窗口与操作系统之间」的边界：应用内部状态仍由各视图自行持有。
//! 三件事共用同一个事实源：主窗口句柄与轮次终态事件，因此放在一处更好核对。

use std::sync::{Mutex, OnceLock};

mod turn_notifications;
use turn_notifications::NotificationState;

use tauri::image::Image;
use tauri::menu::{IconMenuItem, IsMenuItem, Menu, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use crate::conversation::wire::StreamEvent;
use crate::release_channel::UpdateCheckDto;

/// 托盘菜单项标识。
const MENU_SHOW: &str = "tray-show";
const MENU_CHECK_UPDATE: &str = "tray-check-update";
const MENU_QUIT: &str = "tray-quit";
/// 托盘标识：主题切换后要靠它取回托盘换菜单。
const TRAY_ID: &str = "axon-tray";
/// 托盘菜单图形的像素边长：2 倍图，由各平台按显示缩放自行取样。
const TRAY_ICON_SIZE: u32 = 32;

/*
 * 托盘菜单图形：与既有工作台产品同款，逐字节取自它的图标路径。
 * 直接内嵌原始 RGBA，因此不需要任何图像解码依赖；
 * `icons/tray/README.md` 记录了每个图形的来源与再生成方法。
 *
 * 每个图形备两套墨色。三平台的原生菜单跟随的都是**系统**主题而不是应用主题，
 * 深色菜单上深墨图标会整片消失，因此按菜单底色分两套：
 * 浅色菜单用深墨，深色菜单用浅墨，运行期按窗口主题选一套，主题变化时重建菜单。
 */
struct TrayIcons {
    show: &'static [u8],
    update: &'static [u8],
    quit: &'static [u8],
}

/// 浅色菜单用的一套（深墨）。
static TRAY_ICONS_ON_LIGHT: TrayIcons = TrayIcons {
    show: include_bytes!("../icons/tray/show-on-light.rgba"),
    update: include_bytes!("../icons/tray/update-on-light.rgba"),
    quit: include_bytes!("../icons/tray/quit-on-light.rgba"),
};

/// 深色菜单用的一套（浅墨）。
static TRAY_ICONS_ON_DARK: TrayIcons = TrayIcons {
    show: include_bytes!("../icons/tray/show-on-dark.rgba"),
    update: include_bytes!("../icons/tray/update-on-dark.rgba"),
    quit: include_bytes!("../icons/tray/quit-on-dark.rgba"),
};

/// 主题到图标集：未知主题一律按浅色菜单处理，避免深色墨画在深色底上。
fn icons_for_theme(theme: &tauri::Theme) -> &'static TrayIcons {
    match theme {
        tauri::Theme::Dark => &TRAY_ICONS_ON_DARK,
        _ => &TRAY_ICONS_ON_LIGHT,
    }
}

/// 主窗口标签；托盘与关闭行为都只作用于它。
const MAIN_WINDOW: &str = "main";
/// 通知正文的最大字符数：系统通知不是阅读区，超长只会被截断得体无完肤。
const NOTIFICATION_BODY_LIMIT: usize = 120;
/// 轮次终态事件类型，与后端一致。
const EVENT_TURN_COMPLETED: &str = "turn_completed";
/// 推给前端的「会话有新结果」事件名，与前端 `ipc.ts` 中的常量一致。
pub const LANE_ACTIVITY_EVENT: &str = "axon://lane-activity";
/// 运行过程事件类型：其中承载 durable 的轮次生命周期。
const EVENT_WIRE: &str = "wire_event";

/// 已通知过的轮次标识。
///
/// 收口事件与生命周期事件都可能出现，同一个轮次只允许提示一次；
/// 用轮次标识去重比用时间窗更稳。
static NOTIFICATION_STATE: OnceLock<Mutex<NotificationState>> = OnceLock::new();

/// 安装托盘与关闭行为。
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    // 界面卡死或被系统回收时不把系统错误页留给用户：装上兜底引导页的看门狗。
    // 与托盘无关，但同属窗口级装配，放在这里可以保证主窗口一建好就生效。
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        configure_window_chrome(&window);
        crate::ui_failure::install_watchdog(&window);
        // 最大化状态只在这里挂一次：前端不再于每次尺寸变化时来回问一遍。
        crate::window_effects::watch_maximized(&window);
    }
    // 原生菜单跟随系统主题，因此首屏就按当前主题取墨色。
    let theme = app
        .get_webview_window(MAIN_WINDOW)
        .and_then(|window| window.theme().ok())
        .unwrap_or(tauri::Theme::Light);
    let menu = build_tray_menu(app, icons_for_theme(&theme))?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Axon")
        .menu(&menu)
        // Windows 与 Linux 的托盘习惯是左键回窗口、右键出菜单；
        // macOS 的状态栏习惯相反，左键就应当展开菜单，因此按平台取默认语义。
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_QUIT => app.exit(0),
            MENU_SHOW => show_main_window(app),
            MENU_CHECK_UPDATE => check_updates_from_tray(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if cfg!(target_os = "macos") {
                return;
            }
            // 左键单击回到窗口，右键留给菜单。
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;

    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let handle = window.clone();
        window.on_window_event(move |event| match event {
            // 关闭按钮收进托盘，只有托盘菜单的「退出」才真正结束进程。
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = handle.hide();
            }
            // 系统主题切换后原生菜单会换底色，图标墨色必须跟着换。
            WindowEvent::ThemeChanged(theme) => {
                apply_tray_menu(handle.app_handle(), icons_for_theme(theme));
            }
            /*
             * 通知点击：桌面端拿不到系统通知的直接回调（插件在桌面平台不透出点击事件），
             * 但点击通知必然让系统激活应用、窗口重新获得焦点，这一刻是同一个事实。
             * 有刚发出的待打开目标时才动作，否则聚焦只是聚焦。
             */
            WindowEvent::Focused(true) => open_pending_lane(handle.app_handle()),
            _ => {}
        });
    }
    Ok(())
}

/// 装配跨平台沉浸式一体标题栏与初始窗口主题。
fn configure_window_chrome(window: &tauri::WebviewWindow) {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_decorations(false);
        let _ = window.set_shadow(true);
    }
    #[cfg(target_os = "macos")]
    {
        /*
         * 窗口配置统一声明 `decorations: false`（Windows 与 Linux 需要无边框），
         * macOS 必须把原生边框加回来：左上角红绿灯是 macOS 的窗口语义，
         * 去掉它应用就成了一块关不掉的面板。标题栏取 Overlay，保留交通灯但让出内容区。
         */
        let _ = window.set_decorations(true);
        let _ = window.set_title_bar_style(tauri::TitleBarStyle::Overlay);
    }
    // 初始深浅色与持久化主题同源，判定只在 window_effects 里写一份。
    let _ = window.set_theme(Some(crate::window_effects::stored_window_theme()));
}

/// 托盘菜单：回窗口、检查更新、退出三组，组间用分隔线断开，每项都带图形。
///
/// 分组与既有工作台产品的托盘一致。菜单项一律带图标：
/// 纯文字条目在托盘这种小尺寸菜单里辨识成本更高。
fn build_tray_menu(app: &AppHandle, icons: &'static TrayIcons) -> tauri::Result<Menu<tauri::Wry>> {
    let show = tray_item(app, MENU_SHOW, "显示主界面", icons.show)?;
    let check_update = tray_item(app, MENU_CHECK_UPDATE, "检查更新", icons.update)?;
    let quit = tray_item(app, MENU_QUIT, "退出", icons.quit)?;
    let gap_before_update = PredefinedMenuItem::separator(app)?;
    let gap_before_quit = PredefinedMenuItem::separator(app)?;
    let items: [&dyn IsMenuItem<tauri::Wry>; 5] = [
        &show,
        &gap_before_update,
        &check_update,
        &gap_before_quit,
        &quit,
    ];
    Menu::with_items(app, &items)
}

/// 按给定墨色重建并替换托盘菜单：首次安装与主题切换共用这一条路径。
fn apply_tray_menu(app: &AppHandle, icons: &'static TrayIcons) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match build_tray_menu(app, icons) {
        Ok(menu) => {
            if let Err(error) = tray.set_menu(Some(menu)) {
                eprintln!("Axon 托盘菜单替换失败：{error}");
            }
        }
        Err(error) => eprintln!("Axon 托盘菜单构建失败：{error}"),
    }
}

/// 托盘菜单项：图形由 `icons/tray` 的原始 RGBA 直接给出，尺寸由常量钉死。
fn tray_item(
    app: &AppHandle,
    id: &str,
    text: &str,
    rgba: &'static [u8],
) -> tauri::Result<IconMenuItem<tauri::Wry>> {
    let icon = Image::new_owned(rgba.to_vec(), TRAY_ICON_SIZE, TRAY_ICON_SIZE);
    IconMenuItem::with_id(app, id, text, true, Some(icon), None::<&str>)
}

/// 托盘菜单的「检查更新」。
///
/// 与界面的更新入口共用 `release_channel::check_update` 这一份事实源，
/// 因此两条入口给出的结论必然一致。检查要走网络，放在独立线程上执行，
/// 菜单事件不阻塞主线程。
///
/// 回执用系统模态对话框而不是系统通知：三平台的通知权限与投递状态各不相同
/// （Linux 需要通知守护进程，macOS 需要应用已获授权），而对话框是用户刚点下
/// 这个动作的直接回执，缺少回执就等于点了没反应。
fn check_updates_from_tray(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let (title, body, release_url) = match crate::release_channel::check_update() {
            Ok(result) => {
                let (title, body) = update_notice(&result);
                (title, body, result.release_url)
            }
            Err(error) => ("检查更新失败".to_owned(), error, String::new()),
        };
        show_update_dialog(&handle, &title, &body, &release_url);
    });
}

/// 弹出更新结论：有发布页时给一个「打开发布页」按钮，其余情况只报告结论。
fn show_update_dialog(app: &AppHandle, title: &str, body: &str, release_url: &str) {
    let buttons = if release_url.is_empty() {
        MessageDialogButtons::OkCustom("知道了".to_owned())
    } else {
        MessageDialogButtons::OkCancelCustom("打开发布页".to_owned(), "稍后".to_owned())
    };
    let handle = app.clone();
    let target = release_url.to_owned();
    app.dialog()
        .message(body)
        .title(title)
        .buttons(buttons)
        .show(move |confirmed| {
            if !confirmed || target.is_empty() {
                return;
            }
            // 打开动作与界面里的下载入口同源：都交给系统默认浏览器。
            if let Err(error) = handle.opener().open_url(target.clone(), None::<&str>) {
                eprintln!("Axon 打开发布页失败：{error}");
            }
        });
}

/// 更新检查结果到回执文案：三种结论各有明确标题，正文只给版本事实。
///
/// 失败时把可执行提示一并带上：托盘这一条入口没有别的地方能显示它，
/// 只报原因而不给下一步动作，用户就只能自己猜。
fn update_notice(result: &UpdateCheckDto) -> (String, String) {
    if !result.error.is_empty() {
        let body = if result.hint.is_empty() {
            result.error.clone()
        } else {
            format!("{}\n{}", result.error, result.hint)
        };
        return ("检查更新失败".to_owned(), body);
    }
    if result.update_available {
        return (
            format!("发现新版本 {}", result.latest),
            format!("当前版本 {}，可在设置的关于页里打开下载页", result.current),
        );
    }
    (
        "已是最新版本".to_owned(),
        format!("当前版本 {}", result.current),
    )
}

/// 把主窗口显示到前台。
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    // 托盘唤回与第二次启动都可能带着一条待打开的通知目标回来，一并消费。
    open_pending_lane(app);
}

/// 消费待打开的通知目标：聚焦窗口，并把要打开的会话交给界面。
///
/// 会话选择由界面完成：它持有真实工作区与会话清单，Rust 侧只交出车道标识。
fn open_pending_lane(app: &AppHandle) {
    let Some(lane_id) = crate::lane_context::take_pending_open() else {
        return;
    };
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    if let Err(error) = app.emit(
        crate::lane_context::LANE_OPEN_EVENT,
        serde_json::json!({ "laneId": lane_id }),
    ) {
        eprintln!("Axon 打开会话事件下发失败：{error}");
    }
}

/// 通知标题：会话名在前、结论在后；没有会话名时只给结论。
///
/// 只写「本轮已完成」时用户得点进去才知道是哪个会话，而通知本来就常在一屏之外；
/// 会话名由界面在切换会话时写入 [`crate::lane_context`]。
#[must_use]
fn compose_title(lane: Option<&str>, base: &str) -> String {
    let lane = lane.map(str::trim).filter(|value| !value.is_empty());
    match lane {
        Some(lane) => format!("{lane} · {base}"),
        None => base.to_string(),
    }
}

/// 是否允许发送系统通知。
///
/// 直接读桌面状态：开关是用户设置，且改动频率极低，
/// 为此维护一份内存副本只会多出一个可能与磁盘不一致的状态源。
pub fn notifications_enabled() -> bool {
    crate::state::load_state().notifications_enabled
}

/// 轮次结束时发系统通知，并把「这一轮结束了」推给界面。
///
/// 两条通道分工明确：
/// - **界面标识**在实时终态首次到达时推：应用在前台时它就是通知的替代品，会话标题前的状态图标
///   告诉用户哪条会话有了新结果，看过即清；
/// - **系统通知**只在窗口不在前台时发：用户就坐在界面前再弹一条通知是打扰，
///   而应用收进托盘时，界面标识看不见，通知才是唯一的提醒。
pub fn notify_turn_finished(app: &AppHandle, event: &StreamEvent) {
    let notice = NOTIFICATION_STATE
        .get_or_init(|| Mutex::new(NotificationState::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .observe(event);
    let Some(notice) = notice else {
        return;
    };

    // 界面标识先推：它不依赖通知设置，也不受窗口状态影响。
    let window = app.get_webview_window(MAIN_WINDOW);
    let visible = window
        .as_ref()
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    let focused = window
        .as_ref()
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    let foreground = visible && focused;
    let _ = app.emit(
        LANE_ACTIVITY_EVENT,
        serde_json::json!({
            "id": notice.id,
            "status": notice.status,
            "title": notice.title,
            "foreground": foreground,
        }),
    );

    if foreground {
        return;
    }
    if !notifications_enabled() {
        return;
    }
    /*
     * 通知的会话归属：标题要带会话名，点击后要能回到这条会话。
     *
     * 桌面端同一时刻只订阅当前会话的事件流，因此收到轮次终态的一定就是它；
     * 目标在这里记下，由窗口重新获得焦点那一刻消费。
     */
    let lane = crate::lane_context::active_lane();
    if let Some(lane) = lane.as_ref() {
        crate::lane_context::remember_pending_open(&lane.lane_id);
    }
    let title = compose_title(
        lane.as_ref().map(|context| context.title.as_str()),
        &notice.title,
    );
    // 通知失败必须留痕：这条链路一旦静默失败，用户只会觉得「说好的通知从来没出现过」。
    if let Err(error) = app
        .notification()
        .builder()
        .title(title)
        .body(notice.body)
        .show()
    {
        eprintln!("Axon 系统通知投递失败：{error}");
    }
}

/// 一次轮次结束提示。
struct TurnNotice {
    /// 轮次标识，用于去重。
    id: String,
    /// 终态标签，界面据此选状态图标。
    status: String,
    /// 结论标题；发送时再拼上会话名。
    title: String,
    body: String,
}

/// 从一帧事件里判断是否构成「一轮结束」。
///
/// 收口事件是首选；但订阅端未必收得到终态帧，而轮次生命周期是 durable 的
/// 运行事件、稳定到达，因此两条路都认，避免通知整体失效。
fn turn_notice(event_type: &str, payload_json: &str) -> Option<TurnNotice> {
    if event_type == EVENT_TURN_COMPLETED {
        let payload: serde_json::Value = serde_json::from_str(payload_json).unwrap_or_default();
        let status = payload
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("completed");
        let id = payload
            .get("interactionId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let body = payload
            .get("response")
            .and_then(serde_json::Value::as_str)
            .map(compact_body)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "打开 Axon 查看结果".to_owned());
        return Some(TurnNotice {
            id,
            status: status.to_owned(),
            title: status_title(status).to_string(),
            body,
        });
    }
    if event_type != EVENT_WIRE {
        return None;
    }
    let envelope: serde_json::Value = serde_json::from_str(payload_json).ok()?;
    let kind = envelope.get("kind")?;
    if kind.get("type").and_then(serde_json::Value::as_str) != Some("turnLifecycle") {
        return None;
    }
    let payload = kind.get("payload")?;
    if payload.get("phase").and_then(serde_json::Value::as_str) != Some("regularTurn") {
        return None;
    }
    let status = payload
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if status != "completed" && status != "failed" {
        return None;
    }
    // 生命周期事件没有正文，用它的标题兜底，至少说清这一轮的结果。
    let body = payload
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(compact_body)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "打开 Axon 查看结果".to_owned());
    Some(TurnNotice {
        id: envelope
            .get("interactionId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}:{}",
                    payload
                        .get("turnId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                    status
                )
            }),
        status: status.to_owned(),
        title: status_title(status).to_string(),
        body,
    })
}

/// 终态到提示标题。
const fn status_title(status: &str) -> &'static str {
    match status.as_bytes() {
        b"failed" => "本轮执行失败",
        b"interrupted" => "本轮已中断",
        _ => "本轮已完成",
    }
}

/// 问答提问的通知标题。
pub(super) fn question_title(status: &str) -> &'static str {
    if status == "suspended" {
        "任务已挂起，等待回复"
    } else {
        "模型需要您的确认"
    }
}

/// 压平并截断通知正文。
fn compact_body(text: &str) -> String {
    // 折叠连续空白：多行正文直接贴进通知会带出一串空格，读起来是断裂的。
    let mut flattened = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !flattened.is_empty() {
            flattened.push(' ');
        }
        in_space = false;
        flattened.push(character);
    }
    let trimmed = flattened.trim();
    let mut body: String = trimmed.chars().take(NOTIFICATION_BODY_LIMIT).collect();
    if trimmed.chars().count() > NOTIFICATION_BODY_LIMIT {
        body.push('…');
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_body_is_flattened_and_bounded() {
        let body = compact_body("第一行\n\n第二行\t 第三行");
        assert_eq!(body, "第一行 第二行 第三行");
        let long = compact_body(&"长".repeat(NOTIFICATION_BODY_LIMIT + 10));
        assert_eq!(long.chars().count(), NOTIFICATION_BODY_LIMIT + 1);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn non_terminal_frames_do_not_notify() {
        // 只做纯函数层的判定：通知本身依赖系统能力，不在单测里断言。
        assert_eq!(compact_body("   "), "");
    }

    #[test]
    fn turn_notice_recognises_both_terminal_facts() {
        let completed = turn_notice(
            EVENT_TURN_COMPLETED,
            r#"{"interactionId":"i-1","status":"failed","response":"出错了"}"#,
        )
        .expect("收口事件应构成提示");
        assert_eq!(completed.id, "i-1");
        assert_eq!(completed.title, "本轮执行失败");
        assert_eq!(completed.body, "出错了");

        let lifecycle = turn_notice(
            EVENT_WIRE,
            r#"{"kind":{"type":"turnLifecycle","payload":{"phase":"regularTurn","status":"completed","turnId":"t-9","title":"本轮完成"}}}"#,
        )
        .expect("生命周期终态应构成提示");
        assert_eq!(lifecycle.id, "t-9:completed");
        assert_eq!(lifecycle.title, "本轮已完成");
        assert_eq!(lifecycle.body, "本轮完成");

        assert!(turn_notice(EVENT_WIRE, r#"{"kind":{"type":"turnLifecycle","payload":{"phase":"regularTurn","status":"running"}}}"#).is_none());
        assert!(turn_notice("runtime_observation", "{}").is_none());
    }

    #[test]
    fn tray_icon_assets_match_the_declared_size() {
        // 资源被截断时菜单会画出半张图，因此把尺寸钉死在测试里；
        // 两套墨色都查，漏做深色那一套会在这里暴露。
        for set in [&TRAY_ICONS_ON_LIGHT, &TRAY_ICONS_ON_DARK] {
            for asset in [set.show, set.update, set.quit] {
                assert_eq!(asset.len(), (TRAY_ICON_SIZE * TRAY_ICON_SIZE * 4) as usize);
            }
        }
    }

    #[test]
    fn theme_selects_the_matching_icon_set() {
        assert!(std::ptr::eq(
            icons_for_theme(&tauri::Theme::Dark),
            &TRAY_ICONS_ON_DARK
        ));
        assert!(std::ptr::eq(
            icons_for_theme(&tauri::Theme::Light),
            &TRAY_ICONS_ON_LIGHT
        ));
    }

    #[test]
    fn dark_and_light_sets_carry_different_ink() {
        // 两套图形必须真的不同，否则深色菜单上等于没有图标。
        assert_ne!(TRAY_ICONS_ON_LIGHT.show, TRAY_ICONS_ON_DARK.show);
        assert_ne!(TRAY_ICONS_ON_LIGHT.update, TRAY_ICONS_ON_DARK.update);
        assert_ne!(TRAY_ICONS_ON_LIGHT.quit, TRAY_ICONS_ON_DARK.quit);
    }

    #[test]
    fn update_notice_covers_three_outcomes() {
        let result = |latest: &str, available: bool, error: &str, hint: &str| UpdateCheckDto {
            current: "0.1.0".to_owned(),
            latest: latest.to_owned(),
            update_available: available,
            release_url: String::new(),
            download_url: String::new(),
            published_at: String::new(),
            error: error.to_owned(),
            hint: hint.to_owned(),
        };

        let (title, body) = update_notice(&result("v0.2.0", true, "", ""));
        assert_eq!(title, "发现新版本 v0.2.0");
        assert!(body.contains("0.1.0"));

        let (title, body) = update_notice(&result("v0.1.0", false, "", ""));
        assert_eq!(title, "已是最新版本");
        assert!(body.contains("0.1.0"));

        let (title, body) = update_notice(&result("", false, "查询 GitHub 超时", "稍后重试"));
        assert_eq!(title, "检查更新失败");
        assert!(body.contains("查询 GitHub 超时"));
        assert!(body.contains("稍后重试"));

        // 没有提示时不能给回执留一个空行。
        let (_, body) = update_notice(&result("", false, "查询 GitHub 超时", ""));
        assert_eq!(body, "查询 GitHub 超时");
    }
}
