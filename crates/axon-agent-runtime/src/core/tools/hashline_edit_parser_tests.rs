use super::*;

fn parse(body: &str) -> Result<HashlineSection, String> {
    let sections = parse_sections(&format!("[src/a.rs#AB1234]\n{body}"))?;
    sections
        .into_iter()
        .next()
        .ok_or_else(|| "缺少测试 section".to_owned())
}

#[test]
fn rem_is_exclusive_with_line_operations() {
    let section = parse("REM").expect("REM 应解析");
    assert_eq!(section.file_operation, HashlineFileOperation::Delete);

    let error = parse("SWAP 1.=1:\n+one\nREM").expect_err("REM 与行操作必须互斥");
    assert!(error.contains("REM 不能与行编辑组合"));
}

#[test]
fn move_supports_standalone_and_edit_then_move() {
    let standalone = parse("MV src\\new.rs").expect("独立 MV 应解析");
    assert_eq!(
        standalone.file_operation,
        HashlineFileOperation::Move {
            destination: "src/new.rs".to_owned(),
        }
    );
    let edited = parse("SWAP 1.=1:\n+one\nMV src/new.rs").expect("编辑后 MV 应解析");
    assert_eq!(edited.ops.len(), 1);
    assert!(matches!(
        edited.file_operation,
        HashlineFileOperation::Move { .. }
    ));
}

#[test]
fn section_rejects_multiple_file_operations() {
    let rem = parse("REM\nREM").expect_err("重复 REM 必须拒绝");
    let move_twice = parse("MV src/b.rs\nMV src/c.rs").expect_err("重复 MV 必须拒绝");

    assert!(rem.contains("最多包含一个 REM 或 MV"));
    assert!(move_twice.contains("最多包含一个 REM 或 MV"));
}

#[test]
fn same_path_sections_merge_in_authored_order() {
    let sections = parse_sections(
        "[src/a.rs#AB1234]\nSWAP 2.=2:\n+two\n[src/b.rs#CD3456]\nDEL 1\n[src/a.rs#AB1234]\nINS.POST 4:\n+tail\nMV src/c.rs",
    )
    .expect("同路径 section 应合并");

    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].path, "src/a.rs");
    assert!(matches!(
        &sections[0].ops[..],
        [
            HashlineOp::Swap {
                start: 2,
                end: 2,
                ..
            },
            HashlineOp::InsertAfter { line: 4, .. }
        ]
    ));
    assert_eq!(
        sections[0].file_operation,
        HashlineFileOperation::Move {
            destination: "src/c.rs".to_owned(),
        }
    );
    assert_eq!(sections[1].path, "src/b.rs");
}

#[test]
fn same_path_sections_reject_conflicting_tags_and_file_operations() {
    let tag_error = parse_sections("[src/a.rs#AB1234]\nDEL 1\n[src/a.rs#CD3456]\nDEL 2")
        .expect_err("冲突 snapshot tag 必须拒绝");
    let operation_error =
        parse_sections("[src/a.rs#AB1234]\nMV src/b.rs\n[src/a.rs#AB1234]\nMV src/c.rs")
            .expect_err("多个文件级操作必须拒绝");
    let rem_error = parse_sections("[src/a.rs#AB1234]\nREM\n[src/a.rs#AB1234]\nDEL 1")
        .expect_err("跨 section 的 REM 与行编辑必须互斥");

    assert!(tag_error.contains("冲突的快照标签"));
    assert!(operation_error.contains("最多包含一个 REM 或 MV"));
    assert!(rem_error.contains("REM 不能与行编辑组合"));
}

#[test]
fn parses_all_block_operation_shapes() {
    let section = parse("SWAP.BLK 2:\n+A\nDEL.BLK 5\nINS.BLK.POST 8:\n+B").expect("BLK 应解析");

    assert!(matches!(
        section.ops[0],
        HashlineOp::SwapBlock { line: 2, .. }
    ));
    assert_eq!(section.ops[1], HashlineOp::DeleteBlock { line: 5 });
    assert!(matches!(
        section.ops[2],
        HashlineOp::InsertAfterBlock { line: 8, .. }
    ));
}

