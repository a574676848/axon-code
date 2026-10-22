use super::*;

fn section(path: &str, operation: HashlineFileOperation) -> HashlineSection {
    HashlineSection {
        path: path.to_owned(),
        tag: "AB12".to_owned(),
        ops: Vec::new(),
        file_operation: operation,
        warnings: Vec::new(),
    }
}

#[test]
fn file_operation_validation_rejects_duplicate_move_destination() {
    let sections = vec![
        section(
            "src/a.rs",
            HashlineFileOperation::Move {
                destination: "src/out.rs".to_owned(),
            },
        ),
        section(
            "src/b.rs",
            HashlineFileOperation::Move {
                destination: "src/out.rs".to_owned(),
            },
        ),
    ];

    let error = validate_file_operations(&sections).expect_err("重复 MV 目标必须拒绝");

    assert!(error.contains("同一目标路径"));
}

#[test]
fn file_operation_validation_rejects_destination_that_is_another_source() {
    let sections = vec![
        section(
            "src/a.rs",
            HashlineFileOperation::Move {
                destination: "src/b.rs".to_owned(),
            },
        ),
        section("src/b.rs", HashlineFileOperation::Update),
    ];

    let error = validate_file_operations(&sections).expect_err("MV 目标不能占用同批次源路径");

    assert!(error.contains("同批次源路径"));
}

#[test]
fn range_boundary_insert_is_allowed_but_interior_insert_is_rejected() {
    let boundary = vec![
        HashlineOp::InsertBefore {
            line: 2,
            lines: vec!["before".to_owned()],
        },
        HashlineOp::Swap {
            start: 2,
            end: 4,
            lines: vec!["replacement".to_owned()],
        },
        HashlineOp::InsertAfter {
            line: 4,
            lines: vec!["after".to_owned()],
            block_start: None,
        },
    ];
    assert!(ensure_non_overlapping(&boundary).is_ok());

    let interior = vec![
        HashlineOp::Swap {
            start: 2,
            end: 4,
            lines: vec!["replacement".to_owned()],
        },
        HashlineOp::InsertBefore {
            line: 3,
            lines: vec!["invalid".to_owned()],
        },
    ];
    assert!(ensure_non_overlapping(&interior).is_err());
}

#[test]
fn boundary_insert_applies_after_replacement_without_being_consumed() {
    let mut lines = vec!["old".to_owned(), "}".to_owned()];
    let mut seen = BTreeSet::from([1, 2]);
    let mut ops = vec![
        HashlineOp::InsertBefore {
            line: 1,
            lines: vec!["if (a) {".to_owned()],
        },
        HashlineOp::Swap {
            start: 1,
            end: 1,
            lines: vec!["new".to_owned()],
        },
    ];

    sort_ops_for_application(&mut ops);
    for op in ops {
        apply_op(&mut lines, &mut seen, op).unwrap();
    }

    assert_eq!(lines, vec!["if (a) {", "new", "}"]);
}

#[test]
fn delete_and_same_anchor_insert_after_keeps_original_line_slot() {
    let mut lines = vec![
        "before".to_owned(),
        "removed".to_owned(),
        "after".to_owned(),
    ];
    let mut seen = BTreeSet::from([1, 2, 3]);
    let mut ops = vec![
        HashlineOp::Delete { start: 2, end: 2 },
        HashlineOp::InsertAfter {
            line: 2,
            lines: vec!["inserted".to_owned()],
            block_start: None,
        },
    ];

    sort_ops_for_application(&mut ops);
    for op in ops {
        apply_op(&mut lines, &mut seen, op).unwrap();
    }

    assert_eq!(lines, vec!["before", "inserted", "after"]);
}

#[test]
fn multiple_head_and_tail_hunks_keep_authored_order() {
    let mut lines = vec!["middle".to_owned()];
    let mut seen = BTreeSet::from([1]);
    let mut ops = vec![
        HashlineOp::InsertHead {
            lines: vec!["head-a".to_owned()],
        },
        HashlineOp::InsertHead {
            lines: vec!["head-b".to_owned()],
        },
        HashlineOp::InsertTail {
            lines: vec!["tail-a".to_owned()],
        },
        HashlineOp::InsertTail {
            lines: vec!["tail-b".to_owned()],
        },
    ];

    sort_ops_for_application(&mut ops);
    for op in ops {
        apply_op(&mut lines, &mut seen, op).unwrap();
    }

    assert_eq!(
        lines,
        vec!["head-a", "head-b", "middle", "tail-a", "tail-b"]
    );
}
