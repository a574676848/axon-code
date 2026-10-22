use serde::{Deserialize, Serialize};

use crate::{RemoteStateStoreIndex, RemoteStateStoreRuntime};

const TEST_NAMESPACE: &str = "remote-state-lru-test";
const TERMINAL_STATUS: &str = "delivered";
const ACTIVE_STATUS: &str = "pending";
const MAX_RECORDS: usize = 3;

#[derive(Debug, Deserialize, Serialize)]
struct State {
    id: String,
}

fn index(id: &str, status: &'static str, updated_at_ms: u64) -> RemoteStateStoreIndex {
    RemoteStateStoreIndex::new(TEST_NAMESPACE, id, status, updated_at_ms)
}

fn write(path: &std::path::Path, id: &str, status: &'static str, updated_at_ms: u64) {
    let state = State { id: id.to_owned() };
    RemoteStateStoreRuntime::upsert(path, id, index(id, status, updated_at_ms), &state)
        .expect("写入状态");
}

#[test]
fn bounded_upsert_evicts_terminal_before_oldest_active_record() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("state.sqlite3");
    // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
    let _close_pool = axon_test_support::DeferredCleanup::new({
        let path = path.clone();
        move || crate::RemoteStateStoreRuntime::close_path(&path)
    });
    write(&path, "terminal", TERMINAL_STATUS, 1);
    write(&path, "old-active", ACTIVE_STATUS, 2);
    write(&path, "recent-active", ACTIVE_STATUS, 3);

    let next = State {
        id: "fresh".to_owned(),
    };
    let report = RemoteStateStoreRuntime::upsert_with_lru_eviction(
        &path,
        &next.id,
        index(&next.id, ACTIVE_STATUS, 4),
        &next,
        MAX_RECORDS,
        &[TERMINAL_STATUS],
    )
    .expect("有界写入");

    assert_eq!(report, vec!["terminal"]);
    let states = RemoteStateStoreRuntime::read_all::<State>(&path).expect("读取状态");
    assert_eq!(states.len(), MAX_RECORDS);
    assert!(states.iter().all(|state| state.id != "terminal"));
    assert!(states.iter().any(|state| state.id == "fresh"));
}
