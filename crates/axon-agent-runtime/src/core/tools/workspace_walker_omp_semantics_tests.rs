use std::fs;

use super::*;

#[test]
fn walker_always_skips_git_and_node_modules_unless_pattern_mentions_node_modules() {
    let root = axon_test_support::test_temp_dir();
    fs::create_dir_all(root.path().join(".git/objects")).unwrap();
    fs::create_dir_all(root.path().join("node_modules/pkg")).unwrap();
    fs::write(root.path().join(".git/objects/hidden.rs"), "").unwrap();
    fs::write(root.path().join("node_modules/pkg/index.js"), "").unwrap();
    fs::write(root.path().join("keep.rs"), "").unwrap();

    let default_report = WorkspaceWalker::walk(WorkspaceWalkInput {
        root: root.path().to_path_buf(),
        paths: vec!["**/*".to_owned()],
        hidden: true,
        gitignore: false,
        include: WorkspaceWalkInclude::Files,
        default_scope: WorkspaceWalkDefaultScope::RecursiveFiles,
        cancel_token: None,
    })
    .unwrap();
    let default_paths = default_report
        .entries
        .iter()
        .map(|entry| entry.display_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(default_paths, ["keep.rs"]);

    let explicit_report = WorkspaceWalker::walk(WorkspaceWalkInput {
        root: root.path().to_path_buf(),
        paths: vec!["node_modules/**/*.js".to_owned()],
        hidden: true,
        gitignore: false,
        include: WorkspaceWalkInclude::Files,
        default_scope: WorkspaceWalkDefaultScope::RecursiveFiles,
        cancel_token: None,
    })
    .unwrap();
    assert_eq!(
        explicit_report
            .entries
            .iter()
            .map(|entry| entry.display_path.as_str())
            .collect::<Vec<_>>(),
        ["node_modules/pkg/index.js"]
    );
}
