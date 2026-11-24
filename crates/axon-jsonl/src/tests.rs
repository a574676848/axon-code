use super::*;
use std::{fs, io, io::Write as _};

thread_local! {
    /// 下一次把临时文件改名到这些目标时各失败几次。
    static FAILED_RENAMES: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, u32>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// 让下一次把临时文件改名到 `target` 时失败一次（模拟目标被占用、杀软扫描）。
fn fail_next_rename_to(target: &std::path::Path) {
    FAILED_RENAMES.with(|failed| {
        *failed.borrow_mut().entry(target.to_path_buf()).or_insert(0) += 1;
    });
}

/// 取走一次注入的改名失败；没有注入时返回 false。
pub(crate) fn take_rename_failure(target: &std::path::Path) -> bool {
    FAILED_RENAMES.with(|failed| {
        let mut failed = failed.borrow_mut();
        match failed.get_mut(target) {
            Some(remaining) if *remaining > 0 => {
                *remaining -= 1;
                true
            }
            _ => false,
        }
    })
}

#[test]
fn rename_failure_falls_back_to_backup_without_leaving_a_gap() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("rename-fallback.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    store
        .append_line(&logical, br#"{"key":"value"}"#)
        .expect("append");
    store.append_line(&logical, b"x").expect("append");
    assert_eq!(store.shard_sequences(&logical).expect("sequences"), [0, 1]);

    // 目标分片改名失败一次：替换必须退回「先挪到 .bak 再补位」，不得先删目标。
    fail_next_rename_to(&ShardedJsonlStore::physical_shard_path(&logical, 0));
    store
        .rewrite_with(&logical, |_| {
            Ok(((), vec![br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()]))
        })
        .expect("回退补位后重写必须成功");

    assert_eq!(
        store.shard_sequences(&logical).expect("sequences"),
        [0, 1],
        "回退路径不得让分片出现缺口"
    );
    assert_eq!(
        store.read_lines(&logical).expect("lines"),
        [br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()]
    );
    assert!(
        !ShardedJsonlStore::physical_shard_path(&logical, 0)
            .with_extension("bak")
            .exists(),
        "补位成功后旧分片的 .bak 必须清掉"
    );
}

#[test]
fn failed_backup_promotion_keeps_backup_and_reads_as_incomplete() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("rename-broken.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    store
        .append_line(&logical, br#"{"key":"value"}"#)
        .expect("append");
    store.append_line(&logical, b"x").expect("append");
    let old_first_shard = fs::read(ShardedJsonlStore::physical_shard_path(&logical, 0)).unwrap();

    // 两次改名都失败：回退也补不上位。旧内容必须留在 .bak，错误向上报。
    fail_next_rename_to(&ShardedJsonlStore::physical_shard_path(&logical, 0));
    fail_next_rename_to(&ShardedJsonlStore::physical_shard_path(&logical, 0));
    let error = store
        .rewrite_with(&logical, |_| {
            Ok(((), vec![br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()]))
        })
        .expect_err("补位失败必须上抛");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");

    let backup = ShardedJsonlStore::physical_shard_path(&logical, 0).with_extension("bak");
    assert!(backup.exists(), "补位失败时旧分片必须保留在 .bak");
    assert_eq!(
        fs::read(&backup).unwrap(),
        old_first_shard,
        "旧内容不得丢失"
    );

    // 读侧据「分片序号不连续」判不完整：绝不把缺块的历史当成完整历史。
    assert_eq!(store.shard_sequences(&logical).expect("sequences"), [1]);
    let read_error = store.read_lines(&logical).expect_err("缺口必须读成错误");
    assert_eq!(read_error.kind(), io::ErrorKind::InvalidData);
    assert!(read_error.to_string().contains("不连续"), "{read_error}");
}

#[test]
fn concurrent_readers_never_observe_a_torn_rewrite() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("concurrent.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    let first = vec![br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()];
    let second = vec![br#"{"id":3}"#.to_vec(), br#"{"id":4}"#.to_vec()];
    // 先把库放在第一份完整内容上：此后读者只允许读到 first 或 second。
    store
        .rewrite_with(&logical, |_| Ok(((), first.clone())))
        .expect("首次重写");

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let logical = logical.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let store = ShardedJsonlStore::with_max_shard_bytes(16);
            let mut toggle = false;
            while !stop.load(Ordering::Relaxed) {
                let target = if toggle { &first } else { &second };
                store
                    .rewrite_with(&logical, |_| Ok(((), target.clone())))
                    .expect("并发重写必须成功");
                toggle = !toggle;
                std::thread::yield_now();
            }
        })
    };

    let reader = {
        let logical = logical.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let store = ShardedJsonlStore::with_max_shard_bytes(16);
            let mut observed = 0_usize;
            while !stop.load(Ordering::Relaxed) {
                let lines = store.read_lines(&logical).expect("读者不得读到撕裂的重写");
                assert!(
                    lines == [br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()]
                        || lines == [br#"{"id":3}"#.to_vec(), br#"{"id":4}"#.to_vec()],
                    "读者必须读到完整的一段，实测 {lines:?}"
                );
                observed += 1;
            }
            observed
        })
    };

    std::thread::sleep(std::time::Duration::from_millis(60));
    stop.store(true, Ordering::Relaxed);
    writer.join().expect("writer");
    let observed = reader.join().expect("reader");
    assert!(observed > 0, "读者必须真的跑过一轮读");
}

