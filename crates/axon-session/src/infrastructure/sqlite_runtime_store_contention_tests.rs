//! 运行态写的并发争用用例。
//!
//! 这一组的存在理由只有一条：四条写路径不变量（空集合不取写锁、写事务不跨行、
//! 分配序号与落盘同锁、整库独占维护不落在活库）里，前三条**只能靠争用才能证伪**。
//! 静态审计能确认锁序与前置条件，确认不了「真跑起来会不会自死锁、会不会撞上
//! `database is locked`」。这里让多个写者与维护者在同一会话上抢同一把单写者锁，
//! 把这两件事变成可执行断言。

use super::SqliteRuntimeStore;
use crate::infrastructure::sqlite_runtime_store_interaction_fixtures::interaction_in_session;
use crate::ports::EventLogStore as _;
use crate::{InteractionStore, RuntimeStore};
use axon_domain::{DaemonSessionId, RuntimeHostId};

/// 并发争用下不得出现 `database is locked`，事件面序号也不得出现洞或乱序。
///
/// 价值在两处，都是静态审计拿不到的：其一，维护路径**真的结算一条**（租约过期的
/// Interaction），于是它会与帧写者抢同一会话的写锁并真的分配序号，任何一处
/// 「先开事务、后取会话锁」的自死锁都会让本用例挂住或超时；其二，维护调用在空集合与
/// 非空集合之间反复进出，任何一个 `SQLITE_BUSY` 都会让 `expect` 直接失败。
#[test]
fn concurrent_frame_writers_keep_the_event_tail_contiguous() {
    const WRITERS: usize = 4;
    const BATCHES: usize = 40;
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    store.initialize().expect("初始化");
    let host = store
        .claim_host(
            &RuntimeHostId::new("contention-host").expect("host"),
            1,
            None,
            10,
            600_000,
        )
        .expect("host lease");
    let session_id = DaemonSessionId::new("session-contention").expect("session id");
    store
        .create_session(&session_id, ".", 10)
        .expect("建立会话");
    // 种一条租约过期的在跑轮次：维护线程会在争用中把它收口并追加证据帧。
    let interaction = interaction_in_session(
        &session_id,
        "interaction-contention",
        "workspace-test",
        "terminal-test",
        10,
    );
    store.append(&interaction).expect("追加 Interaction");
    store
        .claim_processing_by_id(&interaction.interaction_id, 20, 100)
        .expect("领取")
        .expect("lease");

    std::thread::scope(|scope| {
        for _ in 0..WRITERS {
            let store = store.clone();
            let host = &host;
            let session_id = &session_id;
            scope.spawn(move || {
                for _ in 0..BATCHES {
                    store
                        .append_events(
                            session_id,
                            host,
                            &[("checkpoint".to_owned(), "{}".to_owned())],
                            20,
                        )
                        .expect("并发追加不得撞上 database is locked");
                }
            });
        }
        let store = store.clone();
        let host = &host;
        scope.spawn(move || {
            for _ in 0..BATCHES {
                // 租约在 120 到期，这里用 200 判定过期：首轮真正结算，其余为空集合。
                store
                    .quarantine_interrupted_interactions(host, 200, 8)
                    .expect("收口不得撞上 database is locked");
                store
                    .settle_recovering_sessions(host, 200, 60_000, 8)
                    .expect("空集合收敛");
                store
                    .claim_due_schedules(host, 200, 1_000, 8)
                    .expect("空集合领取");
            }
        });
    });

    let records = store.replay_events(&session_id, 0, 10_000).expect("回放");
    let sequences: Vec<u64> = records.iter().map(|record| record.sequence).collect();
    assert_eq!(
        sequences.len(),
        WRITERS * BATCHES + 1,
        "并发追加的次数加上一条中断证据帧，必须一条不少"
    );
    for (index, sequence) in sequences.iter().enumerate() {
        assert_eq!(
            *sequence,
            index as u64 + 1,
            "序号必须从 1 连续升序：出现洞或乱序就说明分配序号与落盘分属两把锁"
        );
    }
}
