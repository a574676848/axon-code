use super::*;

fn root(tag: &str) -> PathBuf {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-apply-{tag}-{}-{}",
        std::process::id(),
        TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn applies_relative_path_outside_workspace_when_requested() {
    let workspace = root("outside-relative");
    let outside = workspace
        .parent()
        .unwrap()
        .join(format!("axon-apply-outside-{}.txt", std::process::id()));
    let patch_path = format!("../{}", outside.file_name().unwrap().to_string_lossy());
    let patch = format!("*** Begin Patch\n*** Add File: {patch_path}\n+created\n*** End Patch");

    ApplyPatchExecutor.apply(&workspace, &patch).unwrap();
    assert_eq!(fs::read_to_string(&outside).unwrap(), "created\n");
    fs::remove_file(outside).unwrap();
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn applies_absolute_path_outside_workspace_when_requested() {
    let workspace = root("outside-absolute");
    let outside = axon_test_support::test_temp_root().join(format!(
        "axon-apply-absolute-{}-{}.txt",
        std::process::id(),
        TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let absolute = outside.display().to_string().replace('\\', "/");
    let patch = format!("*** Begin Patch\n*** Add File: {absolute}\n+created\n*** End Patch");

    ApplyPatchExecutor.apply(&workspace, &patch).unwrap();
    assert_eq!(fs::read_to_string(&outside).unwrap(), "created\n");
    fs::remove_file(outside).unwrap();
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn accepts_windows_path_separators() {
    let workspace = root("windows-separators");
    let patch = "*** Begin Patch\n*** Add File: nested\\new.txt\n+created\n*** End Patch";
    ApplyPatchExecutor.apply(&workspace, patch).unwrap();
    assert_eq!(
        fs::read_to_string(workspace.join("nested").join("new.txt")).unwrap(),
        "created\n"
    );
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn add_file_overwrites_an_existing_file_through_the_update_transaction() {
    let workspace = root("add-overwrites");
    let target = workspace.join("doc.md");
    fs::write(&target, "# old\n\nbody\n").unwrap();
    let patch = "*** Begin Patch\n*** Add File: doc.md\n+# new\n+\n+rewritten\n*** End Patch";

    let report = ApplyPatchExecutor.apply(&workspace, patch).unwrap();

    assert_eq!(fs::read_to_string(&target).unwrap(), "# new\n\nrewritten\n");
    assert_eq!(report.added, 0);
    assert_eq!(report.updated, 1);
    assert_eq!(report.files[0].operation, "update");
    // 重写必须在回执里给出真实的前后对比，而不是把重写显示成新增。
    assert!(report.files[0].diff.contains("-# old"));
    assert!(report.files[0].diff.contains("+# new"));
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn add_file_still_creates_a_missing_file() {
    let workspace = root("add-creates");
    let patch = "*** Begin Patch\n*** Add File: fresh.md\n+created\n*** End Patch";

    let report = ApplyPatchExecutor.apply(&workspace, patch).unwrap();

    assert_eq!(report.added, 1);
    assert_eq!(report.updated, 0);
    assert_eq!(report.files[0].operation, "add");
    assert_eq!(
        fs::read_to_string(workspace.join("fresh.md")).unwrap(),
        "created\n"
    );
    fs::remove_dir_all(workspace).unwrap();
}

#[cfg(windows)]
#[test]
fn accepts_windows_extended_absolute_path_inside_workspace() {
    let workspace = root("extended-absolute");
    let target = workspace.join("extended.txt");
    fs::write(&target, "before\n").unwrap();
    let absolute = target.display().to_string().replace('\\', "/");
    let extended = format!("//?/{absolute}");
    let patch =
        format!("*** Begin Patch\n*** Update File: {extended}\n@@\n-before\n+after\n*** End Patch");

    ApplyPatchExecutor.apply(&workspace, &patch).unwrap();
    assert_eq!(fs::read_to_string(target).unwrap(), "after\n");
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn precondition_failure_leaves_earlier_files_unchanged() {
    let workspace = root("precondition");
    fs::write(workspace.join("first.txt"), "before\n").unwrap();
    fs::write(workspace.join("mover.txt"), "move\n").unwrap();
    fs::write(workspace.join("occupied.txt"), "keep\n").unwrap();
    // Add File 现在允许覆盖已存在文件（整文件重写），因此预检失败的判据改用
    // 仍然会被拒绝的情形：Move 目标已被占用。
    let patch = "*** Begin Patch\n*** Update File: first.txt\n@@\n-before\n+after\n*** Update File: mover.txt\n*** Move to: occupied.txt\n@@\n-move\n+moved\n*** End Patch";
    assert!(ApplyPatchExecutor.apply(&workspace, patch).is_err());
    assert_eq!(
        fs::read_to_string(workspace.join("first.txt")).unwrap(),
        "before\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("mover.txt")).unwrap(),
        "move\n"
    );
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn applies_all_file_operations_and_preserves_existing_text_format() {
    let workspace = root("all-operations");
    fs::write(workspace.join("update.rs"), "\u{feff}first\r\nold\r\n").unwrap();
    fs::write(workspace.join("delete.txt"), "gone\n").unwrap();
    fs::write(workspace.join("move.txt"), "before\n").unwrap();
    let patch = "*** Begin Patch\n*** Add File: nested/new.txt\n+new\n*** Update File: update.rs\n@@\n-old\n+updated\n*** Delete File: delete.txt\n*** Update File: move.txt\n*** Move to: moved.txt\n@@\n-before\n+after\n*** End Patch";
    let report = ApplyPatchExecutor.apply(&workspace, patch).unwrap();
    assert_eq!(
        (report.added, report.updated, report.deleted, report.moved),
        (1, 2, 1, 1)
    );
    assert_eq!(
        report
            .files
            .iter()
            .map(|file| file.operation.as_str())
            .collect::<Vec<_>>(),
        ["add", "update", "delete", "move"]
    );
    assert_eq!(report.files[2].diff, "gone\n");
    assert_eq!(report.files[3].source_path.as_deref(), Some("move.txt"));
    assert_eq!(
        fs::read_to_string(workspace.join("nested/new.txt")).unwrap(),
        "new\n"
    );
    assert_eq!(
        fs::read(workspace.join("update.rs")).unwrap(),
        "\u{feff}first\r\nupdated\r\n".as_bytes()
    );
    assert!(!workspace.join("delete.txt").exists());
    assert!(!workspace.join("move.txt").exists());
    assert_eq!(
        fs::read_to_string(workspace.join("moved.txt")).unwrap(),
        "after\n"
    );
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn invalid_or_unmatched_patch_leaves_the_workspace_unchanged() {
    let workspace = root("rejected-patch");
    let path = workspace.join("source.txt");
    fs::write(&path, "actual\n").unwrap();
    assert!(ApplyPatchExecutor.apply(&workspace, "not a patch").is_err());
    assert!(
        ApplyPatchExecutor
            .apply(
                &workspace,
                "*** Begin Patch\n*** Update File: source.txt\n@@\n-missing\n+new\n*** End Patch"
            )
            .is_err()
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "actual\n");
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn failed_second_promotion_restores_every_source() {
    use axon_patch_model::PatchFileOperation;
    let workspace = root("rollback-second-promotion");
    fs::write(workspace.join("first-source.txt"), "first-before\n").unwrap();
    fs::write(workspace.join("second-source.txt"), "second-before\n").unwrap();
    let plan = PatchPlan {
        operations: vec![
            PatchFileOperation {
                path: "first-source.txt".to_owned(),
                change: PatchFileChange::Update {
                    move_path: Some("first-target.txt".to_owned()),
                    new_content: "first-after\n".to_owned(),
                },
            },
            PatchFileOperation {
                path: "second-source.txt".to_owned(),
                change: PatchFileChange::Update {
                    move_path: Some("second-target.txt".to_owned()),
                    new_content: "second-after\n".to_owned(),
                },
            },
        ],
    };
    let snapshot = HashMap::from([
        ("first-source.txt".to_owned(), "first-before\n".to_owned()),
        ("second-source.txt".to_owned(), "second-before\n".to_owned()),
    ]);
    let mut transaction = TransactionRoot::create(&workspace).unwrap();
    let transaction_path = transaction.path().to_path_buf();
    let mut entries = build_entries(&workspace, &plan, &snapshot).unwrap();
    verify_preconditions(&mut entries).unwrap();
    stage_contents(&transaction_path, &mut entries).unwrap();
    fs::write(workspace.join("second-target.txt"), "external\n").unwrap();
    let error = commit(&mut transaction, &mut entries).expect_err("提交失败后应回滚并报错");

    // B4：事务状态的措辞只能由**实际执行回滚的代码**给出。本用例回滚成功，
    // 因此必须明说成功，且不得同时出现失败措辞。
    assert!(
        error.to_string().contains("已全部安全回滚"),
        "回滚成功时必须明确告知：{error}"
    );
    assert!(
        !error.to_string().contains("回滚**未完成**"),
        "回滚成功时不得给出失败措辞：{error}"
    );
    drop(transaction);
    assert!(!transaction_path.exists(), "回滚成功后事务目录应被清理");

    // B4 反向对照：`preserve()` 置位后 Drop **不得**删除事务目录：
    // 其中的备份是目标文件可能唯一的副本，删除即永久数据丢失。
    let mut kept = TransactionRoot::create(&workspace).unwrap();
    let kept_path = kept.path().to_path_buf();
    kept.preserve();
    drop(kept);
    assert!(kept_path.exists(), "标记保留后 Drop 不得删除事务目录");
    fs::remove_dir_all(&kept_path).unwrap();
    assert_eq!(
        fs::read_to_string(workspace.join("first-source.txt")).unwrap(),
        "first-before\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("second-source.txt")).unwrap(),
        "second-before\n"
    );
    assert!(!workspace.join("first-target.txt").exists());
    assert_eq!(
        fs::read_to_string(workspace.join("second-target.txt")).unwrap(),
        "external\n"
    );
    fs::remove_dir_all(workspace).unwrap();
}

#[cfg(unix)]
#[test]
fn rejects_symlink_source_and_target_parent() {
    use std::os::unix::fs::symlink;
    let workspace = root("symlink");
    let outside = root("symlink-outside");
    fs::write(outside.join("source.txt"), "before\n").unwrap();
    symlink(outside.join("source.txt"), workspace.join("source.txt")).unwrap();
    symlink(&outside, workspace.join("linked")).unwrap();
    let update = "*** Begin Patch\n*** Update File: source.txt\n@@\n-before\n+after\n*** End Patch";
    let add = "*** Begin Patch\n*** Add File: linked/escape.txt\n+bad\n*** End Patch";
    assert!(ApplyPatchExecutor.apply(&workspace, update).is_err());
    assert!(ApplyPatchExecutor.apply(&workspace, add).is_err());
    assert_eq!(
        fs::read_to_string(outside.join("source.txt")).unwrap(),
        "before\n"
    );
    assert!(!outside.join("escape.txt").exists());
    fs::remove_dir_all(workspace).unwrap();
    fs::remove_dir_all(outside).unwrap();
}