#[test]
fn recover_unfinished_rewrite_clears_crash_debris_inside_the_lock() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("recover.jsonl");
    let store = ShardedJsonlStore::new();
    let stale = logical.with_extension("rewrite0.tmp");
    fs::write(&stale, b"{\"id\":9}").expect("写临时文件");

    store
        .recover_unfinished_rewrite(&logical)
        .expect("自愈必须清掉残留临时文件");

    assert!(!stale.exists(), "残留临时文件必须被清掉");
    assert!(!store.has_stale_rewrite_temporaries(&logical).expect("探测"));
}

#[test]
fn append_rolls_and_reads_shards_by_sequence() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("events.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);

    store.append_line(&logical, br#"{"id":1}"#).expect("append");
    store.append_line(&logical, br#"{"id":2}"#).expect("append");
    store
        .append_line(&logical, br#"{"id-long":3}"#)
        .expect("append");

    let paths = store.shard_paths(&logical).expect("shard paths");
    assert_eq!(paths.len(), 3);
    assert!(paths[0].ends_with("events.000000.jsonl"));
    assert!(paths[1].ends_with("events.000001.jsonl"));
    assert!(paths[2].ends_with("events.000002.jsonl"));
    let lines = store.read_lines(&logical).expect("lines");
    assert_eq!(
        lines,
        [
            b"{\"id\":1}".to_vec(),
            b"{\"id\":2}".to_vec(),
            b"{\"id-long\":3}".to_vec()
        ]
    );
}

#[test]
fn append_retains_only_records_at_or_after_cutoff() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("retained.jsonl");
    let store = ShardedJsonlStore::new();
    let timestamp = |line: &[u8]| {
        let value: serde_json::Value = serde_json::from_slice(line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        value
            .get("recordedAtMs")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "缺少时间戳"))
    };

    store
        .append_retained_line(&logical, br#"{"recordedAtMs":10}"#, 100, 50, timestamp)
        .expect("old append");
    store
        .append_retained_line(&logical, br#"{"recordedAtMs":90}"#, 100, 50, timestamp)
        .expect("new append");

    let lines = store.read_lines(&logical).expect("lines");
    assert_eq!(lines, [b"{\"recordedAtMs\":90}".to_vec()]);
}

/// 重写不得先删目标再改名：替换必须一步到位，且崩溃留下的临时文件在下次重写前清掉。
#[test]
fn rewrite_replaces_shards_atomically_and_drops_stale_temporaries() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("atomic.jsonl");
    let store = ShardedJsonlStore::new();
    store.append_line(&logical, br#"{"id":1}"#).expect("append");
    store.append_line(&logical, br#"{"id":2}"#).expect("append");

    // 模拟上一次重写崩溃：留下一个不属于任何分片的临时文件。
    let stale = logical.with_extension("rewrite9.tmp");
    fs::write(&stale, b"{\"id\":9}").expect("写临时文件");
    assert!(store.has_stale_rewrite_temporaries(&logical).expect("探测"));

    store
        .rewrite_with(&logical, |_| Ok(((), vec![br#"{"id":3}"#.to_vec()])))
        .expect("重写");

    assert!(!stale.exists(), "重写前必须清掉遗留临时文件");
    assert!(
        !store.has_stale_rewrite_temporaries(&logical).expect("探测"),
        "重写后不得留下临时文件"
    );
    let lines = store.read_lines(&logical).expect("lines");
    assert_eq!(lines, [br#"{"id":3}"#.to_vec()]);
}

#[test]
fn rewrite_removes_superseded_shards() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("snapshot.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    store
        .append_line(&logical, br#"{"version":1}"#)
        .expect("append");
    store
        .append_line(&logical, br#"{"version":2}"#)
        .expect("append");

    store
        .rewrite_with(&logical, |lines| {
            Ok((lines.len(), vec![b"{\"version\":2}".to_vec()]))
        })
        .expect("rewrite");

    assert_eq!(store.shard_paths(&logical).expect("paths").len(), 1);
    assert_eq!(
        store.read_lines(&logical).expect("lines"),
        [b"{\"version\":2}".to_vec()]
    );
}

#[test]
fn rewrite_to_empty_removes_all_shards() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("empty.jsonl");
    let store = ShardedJsonlStore::new();
    store.append_line(&logical, br#"{"id":1}"#).expect("append");

    store
        .rewrite_with(&logical, |_| Ok(((), Vec::new())))
        .expect("rewrite empty");

    assert!(store.shard_paths(&logical).expect("paths").is_empty());
}

#[test]
fn rewrite_rejects_invalid_lines_without_data_loss() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("guarded.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    store
        .append_line(&logical, br#"{"version":1}"#)
        .expect("append");

    let result = store.rewrite_with(&logical, |_| Ok(((), vec![b"{\"broken\":\n}".to_vec()])));
    assert!(result.is_err());
    assert_eq!(
        store.read_lines(&logical).expect("lines"),
        [b"{\"version\":1}".to_vec()]
    );
}

#[test]
fn remove_all_clears_physical_shards() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("temporary.jsonl");
    let store = ShardedJsonlStore::new();
    store.append_line(&logical, br#"{"id":1}"#).expect("append");

    assert!(store.remove_all(&logical).expect("remove"));
    assert!(store.shard_paths(&logical).expect("paths").is_empty());
    assert!(store.read_lines(&logical).expect("lines").is_empty());
}

#[test]
fn rejects_newline_and_isolates_oversized_lines() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("invalid.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(4);

    assert!(store.append_line(&logical, "{\n}").is_err());
    // 单条超过 shard 预算不再拒绝：让它独占一个 shard，读回来仍是同一条记录。
    store
        .append_line(&logical, br#"{"too-long":1}"#)
        .expect("oversized line is isolated");
    assert_eq!(
        store.read_lines(&logical).expect("lines"),
        vec![br#"{"too-long":1}"#.to_vec()]
    );
    assert_eq!(store.shard_paths(&logical).expect("paths").len(), 1);
}

#[test]
fn line_budget_includes_jsonl_newline() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("budget.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);

    store
        .append_line(&logical, br#"{"key":"value"}"#)
        .expect("line plus newline fits");
    let shard = ShardedJsonlStore::physical_shard_path(&logical, 0);
    assert_eq!(shard.metadata().expect("shard metadata").len(), 16);

    // 多一个字符就越过预算：换行也算进体量，因此它独占下一个 shard。
    store
        .append_line(&logical, br#"{"keys":"value"}"#)
        .expect("oversized line is isolated");
    let next = ShardedJsonlStore::physical_shard_path(&logical, 1);
    assert_eq!(next.metadata().expect("shard metadata").len(), 17);
}

#[test]
fn append_budget_keeps_stored_bytes_within_shard() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("boundary.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);

    store
        .append_line(&logical, br#"{"key":"value"}"#)
        .expect("append");
    store.append_line(&logical, b"x").expect("append");

    let paths = store.shard_paths(&logical).expect("shard paths");
    assert_eq!(paths.len(), 2);
    assert!(
        paths
            .iter()
            .all(|path| path.metadata().expect("shard metadata").len() <= 16)
    );
}

#[test]
fn bounded_budget_includes_jsonl_newline() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("bounded.jsonl");
    let store = ShardedJsonlStore::new();

    store
        .append_bounded_line(&logical, br#"{"key":"value"}"#, 16)
        .expect("append");
    assert!(store.append_bounded_line(&logical, b"x", 16).is_err());
    assert_eq!(store.shard_paths(&logical).expect("paths").len(), 1);
    assert_eq!(
        ShardedJsonlStore::physical_shard_path(&logical, 0)
            .metadata()
            .expect("shard metadata")
            .len(),
        16
    );
}

#[test]
fn read_repaired_lines_rejects_incomplete_interior_shard() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("repair.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    store
        .append_line(&logical, br#"{"key":"value"}"#)
        .expect("append");
    store.append_line(&logical, b"x").expect("append");
    let interior = ShardedJsonlStore::physical_shard_path(&logical, 0);
    let mut file = fs::OpenOptions::new().append(true).open(&interior).unwrap();
    file.write_all(b"{").unwrap();
    drop(file);

    let error = store.read_repaired_lines(&logical).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(interior.metadata().unwrap().len(), 17);
}

#[test]
fn append_repairs_incomplete_tail_before_plain_append() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("append-repair.jsonl");
    let store = ShardedJsonlStore::new();
    store.append_line(&logical, br#"{"id":1}"#).expect("append");
    let shard = ShardedJsonlStore::physical_shard_path(&logical, 0);
    let mut file = fs::OpenOptions::new().append(true).open(&shard).unwrap();
    file.write_all(b"{").unwrap();
    drop(file);

    store
        .append_line(&logical, br#"{"id":2}"#)
        .expect("append after repair");

    let lines = store.read_lines(&logical).expect("lines");
    assert_eq!(lines, [br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()]);
}

#[test]
fn append_with_rejects_batch_before_writing_any_line() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("batch-append.jsonl");
    let store = ShardedJsonlStore::new();

    let result = store.append_with(&logical, |_| {
        Ok((
            (),
            vec![
                br#"{"valid":1}"#.to_vec(),
                br#"{"broken":
}"#
                .to_vec(),
            ],
        ))
    });

    assert!(result.is_err());
    assert!(store.shard_paths(&logical).expect("paths").is_empty());
}

#[test]
fn append_rejects_sequence_beyond_fixed_width() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("sequence.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    let last_shard = directory.path().join("sequence.999999.jsonl");
    fs::write(&last_shard, b"{\"id\":1}\n").expect("write last shard");

    assert!(store.append_line(&logical, br#"{"id":2}"#).is_err());
    assert_eq!(store.shard_paths(&logical).expect("paths").len(), 1);
    assert_eq!(fs::read(&last_shard).unwrap(), b"{\"id\":1}\n");
}

#[test]
fn shard_scan_requires_fixed_width_sequence() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("stable.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);
    store
        .append_line(&logical, br#"{"key":"value"}"#)
        .expect("append");

    let unexpected = directory.path().join("stable.1.jsonl");
    fs::write(&unexpected, b"{}\n").expect("write unrelated file");
    let overlong = directory.path().join("stable.0000000.jsonl");
    fs::write(&overlong, b"{}\n").expect("write malformed shard");

    let paths = store.shard_paths(&logical).expect("shard paths");
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("stable.000000.jsonl"));
}
