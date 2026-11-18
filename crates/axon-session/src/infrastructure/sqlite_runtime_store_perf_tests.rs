//! 会话身份相关的性能基线：单次入站的主键查找代价。
//!
//! 存储架构 §10.1 的红线是「会话校验为一次主键查找，上限 1ms，禁止每帧查库」。
//! 入站带 `session_id` 之后，网关每次都要确认会话存在，因此这条读必须是主键索引
//! 上的单次点查；一旦退化成扫描或二次查询，事件吞吐立刻被它压住。
//!
//! 用例按 `#[ignore]` 归档：它要先建 500 条会话再跑 1000 次查找，属于性能验收
//! 而不是日常回归。门禁脚本用 `--ignored inbound_session_lookup --nocapture` 调用。

use std::sync::atomic::{AtomicU64, Ordering};

use axon_domain::DaemonSessionId;

use super::SqliteRuntimeStore;
use crate::ports::RuntimeStore as _;

/// 夹具目录序号：同一测试线程内多个用例共用一个临时根，路径必须逐用例区分。
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const SESSIONS: usize = 500;
const LOOKUPS: usize = 1_000;

#[test]
#[ignore = "性能验收：会话存在性校验必须是主键单次点查，p95 上限 1ms"]
fn inbound_session_lookup_is_a_single_primary_key_read() {
    let directory = axon_test_support::test_temp_path(&format!(
        "inbound-session-lookup-{}",
        FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化运行态库");

    let session_ids = (0..SESSIONS)
        .map(|index| {
            let session_id = DaemonSessionId::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F{index:02}"))
                .expect("合法会话 ID");
            store
                .create_session(&session_id, "/workspace", 1)
                .expect("登记会话");
            session_id
        })
        .collect::<Vec<_>>();

    // 先热一遍连接池与页缓存，再取样：冷启动的第一次读不代表稳态代价。
    for _ in 0..10 {
        for session_id in &session_ids {
            store.get_session(session_id).expect("读取会话");
        }
    }

    let mut samples = Vec::with_capacity(LOOKUPS);
    for index in 0..LOOKUPS {
        let session_id = &session_ids[index % SESSIONS];
        let started = std::time::Instant::now();
        let record = store.get_session(session_id).expect("读取会话");
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(record.session_id, *session_id, "必须命中同一条会话");
    }
    samples.sort_by(|left, right| left.partial_cmp(right).expect("毫秒读数可比"));

    let position = samples.len() * 95 / 100;
    let p95_ms = samples[position.min(samples.len() - 1)];
    println!(
        "inbound_session_lookup_p95_ms={p95_ms:.4} sessions={SESSIONS} lookups={LOOKUPS} 门槛=1ms"
    );
    assert!(
        p95_ms <= 1.0,
        "会话存在性校验超出 1ms 上限：p95={p95_ms:.4}ms"
    );
}
