use super::*;

#[test]
fn test_parse_hunk_header() {
    let planner = AtomicCommitPlanner::default();
    let hunk = planner.parse_hunk_header("@@ -1,5 +1,6 @@").unwrap();
    assert_eq!(hunk.old_start, 1);
    assert_eq!(hunk.old_count, 5);
    assert_eq!(hunk.new_start, 1);
    assert_eq!(hunk.new_count, 6);
}

#[test]
fn test_group_by_directory() {
    let planner = AtomicCommitPlanner::new(CommitGroupStrategy::ByDirectory);
    let changes = vec![
        FileChange {
            path: "src/main.rs".to_owned(),
            status: FileChangeStatus::Modified,
            old_path: None,
            hunks: vec![],
        },
        FileChange {
            path: "src/lib.rs".to_owned(),
            status: FileChangeStatus::Modified,
            old_path: None,
            hunks: vec![],
        },
        FileChange {
            path: "tests/integ.rs".to_owned(),
            status: FileChangeStatus::Added,
            old_path: None,
            hunks: vec![],
        },
    ];

    let groups = planner.group_changes(changes);
    assert_eq!(groups.len(), 2);

    let src_group = groups.iter().find(|g| g.name == "src").unwrap();
    assert_eq!(src_group.files.len(), 2);
}

#[test]
fn test_infer_commit_type() {
    let planner = AtomicCommitPlanner::default();

    let test_files = vec!["tests/test_foo.rs".to_owned()];
    assert_eq!(
        planner.infer_commit_type(&test_files),
        AtomicCommitType::Test
    );

    let doc_files = vec!["README.md".to_owned()];
    assert_eq!(
        planner.infer_commit_type(&doc_files),
        AtomicCommitType::Docs
    );

    let config_files = vec!["Cargo.toml".to_owned()];
    assert_eq!(
        planner.infer_commit_type(&config_files),
        AtomicCommitType::Config
    );
}

#[test]
fn test_validate_plan() {
    let planner = AtomicCommitPlanner::default();
    let changes = vec![
        FileChange {
            path: "foo.rs".to_owned(),
            status: FileChangeStatus::Modified,
            old_path: None,
            hunks: vec![],
        },
        FileChange {
            path: "bar.rs".to_owned(),
            status: FileChangeStatus::Added,
            old_path: None,
            hunks: vec![],
        },
    ];

    let plan = vec![AtomicCommit {
        files: vec!["foo.rs".to_owned(), "bar.rs".to_owned()],
        message: "feat: update".to_owned(),
        commit_type: AtomicCommitType::Feature,
        scope: Some("src".to_owned()),
    }];

    assert!(planner.validate_plan(&plan, &changes));

    let incomplete_plan = vec![AtomicCommit {
        files: vec!["foo.rs".to_owned()],
        message: "feat: foo".to_owned(),
        commit_type: AtomicCommitType::Feature,
        scope: None,
    }];

    assert!(!planner.validate_plan(&incomplete_plan, &changes));
}
