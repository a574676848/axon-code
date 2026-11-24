//! `search_index` 单元测试：候选过滤的 fail-open 语义、覆盖层、淘汰与取消。

use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use super::{
    builder,
    candidate_filter::select_candidates,
    manager::{IndexQuery, IndexState, SearchIndexManager, rebuild_allowed, stale_files},
    model::{
        FileFingerprint, INDEX_FORMAT_VERSION, IndexKey, IndexSnapshot, IndexedFile,
        SEARCH_INDEX_SCHEMA_VERSION, SearchIndexPolicy, TrigramKey,
    },
    overlay::LiveOverlay,
    planner::build_query_plan,
    trigram::extract,
};
use crate::workspace_walker::{WorkspaceWalkEntry, WorkspaceWalkReport};

/// 模拟 walker 观察到的 entry：长度与修改时间来自真实文件元数据。
fn entry(root: &std::path::Path, name: &str) -> WorkspaceWalkEntry {
    let path = root.join(name);
    let fingerprint = path
        .metadata()
        .ok()
        .and_then(|metadata| FileFingerprint::from_metadata(&metadata));
    WorkspaceWalkEntry {
        path,
        display_path: name.to_owned(),
        is_dir: false,
        file_len: fingerprint
            .as_ref()
            .map_or(0, |fingerprint| fingerprint.length),
        modified_at_ms: fingerprint.map_or(0, |fingerprint| fingerprint.modified_at_ms),
    }
}

fn report(entries: Vec<WorkspaceWalkEntry>) -> WorkspaceWalkReport {
    WorkspaceWalkReport {
        entries,
        ..Default::default()
    }
}

/// 构造与真实文件一致的基础快照，posting 使用稳定 file id。
fn snapshot(root: &std::path::Path, files: &[(&str, &[u8], bool)]) -> IndexSnapshot {
    let mut names = files.iter().map(|(name, _, _)| *name).collect::<Vec<_>>();
    names.sort_unstable();

    let mut indexed_files = BTreeMap::new();
    let mut postings = BTreeMap::<TrigramKey, Vec<u32>>::new();
    for (index, name) in names.iter().enumerate() {
        let (_, bytes, searchable) = files
            .iter()
            .find(|(candidate, _, _)| candidate == name)
            .expect("测试夹具必须定义该文件");
        let metadata = fs::metadata(root.join(name)).expect("测试文件必须存在");
        let id = u32::try_from(index).expect("测试文件数量在 u32 范围内");
        indexed_files.insert(
            (*name).to_owned(),
            IndexedFile {
                id,
                fingerprint: FileFingerprint::from_metadata(&metadata)
                    .expect("测试文件必须有修改时间"),
                searchable: *searchable,
            },
        );
        if *searchable {
            for trigram in extract(bytes) {
                postings.entry(trigram).or_default().push(id);
            }
        }
    }
    for ids in postings.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }

    let mut snapshot = IndexSnapshot {
        format_version: INDEX_FORMAT_VERSION,
        schema_version: SEARCH_INDEX_SCHEMA_VERSION,
        root: root.to_path_buf(),
        hidden: false,
        gitignore: true,
        generation: 1,
        files: indexed_files,
        postings,
        paths: BTreeMap::new(),
    };
    snapshot.rebuild_paths();
    snapshot
}

#[test]
fn candidate_filter_excludes_only_known_text_files_without_a_match() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("match.txt"), b"abc").unwrap();
    fs::write(directory.path().join("other.txt"), b"xyz").unwrap();
    fs::write(directory.path().join("binary.bin"), [0, 1, 2]).unwrap();
    let snapshot = snapshot(
        directory.path(),
        &[
            ("match.txt", b"abc", true),
            ("other.txt", b"xyz", true),
            ("binary.bin", &[0, 1, 2], false),
        ],
    );
    let entries = [
        entry(directory.path(), "match.txt"),
        entry(directory.path(), "other.txt"),
        entry(directory.path(), "binary.bin"),
    ];

    let selection = select_candidates(
        &snapshot,
        &LiveOverlay::default(),
        directory.path(),
        &entries,
        &build_query_plan("abc", true, true),
    );

    assert!(selection.should_scan(&directory.path().join("match.txt")));
    assert!(!selection.should_scan(&directory.path().join("other.txt")));
    assert!(selection.should_scan(&directory.path().join("binary.bin")));
}

