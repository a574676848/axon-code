use serde::{Deserialize, Serialize};

use super::{RemoteStateStoreIndex, RemoteStateStoreRuntime};

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct State {
    value: String,
}

fn index(value: &State, updated_at_ms: u64) -> RemoteStateStoreIndex {
    RemoteStateStoreIndex::new("test", value.value.clone(), "current", updated_at_ms)
}

#[test]
fn stale_revision_cannot_replace_current_state_or_create_history() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("state.sqlite3");
    // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
    let _close_pool = axon_test_support::DeferredCleanup::new({
        let path = path.clone();
        move || crate::RemoteStateStoreRuntime::close_path(&path)
    });

    RemoteStateStoreRuntime::upsert(
        &path,
        "dispatch-1",
        index(
            &State {
                value: "current".to_owned(),
            },
            2,
        ),
        &State {
            value: "current".to_owned(),
        },
    )
    .expect("write current state");
    RemoteStateStoreRuntime::upsert(
        &path,
        "dispatch-1",
        index(
            &State {
                value: "stale".to_owned(),
            },
            1,
        ),
        &State {
            value: "stale".to_owned(),
        },
    )
    .expect("ignore stale state");

    assert_eq!(
        RemoteStateStoreRuntime::read_all::<State>(&path).expect("read current state"),
        vec![State {
            value: "current".to_owned(),
        }]
    );
}

#[test]
fn indexed_reads_and_retention_target_only_matching_current_states() {
    const NAMESPACE: &str = "test";
    const ACTIVE: &str = "active";
    const TERMINAL: &str = "terminal";
    const PARTITION_A: &str = "partition-a";
    const PARTITION_B: &str = "partition-b";
    const RETENTION_CUTOFF_MS: u64 = 100;

    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("state.sqlite3");
    // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
    let _close_pool = axon_test_support::DeferredCleanup::new({
        let path = path.clone();
        move || crate::RemoteStateStoreRuntime::close_path(&path)
    });
    let records = [
        ("old-terminal", PARTITION_A, TERMINAL, 50, "old"),
        ("recent-terminal", PARTITION_A, TERMINAL, 150, "recent"),
        ("active", PARTITION_A, ACTIVE, 50, "active"),
        ("other-partition", PARTITION_B, TERMINAL, 50, "other"),
    ];
    for (key, partition, status, updated_at_ms, value) in records {
        let state = State {
            value: value.to_owned(),
        };
        RemoteStateStoreRuntime::upsert(
            &path,
            key,
            RemoteStateStoreIndex::new(NAMESPACE, partition, status, updated_at_ms),
            &state,
        )
        .expect("write state");
    }

    assert_eq!(
        RemoteStateStoreRuntime::read_indexed::<State>(
            &path,
            NAMESPACE,
            Some(PARTITION_A),
            &[TERMINAL],
        )
        .expect("read indexed"),
        vec![
            State {
                value: "old".to_owned(),
            },
            State {
                value: "recent".to_owned(),
            },
        ]
    );
    assert_eq!(
        RemoteStateStoreRuntime::delete_indexed_before(
            &path,
            NAMESPACE,
            &[TERMINAL],
            RETENTION_CUTOFF_MS,
        )
        .expect("delete terminal states"),
        2
    );
    assert_eq!(
        RemoteStateStoreRuntime::read_all::<State>(&path).expect("read surviving states"),
        vec![
            State {
                value: "active".to_owned(),
            },
            State {
                value: "recent".to_owned(),
            },
        ]
    );
}

#[test]
fn delete_keys_removes_current_state_and_index() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("state.sqlite3");
    // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
    let _close_pool = axon_test_support::DeferredCleanup::new({
        let path = path.clone();
        move || crate::RemoteStateStoreRuntime::close_path(&path)
    });
    let state = State {
        value: "current".to_owned(),
    };
    RemoteStateStoreRuntime::upsert(&path, "dispatch-1", index(&state, 1), &state)
        .expect("write state");

    assert_eq!(
        RemoteStateStoreRuntime::delete_keys(&path, &["dispatch-1".to_owned()])
            .expect("delete state"),
        1
    );
    assert!(
        RemoteStateStoreRuntime::read::<State>(&path, "dispatch-1")
            .expect("read state")
            .is_none()
    );
    assert!(
        RemoteStateStoreRuntime::read_indexed::<State>(&path, "test", None, &["current"])
            .expect("read index")
            .is_empty()
    );
}

#[test]
fn key_operations_cross_bounded_batch_boundary() {
    const RECORD_COUNT: usize = 257;
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("state.sqlite3");
    // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
    let _close_pool = axon_test_support::DeferredCleanup::new({
        let path = path.clone();
        move || crate::RemoteStateStoreRuntime::close_path(&path)
    });
    let mut keys = Vec::with_capacity(RECORD_COUNT);
    let mut partitions = Vec::with_capacity(RECORD_COUNT);
    for index in 0..RECORD_COUNT {
        let key = format!("dispatch-{index:03}");
        let partition = format!("session-{index:03}");
        let state = State {
            value: partition.clone(),
        };
        RemoteStateStoreRuntime::upsert(
            &path,
            &key,
            RemoteStateStoreIndex::new("test", partition.clone(), "current", 1),
            &state,
        )
        .expect("write batched state");
        keys.push(key);
        partitions.push(partition);
    }

    assert_eq!(
        RemoteStateStoreRuntime::count_keys(&path, &keys).expect("count batched keys"),
        RECORD_COUNT
    );
    assert_eq!(
        RemoteStateStoreRuntime::keys_for_indexed_partitions(&path, "test", &partitions)
            .expect("query batched partitions"),
        keys
    );
    assert_eq!(
        RemoteStateStoreRuntime::delete_keys(&path, &keys).expect("delete batched keys"),
        RECORD_COUNT
    );
    assert_eq!(
        RemoteStateStoreRuntime::count_keys(&path, &keys).expect("count deleted keys"),
        0
    );
}

/// 目标态下状态库由进程内连接池长期持有文件句柄，运行期外部替换库文件不再受支持，
/// 替换只在进程重启后的首次建连生效。此处锁定重启归一化契约：空库文件在首次使用
/// 时就地重建 schema，而不是让守护进程持续失败。
#[test]
fn empty_database_file_recreates_remote_state_schema_on_first_use() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("state.sqlite3");
    // 池化连接会锁住库文件：临时目录析构前必须先关闭连接池（守卫按声明逆序析构）。
    let _close_pool = axon_test_support::DeferredCleanup::new({
        let path = path.clone();
        move || crate::RemoteStateStoreRuntime::close_path(&path)
    });
    let state = State {
        value: "current".to_owned(),
    };
    std::fs::File::create(&path).expect("创建空数据库文件");

    RemoteStateStoreRuntime::upsert(&path, "dispatch-1", index(&state, 1), &state)
        .expect("空库文件应自动重建 schema");
    assert_eq!(
        RemoteStateStoreRuntime::read_indexed::<State>(&path, "test", None, &["current"])
            .expect("数据库重建后应可读取"),
        vec![state]
    );
}