#[test]
fn accepts_omp_single_line_and_lenient_range_forms() {
    assert_eq!(
        parse("DEL 2").unwrap().ops[0],
        HashlineOp::Delete { start: 2, end: 2 }
    );
    assert_eq!(
        parse("DEL.RANGE: 20-23\n- #[serde(default)]").unwrap().ops[0],
        HashlineOp::Delete { start: 20, end: 23 }
    );
    for body in [
        "SWAP 2-3\n+X",
        "SWAP 2…3:\n+X",
        "SWAP 2 3:\n+X",
        "SWAP 2..3.:\n+X",
    ] {
        assert!(matches!(
            parse(body).unwrap().ops[0],
            HashlineOp::Swap {
                start: 2,
                end: 3,
                ..
            }
        ));
    }
    assert!(matches!(
        parse("INS.POST 2.:\n+X").unwrap().ops[0],
        HashlineOp::InsertAfter { line: 2, .. }
    ));
    assert!(matches!(
        parse("INS.HEAD.:\n+X").unwrap().ops[0],
        HashlineOp::InsertHead { .. }
    ));
}

#[test]
fn recovers_noisy_bom_and_quoted_header_paths() {
    let noisy =
        parse_sections("\u{feff}[*** Update/File: dir with spaces/a.rs#AB1234]\nDEL 1").unwrap();
    let quoted = parse_sections("[\"dir with spaces/a.rs\"#AB1234]\nMV \"next dir/a.rs\"").unwrap();

    assert_eq!(noisy[0].path, "dir with spaces/a.rs");
    assert_eq!(quoted[0].path, "dir with spaces/a.rs");
    assert_eq!(
        quoted[0].file_operation,
        HashlineFileOperation::Move {
            destination: "next dir/a.rs".to_owned(),
        }
    );
}

#[test]
fn strips_uniform_read_prefixes_but_preserves_mixed_and_numeric_keys() {
    let uniform = parse("SWAP 2.=3:\n2:foo\n\n3:bar").unwrap();
    let mixed = parse("SWAP 2.=3:\n3:keep\nplain").unwrap();
    let numeric = parse("SWAP 2.=3:\n1: \"one\",\n2: \"two\",").unwrap();

    assert!(matches!(
        &uniform.ops[0],
        HashlineOp::Swap { lines, .. }
            if lines == &vec!["foo".to_owned(), String::new(), "bar".to_owned()]
    ));
    assert!(matches!(
        &mixed.ops[0],
        HashlineOp::Swap { lines, .. }
            if lines == &vec!["3:keep".to_owned(), "plain".to_owned()]
    ));
    assert!(matches!(
        &numeric.ops[0],
        HashlineOp::Swap { lines, .. }
            if lines == &vec!["1: \"one\",".to_owned(), "2: \"two\",".to_owned()]
    ));
}

#[test]
fn drops_trailing_layout_blanks_and_rejects_delete_body() {
    let section = parse("SWAP 2.=2:\nfoo\n\nSWAP 4.=4:\nbaz").unwrap();

    assert!(matches!(
        &section.ops[0],
        HashlineOp::Swap { lines, .. } if lines == &vec!["foo".to_owned()]
    ));
    assert!(parse("DEL 2\n+X").is_err());
}

#[test]
fn empty_swap_becomes_delete_and_bare_body_is_normalized() {
    assert_eq!(
        parse("SWAP 2.=2:").unwrap().ops[0],
        HashlineOp::Delete { start: 2, end: 2 }
    );
    let section = parse("SWAP 2.=2:\n2:replacement").unwrap();
    assert_eq!(
        section.ops[0],
        HashlineOp::Swap {
            start: 2,
            end: 2,
            lines: vec!["replacement".to_owned()]
        }
    );
    assert_eq!(section.warnings, vec![BARE_BODY_WARNING]);
}