#[test]
fn uncertain_query_keeps_all_files_scanable() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("a.txt"), b"abc").unwrap();
    let snapshot = snapshot(directory.path(), &[("a.txt", b"abc", true)]);
    let entries = [entry(directory.path(), "a.txt")];

    let selection = select_candidates(
        &snapshot,
        &LiveOverlay::default(),
        directory.path(),
        &entries,
        &build_query_plan("abc", false, true),
    );

    assert!(selection.should_scan(&directory.path().join("a.txt")));
}

#[test]
fn builder_budget_keeps_unindexed_files_fail_open() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("a.txt"), b"abc").unwrap();
    fs::write(directory.path().join("b.txt"), b"xyz").unwrap();
    let entries = [
        entry(directory.path(), "a.txt"),
        entry(directory.path(), "b.txt"),
    ];
    let snapshot = builder::build_snapshot(
        &IndexKey::new(directory.path(), false, true),
        &entries,
        1,
        SearchIndexPolicy {
            max_file_size: 1024,
            max_indexed_bytes: 3,
            min_workspace_files: 0,
            ..SearchIndexPolicy::default()
        },
        &AtomicBool::new(false),
    )
    .expect("未取消时必须产出快照");

    let indexed_id = snapshot.files["a.txt"].id;
    assert!(snapshot.files["a.txt"].searchable);
    assert!(!snapshot.files["b.txt"].searchable);
    assert!(
        snapshot
            .postings
            .values()
            .all(|ids| ids == &vec![indexed_id])
    );
}

#[test]
fn small_workspace_does_not_schedule_a_build() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("a.txt"), b"abc").unwrap();
    let manager = SearchIndexManager::default();
    let walk = report(vec![entry(directory.path(), "a.txt")]);

    let decision = manager.prepare(
        directory.path(),
        &walk,
        IndexQuery {
            query: "abc",
            is_literal: true,
            case_sensitive: true,
            hidden: false,
            gitignore: true,
            allow_update: true,
        },
    );

    assert!(decision.take_update().is_none());
}

#[test]
fn edited_file_is_stale_until_overlay_covers_it() {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("a.txt");
    fs::write(&path, b"abc").unwrap();
    let base = Arc::new(snapshot(directory.path(), &[("a.txt", b"abc", true)]));
    let state = IndexState::new(Arc::clone(&base), Arc::new(LiveOverlay::default()), 0);

    fs::write(&path, b"abcdef").unwrap();
    let entries = [entry(directory.path(), "a.txt")];
    let (upserts, removals) = stale_files(&state, &entries, directory.path());
    assert_eq!(upserts.len(), 1);
    assert!(removals.is_empty());

    let mut overlay = LiveOverlay::default();
    overlay.ingest(directory.path(), &entries[0], SearchIndexPolicy::default());
    let covered = IndexState::new(base, Arc::new(overlay), 0);
    let (upserts, removals) = stale_files(&covered, &entries, directory.path());

    assert!(upserts.is_empty());
    assert!(removals.is_empty());
}

#[test]
fn removed_file_becomes_tombstone() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("a.txt"), b"abc").unwrap();
    fs::write(directory.path().join("b.txt"), b"abc").unwrap();
    let base = Arc::new(snapshot(
        directory.path(),
        &[("a.txt", b"abc", true), ("b.txt", b"abc", true)],
    ));
    let state = IndexState::new(base, Arc::new(LiveOverlay::default()), 0);

    fs::remove_file(directory.path().join("b.txt")).unwrap();
    let entries = [entry(directory.path(), "a.txt")];
    let (upserts, removals) = stale_files(&state, &entries, directory.path());

    assert!(upserts.is_empty());
    assert_eq!(removals, vec!["b.txt".to_owned()]);
}

