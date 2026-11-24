//! doctor 存储不变式检查的测试：违规必须点名，前提缺失必须如实说「无法检查」。

use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore as _, SqliteRuntimeStore};

use super::{InvariantProbe, invariant_checks_for};
use crate::constants::{DOCTOR_CHECK_INV2, DOCTOR_CHECK_INV7A, DOCTOR_CHECK_INV7B};
use crate::interactive_tui::DoctorStatus;

/// 判定用的「现在」：用例自己定死，不随系统时间漂移。
const NOW_MS: i64 = 1_000_000;
/// 300s 过期缓冲以内（与实现同一常量口径），用来构造「刚进入 recovering」的会话。
const RECENT_ACTIVITY_MS: i64 = 999_000;
/// 远超 300s 缓冲的最后活动时刻。
const STALE_ACTIVITY_MS: i64 = 100_000;

/// 一个隔离的运行态库。
fn store() -> (axon_test_support::TestTempDir, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path());
    store.initialize().expect("初始化运行态库");
    (directory, store)
}

/// 登记一条会话并把它写成指定的运行态。
fn session(store: &SqliteRuntimeStore, session_id: &str, status: &str, last_activity_at_ms: i64) {
    let id = DaemonSessionId::new(session_id.to_owned()).expect("合法会话标识");
    store
        .create_session(&id, "/workspace", 10)
        .expect("登记会话");
    store
        .with_read_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET status=?2, last_activity_at_ms=?3 WHERE session_id=?1",
                rusqlite::params![session_id, status, last_activity_at_ms],
            )
        })
        .expect("写入会话状态");
}

/// 登记一条工作项。
fn work_item(store: &SqliteRuntimeStore, execution_id: &str, session_id: &str, status: &str) {
    store
        .with_read_connection(|connection| {
            connection.execute(
                "INSERT INTO work_items(execution_id,session_id,attempt,priority,status,\
                     created_at_ms,updated_at_ms) VALUES(?1,?2,0,0,?3,10,10)",
                rusqlite::params![execution_id, session_id, status],
            )
        })
        .expect("写入工作项");
}

/// 按名称取一条检查结论。
fn check<'a>(
    checks: &'a [crate::interactive_tui::DoctorCheck],
    name: &str,
) -> &'a crate::interactive_tui::DoctorCheck {
    checks
        .iter()
        .find(|check| check.name == name)
        .unwrap_or_else(|| panic!("缺少检查项 {name}"))
}

#[test]
fn doctor_reports_inv2_and_inv7_violations() {
    let (_directory, store) = store();
    // 停在 recovering 且名下没有任何在途工作项：INV-7a 违规，最近活动仍在缓冲内。
    session(
        &store,
        "session-inv7a-violation",
        "recovering",
        RECENT_ACTIVITY_MS,
    );
    // 停在 recovering 超过缓冲：INV-7b 违规；名下有在途工作项，因此不算 INV-7a。
    session(
        &store,
        "session-inv7b-violation",
        "recovering",
        STALE_ACTIVITY_MS,
    );
    work_item(
        &store,
        "execution-inv7b-running",
        "session-inv7b-violation",
        "running",
    );
    // 干净的 recovering 会话：有在途工作项且刚刚活动过，两条 7 系列都不该命中它。
    session(&store, "session-clean", "recovering", RECENT_ACTIVITY_MS);
    work_item(&store, "execution-clean-queued", "session-clean", "queued");
    // 结论状态却没有 terminal_kind：INV-2 违规。
    work_item(
        &store,
        "execution-inv2-violation",
        "session-inv7a-violation",
        "completed",
    );

    let checks = invariant_checks_for(InvariantProbe::Ready {
        store,
        now_ms: NOW_MS,
    });

    let inv2 = check(&checks, DOCTOR_CHECK_INV2);
    assert_eq!(inv2.status, DoctorStatus::Failed);
    assert!(
        inv2.message.contains("execution-inv2-violation"),
        "INV-2 必须点名违规工作项：{}",
        inv2.message
    );
    assert!(
        inv2.message.contains("session-inv7a-violation"),
        "INV-2 必须点名归属会话：{}",
        inv2.message
    );
    assert!(inv2.message.contains("违规 1 行"), "{}", inv2.message);

    let inv7a = check(&checks, DOCTOR_CHECK_INV7A);
    assert_eq!(inv7a.status, DoctorStatus::Failed);
    assert!(
        inv7a.message.contains("session-inv7a-violation"),
        "INV-7a 必须点名违规会话：{}",
        inv7a.message
    );
    assert!(
        !inv7a.message.contains("session-inv7b-violation"),
        "有在途工作项的会话不算 INV-7a：{}",
        inv7a.message
    );
    assert!(
        !inv7a.message.contains("session-clean"),
        "干净会话不得被点名：{}",
        inv7a.message
    );

    let inv7b = check(&checks, DOCTOR_CHECK_INV7B);
    assert_eq!(inv7b.status, DoctorStatus::Failed);
    assert!(
        inv7b.message.contains("session-inv7b-violation"),
        "INV-7b 必须点名过期会话：{}",
        inv7b.message
    );
    assert!(
        !inv7b.message.contains("session-clean"),
        "缓冲内的会话不得被判过期：{}",
        inv7b.message
    );
    assert!(
        inv7b.message.contains("axon daemon restart"),
        "违规必须给出可执行建议：{}",
        inv7b.message
    );
}

#[test]
fn doctor_marks_checks_unverifiable_without_daemon() {
    let reason = "Daemon 未运行或 IPC 不可达（连接 Daemon IPC 失败：端点不存在）";
    let checks = invariant_checks_for(InvariantProbe::Unavailable(reason.to_owned()));

    assert_eq!(checks.len(), 3, "三条不变式都要给出结论");
    for (name, check) in [
        (DOCTOR_CHECK_INV2, &checks[0]),
        (DOCTOR_CHECK_INV7A, &checks[1]),
        (DOCTOR_CHECK_INV7B, &checks[2]),
    ] {
        assert_eq!(check.name, name);
        assert_eq!(
            check.status,
            DoctorStatus::Unverifiable,
            "缺少 Daemon 时不得报零行通过：{}",
            check.message
        );
        assert!(
            check.message.contains("无法检查"),
            "必须如实标注无法检查：{}",
            check.message
        );
        assert!(
            check.message.contains(reason),
            "必须带上具体前提缺失原因：{}",
            check.message
        );
    }
    // 无法检查既不是通过也不是失败：整体报告不该因此变红。
    assert!(
        checks
            .iter()
            .all(|check| check.status != DoctorStatus::Failed)
    );
    // 线上形态是 `unverifiable`，与 `ok` / `failed` 并列，客户端不必猜。
    let serialized = serde_json::to_value(&checks[0]).expect("检查项必须可序列化");
    assert_eq!(serialized["status"], "unverifiable");
}