#[test]
fn accepts_bare_body_lines_starting_with_dash() {
    let section = parse("SWAP 2.=2:\n- #[serde(default)]").expect("字面量负号行应解析");

    assert_eq!(
        section.ops[0],
        HashlineOp::Swap {
            start: 2,
            end: 2,
            lines: vec!["- #[serde(default)]".to_owned()],
        }
    );
    assert_eq!(section.warnings, vec![BARE_BODY_WARNING]);
}

#[test]
fn bracket_literal_in_pending_body_is_not_treated_as_section_header() {
    let section = parse("SWAP 2.=2:\n[item, other]").unwrap();

    assert_eq!(
        section.ops[0],
        HashlineOp::Swap {
            start: 2,
            end: 2,
            lines: vec!["[item, other]".to_owned()],
        }
    );
    assert!(parse_sections("[src/a.rs#AB1234]\nSWAP 1:\n+x\n[src/b.rs#BAD]\nDEL 1").is_err());
}

#[test]
fn tolerates_diff_separators_and_missing_op_header() {
    // 带有 <<<< 分隔符以及 1: 2: 行号的输入
    let input = "[scan_cfg.py#12D0AB]\n<<<<\n1:import os\n2:import re";
    let sections = parse_sections(input).expect("应成功解析");
    assert_eq!(sections.len(), 1);
    assert_eq!(
        sections[0].ops[0],
        HashlineOp::InsertTail {
            lines: vec!["import os".to_owned(), "import re".to_owned()]
        }
    );

    // 没有操作指令直接提供 + 行
    let plus_input = "[scan_cfg.py#12D0AB]\n+import os\n+import re";
    let sections_plus = parse_sections(plus_input).expect("应成功解析");
    assert_eq!(
        sections_plus[0].ops[0],
        HashlineOp::InsertTail {
            lines: vec!["import os".to_owned(), "import re".to_owned()]
        }
    );
}

#[test]
fn tag_length_is_six_hex_digits_and_round_trips_through_parser() {
    // 生成端与解析端必须自洽：解析器接受的正是生成器产出的标签。
    // 只断言常量会漏掉两端脱节，因此这里走一次完整往返。
    assert_eq!(super::HASHLINE_TAG_LEN, 6);
    let tag = crate::hashline_snapshot_tag("alpha\nbeta\n");
    assert_eq!(tag.len(), super::HASHLINE_TAG_LEN);
    assert!(tag.chars().all(|ch| ch.is_ascii_hexdigit()));

    let sections = parse_sections(&format!("[src/a.rs#{tag}]\nDEL 1")).expect("自产标签必须可解析");
    assert_eq!(sections[0].tag, tag);
}

#[test]
fn rejects_legacy_tag_length_with_migration_hint() {
    // 旧格式标签必然来自过期快照：必须**拒绝**（INV-5 无宽容解析），
    // 但文案要给模型一个可直接执行的动作，而不是让它反复猜位数。
    let error = parse_sections("[src/a.rs#AB12]\nDEL 1").expect_err("旧格式标签必须拒绝");
    assert!(error.contains("旧格式"), "应为专用迁移提示，实际：{error}");
    assert!(
        error.contains("builtin.read_file"),
        "提示应给出可执行动作，实际：{error}"
    );
}

#[test]
fn rejects_tag_widths_other_than_six_and_non_hex() {
    for tag in ["A1B2C", "A1B2C3D", "A1B2CG"] {
        let error =
            parse_sections(&format!("[src/a.rs#{tag}]\nDEL 1")).expect_err("非法标签必须拒绝");
        assert!(error.contains("无效"), "标签 {tag} 应报无效，实际：{error}");
        assert!(
            !error.contains("旧格式"),
            "标签 {tag} 并非旧格式，不应触发迁移提示，实际：{error}"
        );
    }
}
