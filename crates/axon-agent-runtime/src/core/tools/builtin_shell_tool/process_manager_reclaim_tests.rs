//! 空闲回收：窗口内的会话照常运行，超窗会话被终止且只通知模型一次。

use super::super::{
    session::SHELL_IDLE_TIMEOUT,
    session_reclaim::{
        ShellSessionReclaim, ShellSessionReclaimHandler, subscribe_shell_session_reclaims,
    },
};
use super::test_support::{
    TEST_TERMINAL_RETENTION, empty_tombstones, lock, running_session, sessions_with,
};
use super::*;

/// 空闲回收测试用会话 ID；同时是通知去重键，必须与其他测试互不相同。
const RECLAIM_SESSION_ID: &str = "idle-reclaim-session";

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

/// 空闲回收：终止会话、投影为取消，并给模型投递且只投递一次通知。
#[test]
fn idle_reclaim_terminates_the_session_and_notifies_once() {
    let now = Instant::now();
    let last_touch = now - SHELL_IDLE_TIMEOUT - Duration::from_secs(1);
    let session = running_session(RECLAIM_SESSION_ID, last_touch);
    let owner_id = lock(&session).owner_id.clone();
    let recorder = Arc::new(ReclaimRecorder::default());
    let _subscription = subscribe_shell_session_reclaims(&owner_id, recorder.handler());
    let sessions = sessions_with(Arc::clone(&session));
    let tombstones = empty_tombstones();

    reap_once(&sessions, &tombstones, now, TEST_TERMINAL_RETENTION);

    {
        let session = lock(&session);
        let terminal = session
            .terminal
            .as_ref()
            .expect("超过空闲窗口的会话必须被回收为终态");
        assert_eq!(terminal.exit_code, SHELL_COMMAND_CANCELLED_EXIT_CODE);
        assert!(terminal.cancelled, "空闲回收按取消投影");
        assert!(!terminal.timed_out, "空闲回收不是墙钟超时");
    }

    let notices = recorder.notices();
    assert_eq!(notices.len(), 1, "同一会话只通知一次");
    assert_eq!(notices[0].session_id, RECLAIM_SESSION_ID);
    assert_eq!(notices[0].exit_code, SHELL_COMMAND_CANCELLED_EXIT_CODE);
    assert_eq!(notices[0].owner_id, owner_id);
    assert!(
        notices[0].render().contains("write_stdin"),
        "通知必须告诉模型用哪条通道收尾"
    );

    // 会话仍在保留窗口内，因此第二次推进既不回收也不重复通知。
    reap_once(&sessions, &tombstones, now, TEST_TERMINAL_RETENTION);
    assert_eq!(recorder.notices().len(), 1, "重复推进不得重复通知");
}

/// 仍被模型轮询的会话不受空闲回收影响。
#[test]
fn recently_touched_session_survives_the_reaper() {
    let now = Instant::now();
    let session = running_session("recently-touched-session", now - Duration::from_secs(1));
    let owner_id = lock(&session).owner_id.clone();
    let recorder = Arc::new(ReclaimRecorder::default());
    let _subscription = subscribe_shell_session_reclaims(&owner_id, recorder.handler());
    let sessions = sessions_with(Arc::clone(&session));

    reap_once(&sessions, &empty_tombstones(), now, TEST_TERMINAL_RETENTION);

    assert!(lock(&session).terminal.is_none(), "窗口内的会话不得被回收");
    assert!(recorder.notices().is_empty(), "未回收就不得通知");
}