#[test]
fn overlay_merge_produces_new_generation_and_drops_tombstones() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("keep.txt"), b"abc").unwrap();
    fs::write(directory.path().join("gone.txt"), b"abc").unwrap();
    let base = snapshot(
        directory.path(),
        &[("keep.txt", b"abc", true), ("gone.txt", b"abc", true)],
    );

    let mut overlay = LiveOverlay::default();
    overlay.remove("gone.txt");
    overlay.upsert(
        "added.txt",
        FileFingerprint {
            length: 6,
            modified_at_ms: 42,
        },
        true,
        extract(b"abcdef"),
    );

    let merged = overlay.merge_into(&base, 9);

    assert_eq!(merged.generation, 9);
    assert!(merged.files.contains_key("keep.txt"));
    assert!(merged.files.contains_key("added.txt"));
    assert!(!merged.files.contains_key("gone.txt"));
    assert!(
        merged
            .postings
            .values()
            .all(|ids| ids.iter().all(|id| merged.path_of(*id).is_some()))
    );
    assert_eq!(
        merged.path_of(merged.files["added.txt"].id),
        Some("added.txt")
    );
}

#[test]
fn eviction_keeps_tracked_state_count_bounded() {
    let manager = SearchIndexManager::default();
    let policy = SearchIndexPolicy::default();
    for index in 0..policy.max_tracked_workspaces.saturating_add(2) {
        let root = std::path::PathBuf::from(format!("E:/axon-evict-{index}"));
        let state = Arc::new(IndexState::new(
            Arc::new(snapshot(&root, &[])),
            Arc::new(LiveOverlay::default()),
            index as u64,
        ));
        manager.insert_state(IndexKey::new(&root, false, true), state);
    }

    let tracked = manager.tracked_state_count();
    assert!(tracked <= policy.max_tracked_workspaces);
}

#[test]
fn cancelled_rebuild_publishes_nothing() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("a.txt"), b"abc").unwrap();
    let entries = [entry(directory.path(), "a.txt")];
    let manager = Arc::new(SearchIndexManager::default());
    let key = IndexKey::new(directory.path(), false, true);
    let cancel = Arc::new(AtomicBool::new(true));

    manager.run_rebuild(key.clone(), &entries, SearchIndexPolicy::default(), &cancel);

    assert!(manager.cached_state(&key).is_none());
    assert_eq!(manager.active_task_count(), 0);
}

#[test]
fn cancelled_build_stops_early_without_publishing() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("a.txt"), b"abc").unwrap();
    let entries = [entry(directory.path(), "a.txt")];
    let cancel = AtomicBool::new(false);
    cancel.store(true, Ordering::Relaxed);

    let snapshot = builder::build_snapshot(
        &IndexKey::new(directory.path(), false, true),
        &entries,
        1,
        SearchIndexPolicy::default(),
        &cancel,
    );

    assert!(snapshot.is_none());
}

#[test]
fn rebuild_cooldown_suppresses_repeated_builds() {
    let cooldown = Duration::from_secs(30);
    let now = Instant::now();

    assert!(rebuild_allowed(None, cooldown, now));
    assert!(!rebuild_allowed(Some(now), cooldown, now));
    assert!(rebuild_allowed(Some(now), cooldown, now + cooldown));
}

#[test]
fn manager_is_send_sync_and_policy_is_bounded() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SearchIndexManager>();

    let policy = SearchIndexPolicy::default();
    assert!(policy.max_file_size > 0);
    assert!(policy.max_tracked_workspaces > 0);
    assert!(policy.max_overlay_files > 0);
    assert!(policy.max_overlay_bytes > 0);
}

#[test]
fn relative_path_matches_even_with_short_or_canonical_asymmetry() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    let file = root.join("child.txt");
    fs::write(&file, "test").unwrap();

    let canonical_root = std::fs::canonicalize(root).unwrap();
    let canonical_file = std::fs::canonicalize(&file).unwrap();

    assert_eq!(
        super::model::relative_path(root, &canonical_file).as_deref(),
        Some("child.txt")
    );
    assert_eq!(
        super::model::relative_path(&canonical_root, &file).as_deref(),
        Some("child.txt")
    );
    assert_eq!(
        super::model::relative_path(&canonical_root, &canonical_file).as_deref(),
        Some("child.txt")
    );
}
