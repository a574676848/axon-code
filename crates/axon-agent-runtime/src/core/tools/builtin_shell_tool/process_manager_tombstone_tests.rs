//! 会话墓碑回收：超期会话移出注册表，并留下可与「从未存在」区分的原因。

use super::test_support::{
    TEST_TERMINAL_RETENTION, empty_tombstones, lock, sessions_with, terminal_session,
};
use super::*;

/// 测试用会话 ID。
const REAPED_SESSION_ID: &str = "reaped-session";
/// 一次测试中伪造的墓碑数量，超出容量上界以覆盖淘汰。
const OVERFLOW_TOMBSTONE_COUNT: usize = SHELL_TOMBSTONE_CAPACITY + 10;

#[test]
fn reaping_a_terminal_session_records_a_tombstone() {
    let now = Instant::now();
    let ended_at = now - TEST_TERMINAL_RETENTION - Duration::from_secs(1);
    let sessions = sessions_with(terminal_session(REAPED_SESSION_ID, ended_at));
    let tombstones = empty_tombstones();

    reap_once(&sessions, &tombstones, now, TEST_TERMINAL_RETENTION);

    assert!(
        !lock(&sessions).contains_key(REAPED_SESSION_ID),
        "超出保留窗口的会话应已从注册表移除"
    );
    let tombstones = lock(&tombstones);
    assert_eq!(
        session_unreachable_reason(&tombstones, REAPED_SESSION_ID),
        SHELL_SESSION_EXPIRED_ERROR,
        "已回收会话必须报告「已回收」，与「从未存在」区分开"
    );
    assert_eq!(
        session_unreachable_reason(&tombstones, "never-existed"),
        SHELL_SESSION_NOT_FOUND_ERROR
    );
}

#[test]
fn session_within_retention_is_not_reaped() {
    let now = Instant::now();
    let ended_at = now - Duration::from_secs(1);
    let sessions = sessions_with(terminal_session(REAPED_SESSION_ID, ended_at));
    let tombstones = empty_tombstones();

    reap_once(&sessions, &tombstones, now, TEST_TERMINAL_RETENTION);

    assert!(
        lock(&sessions).contains_key(REAPED_SESSION_ID),
        "保留窗口内仍可读终态输出的会话不得被回收"
    );
    assert!(lock(&tombstones).is_empty());
}

#[test]
fn tombstones_expire_after_their_own_retention() {
    let now = Instant::now();
    let mut tombstones = VecDeque::from([SessionTombstone {
        session_id: REAPED_SESSION_ID.to_owned(),
        retired_at: now - SHELL_TOMBSTONE_RETENTION - Duration::from_secs(1),
    }]);

    prune_tombstones_locked(&mut tombstones, now);

    assert!(tombstones.is_empty(), "超出墓碑保留时长的记录应被清理");
}

#[test]
fn tombstones_evict_oldest_records_beyond_capacity() {
    let now = Instant::now();
    let tombstones = empty_tombstones();
    let expired = (0..OVERFLOW_TOMBSTONE_COUNT)
        .map(|index| format!("session-{index}"))
        .collect::<Vec<_>>();

    record_tombstones(&tombstones, expired, now);

    let tombstones = lock(&tombstones);
    assert_eq!(
        tombstones.len(),
        SHELL_TOMBSTONE_CAPACITY,
        "墓碑必须有容量上界，避免长期运行下无限增长"
    );
    assert!(
        !tombstones
            .iter()
            .any(|tombstone| tombstone.session_id == "session-0"),
        "超出容量后应淘汰最旧记录"
    );
    assert!(
        tombstones
            .iter()
            .any(|tombstone| tombstone.session_id
                == format!("session-{}", OVERFLOW_TOMBSTONE_COUNT - 1)),
        "最新记录应保留"
    );
}
