//! 后台完成通知收件箱：把 Goal 与定时任务的完成结论变成系统通知。
//!
//! 桌面端同一时刻只订阅当前会话的实时流，切到别的会话或收进托盘之后，其它会话
//! 里的后台结果不会再产生任何提示。完成通知在 Daemon 侧是持久收件箱，本模块
//! 周期拉取未读结论、交给系统通知，再回执送达；拉取失败只留日志，下一轮照常重试。

use std::collections::HashSet;
use std::time::Duration;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// 轮询间隔：完成结论是分钟级可容忍的后台事实，20 秒一次既不积压也不打扰。
const POLL_INTERVAL: Duration = Duration::from_secs(20);
/// 单次拉取条数：收件箱按有界批次消费，剩余的下一轮继续。
const POLL_LIMIT: u32 = 10;
/// 桌面端渠道标识，与 Daemon 侧登记的口径一致。
const DESKTOP_CHANNEL: &str = "desktop";

/// 启动后台收件箱线程。
///
/// 线程只做「拉取 → 提示 → 回执」三步，不持有任何界面状态：界面换了、窗口关了，
/// 它都不受影响。已提示过的通知记在本线程内，回执失败时下一轮也不会重复打扰。
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let mut notified = HashSet::new();
        loop {
            std::thread::sleep(POLL_INTERVAL);
            if let Err(error) = poll_once(&app, &mut notified) {
                eprintln!("Axon 完成通知收件箱拉取失败：{error}");
            }
        }
    });
}

fn poll_once(app: &AppHandle, notified: &mut HashSet<String>) -> Result<(), String> {
    let path = format!("/notifications?channel={DESKTOP_CHANNEL}&limit={POLL_LIMIT}");
    let response = crate::gateway::get_json(&path)?;
    let items = response
        .get("notifications")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut entries = Vec::new();
    for item in items {
        let notification_id = item
            .get("notificationId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let destination = item
            .get("destination")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if notification_id.is_empty() {
            continue;
        }
        if notified.insert(notification_id.to_owned()) {
            let content = item
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            notify(app, content);
        }
        entries.push(serde_json::json!({
            "notificationId": notification_id,
            "destination": destination,
        }));
    }
    if entries.is_empty() {
        return Ok(());
    }
    crate::gateway::post_json(
        "/notifications/ack",
        &serde_json::json!({ "channel": DESKTOP_CHANNEL, "entries": entries }),
    )?;
    Ok(())
}

/// 发一条系统通知。
///
/// 通知开关与正文压平都复用桌面外壳的既有口径：后台结论与轮次结论在用户眼里
/// 是同一类提醒，长度与可读性不该有两套标准。
fn notify(app: &AppHandle, content: &str) {
    if !crate::desktop_shell::notifications_enabled() {
        return;
    }
    let body = crate::desktop_shell::compact_body(content);
    let body = if body.is_empty() {
        "打开 Axon 查看结果".to_owned()
    } else {
        body
    };
    if let Err(error) = app
        .notification()
        .builder()
        .title("后台任务已完成")
        .body(body)
        .show()
    {
        eprintln!("Axon 系统通知投递失败：{error}");
    }
}
