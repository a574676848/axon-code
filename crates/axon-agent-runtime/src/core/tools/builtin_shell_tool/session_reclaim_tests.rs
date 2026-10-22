//! 会话回收通知的渲染、去重与投递语义。

use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use super::*;

/// 每个测试用独立 owner，避免进程内共享总线在并行测试间串台。
static OWNER_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn unique_owner(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        OWNER_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Default)]
struct ReclaimRecorder {
    notices: Mutex<Vec<ShellSessionReclaim>>,
}

impl ReclaimRecorder {
    fn handler(self: &Arc<Self>) -> ShellSessionReclaimHandler {
        let recorder = Arc::clone(self);
        Arc::new(move |notice| {
            recorder
                .notices
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(notice);
        })
    }

    fn notices(&self) -> Vec<ShellSessionReclaim> {
        self.notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn notice(session_id: &str, owner: &str) -> ShellSessionReclaim {
    ShellSessionReclaim::new(
        session_id,
        owner,
        "cargo test --workspace",
        "E:\\zbg\\axon-code",
        130,
        Duration::from_secs(600),
        "C:\\axon\\data\\tee\\session.shell.log",
    )
}

#[test]
fn render_carries_session_reference_terminal_and_next_actions() {
    let owner = unique_owner("render");
    let rendered = notice("session-render", &owner).render();

    for expected in [
        "session-render",
        "cargo test --workspace",
        "退出码：130",
        "10 分钟",
        "C:\\axon\\data\\tee\\session.shell.log",
        "write_stdin",
        "run_in_background",
    ] {
        assert!(
            rendered.contains(expected),
            "通知正文缺少 {expected}：{rendered}"
        );
    }
    assert!(
        !rendered.contains("job_id"),
        "会话回收通知不得把 session_id 说成 job_id：{rendered}"
    );
}

#[test]
fn render_truncates_body_but_keeps_session_reference_and_terminal() {
    let owner = unique_owner("truncate");
    let long_command = "x".repeat(SHELL_RECLAIM_NOTICE_COMMAND_MAX_CHARS * 4);
    let notice = ShellSessionReclaim::new(
        "session-truncate",
        &owner,
        &long_command,
        &long_command,
        130,
        Duration::from_secs(90),
        "C:\\axon\\session.shell.log",
    );

    let rendered = notice.render();

    assert!(
        rendered.chars().count() <= SHELL_RECLAIM_NOTICE_MAX_CHARS,
        "正文必须有界"
    );
    assert!(rendered.contains("session-truncate"), "截断不得丢会话引用");
    assert!(rendered.contains("退出码：130"), "截断不得丢终态");
    assert!(
        rendered.contains('…'),
        "超限正文必须留下截断标记：{rendered}"
    );
    assert!(rendered.contains("1 分钟"), "不足一分钟按秒、其余按分钟");
}

#[test]
fn command_and_cwd_are_bounded_independently() {
    let owner = unique_owner("bounded");
    let long_command = "y".repeat(SHELL_RECLAIM_NOTICE_COMMAND_MAX_CHARS + 50);
    let notice = ShellSessionReclaim::new(
        "session-bounded",
        &owner,
        &long_command,
        "z",
        130,
        Duration::from_secs(600),
        "C:\\axon\\session.shell.log",
    );

    assert_eq!(
        notice.command.chars().count(),
        SHELL_RECLAIM_NOTICE_COMMAND_MAX_CHARS
    );
    assert!(notice.command.ends_with('…'), "有界命令以省略号收尾");
    assert_eq!(notice.cwd, "z");
}

#[test]
fn publishing_the_same_session_twice_delivers_once() {
    let owner = unique_owner("dedup");
    let recorder = Arc::new(ReclaimRecorder::default());
    let _subscription = subscribe_shell_session_reclaims(&owner, recorder.handler());

    publish_shell_session_reclaim(notice("session-dedup", &owner));
    publish_shell_session_reclaim(notice("session-dedup", &owner));

    assert_eq!(
        recorder.notices().len(),
        1,
        "同一会话的重复回收必须被结构性去重"
    );
}

#[test]
fn notice_published_without_a_subscriber_is_flushed_on_subscribe() {
    let owner = unique_owner("pending");
    publish_shell_session_reclaim(notice("session-pending", &owner));

    let recorder = Arc::new(ReclaimRecorder::default());
    let _subscription = subscribe_shell_session_reclaims(&owner, recorder.handler());

    let delivered = recorder.notices();
    assert_eq!(delivered.len(), 1, "订阅时必须冲刷已暂存的通知");
    assert_eq!(delivered[0].session_id, "session-pending");
}

#[test]
fn pending_queue_keeps_the_newest_notices_per_owner() {
    let owner = unique_owner("bounded-pending");
    let total = SHELL_RECLAIM_NOTICE_MAX_PENDING_PER_OWNER + 3;
    for index in 0..total {
        publish_shell_session_reclaim(notice(&format!("session-{index}"), &owner));
    }

    let recorder = Arc::new(ReclaimRecorder::default());
    let _subscription = subscribe_shell_session_reclaims(&owner, recorder.handler());

    let delivered = recorder.notices();
    assert_eq!(
        delivered.len(),
        SHELL_RECLAIM_NOTICE_MAX_PENDING_PER_OWNER,
        "暂存队列必须有上界"
    );
    assert_eq!(
        delivered.last().map(|notice| notice.session_id.as_str()),
        Some(format!("session-{}", total - 1).as_str()),
        "淘汰应丢最旧的、保留最新的"
    );
}

#[test]
fn delivery_targets_the_owning_run_only() {
    let owner = unique_owner("owner-a");
    let other = unique_owner("owner-b");
    let recorder = Arc::new(ReclaimRecorder::default());
    let other_recorder = Arc::new(ReclaimRecorder::default());
    let _subscription = subscribe_shell_session_reclaims(&owner, recorder.handler());
    let _other_subscription = subscribe_shell_session_reclaims(&other, other_recorder.handler());

    publish_shell_session_reclaim(notice("session-owner-a", &owner));

    assert_eq!(recorder.notices().len(), 1);
    assert!(
        other_recorder.notices().is_empty(),
        "通知不得越权投递给另一个 owner"
    );
}
