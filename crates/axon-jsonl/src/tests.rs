use super::*;
use std::{fs, io, io::Write as _};

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
fn rejects_newline_and_oversized_lines() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("invalid.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(4);

    assert!(store.append_line(&logical, "{\n}").is_err());
    assert!(store.append_line(&logical, br#"{"too-long":1}"#).is_err());
    assert!(store.shard_paths(&logical).expect("paths").is_empty());
}

#[test]
fn line_budget_includes_jsonl_newline() {
    let directory = axon_test_support::test_temp_dir();
    let logical = directory.path().join("budget.jsonl");
    let store = ShardedJsonlStore::with_max_shard_bytes(16);

    assert!(store.append_line(&logical, br#"{"keys":"value"}"#).is_err());
    assert!(!ShardedJsonlStore::physical_shard_path(&logical, 0).exists());

    store
        .append_line(&logical, br#"{"key":"value"}"#)
        .expect("line plus newline fits");
    let shard = ShardedJsonlStore::physical_shard_path(&logical, 0);
    assert_eq!(shard.metadata().expect("shard metadata").len(), 16);
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
