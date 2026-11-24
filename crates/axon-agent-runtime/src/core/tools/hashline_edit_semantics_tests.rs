use super::*;

fn strings(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

#[test]
fn drops_root_tsx_duplicated_closing_block() {
    let source = strings(&[
        "export const Root = () => {",
        "\treturn (",
        "\t\t<>",
        "\t\t\t<Composition />",
        "\t\t</>",
        "\t);",
        "};",
    ]);
    let ops = vec![HashlineOp::Swap {
        start: 2,
        end: 4,
        lines: strings(&["\treturn (", "\t\t<>", "\t\t\t<New />", "\t\t</>", "\t);"]),
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    let HashlineOp::Swap { lines, .. } = &ops[0] else {
        panic!("应保持 SWAP");
    };
    assert_eq!(lines, &strings(&["\treturn (", "\t\t<>", "\t\t\t<New />"]));
    assert_eq!(warnings.len(), 1);
}

/// 修复类 warning 必须带统一前缀，回执层才能据此把它与普通 warning 区分开。
///
/// 这是「静默修复」防线的前半段：没有可识别的标记，回执层就无法要求模型复核。
#[test]
fn repair_warnings_carry_the_shared_prefix() {
    let source = strings(&[
        "export const Root = () => {",
        "\treturn (",
        "\t\t<>",
        "\t\t\t<Composition />",
        "\t\t</>",
        "\t);",
        "};",
    ]);
    let ops = vec![HashlineOp::Swap {
        start: 2,
        end: 4,
        lines: strings(&["\treturn (", "\t\t<>", "\t\t\t<New />", "\t\t</>", "\t);"]),
    }];

    let (_, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].starts_with(HASHLINE_REPAIR_WARNING_PREFIX),
        "修复 warning 必须以前缀开头，实际为 {warnings:?}"
    );
    assert!(
        !HASHLINE_REPAIR_WARNING_PREFIX.is_empty(),
        "前缀不得为空，否则检测会退化成「任何 warning 都算修复」"
    );
}

#[test]
fn keeps_nested_jsx_closer_that_matches_surviving_parent() {
    let source = strings(&["const view = (", "  <section>", "old", "  </section>", ");"]);
    let ops = vec![HashlineOp::Swap {
        start: 3,
        end: 3,
        lines: strings(&["    <section>", "    new", "    </section>"]),
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    let HashlineOp::Swap { lines, .. } = &ops[0] else {
        panic!("应保持 SWAP");
    };
    assert_eq!(lines.len(), 3);
    assert!(warnings.is_empty());
}

#[test]
fn keeps_missing_closer_when_body_claims_block_interior() {
    let source = strings(&["if (x) {", "    old();", "}", "after();"]);
    let ops = vec![HashlineOp::Swap {
        start: 3,
        end: 3,
        lines: strings(&["    new();"]),
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert_eq!(
        ops,
        vec![HashlineOp::InsertBefore {
            line: 3,
            lines: strings(&["    new();"]),
        }]
    );
    assert_eq!(warnings.len(), 1);
}

#[test]
fn does_not_keep_closer_when_another_hunk_removes_its_opener() {
    let source = strings(&["if (x) {", "    old();", "}", "after();"]);
    let ops = vec![
        HashlineOp::Delete { start: 1, end: 1 },
        HashlineOp::Swap {
            start: 2,
            end: 3,
            lines: strings(&["new();"]),
        },
    ];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[1], HashlineOp::Swap { end: 3, .. }));
    assert!(warnings.is_empty());
}

#[test]
fn spends_residual_on_genuine_missing_closer() {
    let source = strings(&[
        "if enabled {",
        "    old();",
        "}",
        "const config = {",
        "    a: 1,",
        "};",
    ]);
    let ops = vec![
        HashlineOp::Delete { start: 1, end: 1 },
        HashlineOp::Swap {
            start: 2,
            end: 3,
            lines: strings(&["new();"]),
        },
        HashlineOp::Swap {
            start: 6,
            end: 6,
            lines: strings(&["    b: 2,"]),
        },
    ];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[1], HashlineOp::Swap { end: 3, .. }));
    assert!(matches!(ops[2], HashlineOp::InsertBefore { line: 6, .. }));
    assert_eq!(warnings.len(), 1);
}

#[test]
fn keeps_outer_closer_not_restated_by_payload() {
    let source = strings(&["class C {", "    old();", "    }", "}"]);
    let ops = vec![HashlineOp::Swap {
        start: 2,
        end: 4,
        lines: strings(&["    new_method() {", "        return 1;", "    }"]),
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert_eq!(
        ops,
        vec![HashlineOp::Swap {
            start: 2,
            end: 3,
            lines: strings(&["    new_method() {", "        return 1;", "    }"]),
        }]
    );
    assert_eq!(warnings.len(), 1);
}

#[test]
fn surviving_closer_below_range_covers_deleted_tail() {
    let source = strings(&[
        "class C {",
        "    method() {",
        "        old();",
        "    }",
        "}",
        "}",
    ]);
    let ops = vec![HashlineOp::Swap {
        start: 2,
        end: 5,
        lines: strings(&["    method() {", "        new();", "    }"]),
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert_eq!(
        ops,
        vec![HashlineOp::Swap {
            start: 2,
            end: 5,
            lines: strings(&["    method() {", "        new();", "    }"]),
        }]
    );
    assert!(warnings.is_empty());
}

#[test]
fn same_line_inserted_opener_supports_closer_repair() {
    let source = strings(&["    old();", "}"]);
    let ops = vec![
        HashlineOp::InsertBefore {
            line: 1,
            lines: strings(&["if (a) {"]),
        },
        HashlineOp::Swap {
            start: 1,
            end: 2,
            lines: strings(&["    new();"]),
        },
    ];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert_eq!(
        ops,
        vec![
            HashlineOp::InsertBefore {
                line: 1,
                lines: strings(&["if (a) {"]),
            },
            HashlineOp::Swap {
                start: 1,
                end: 1,
                lines: strings(&["    new();"]),
            },
        ]
    );
    assert_eq!(warnings.len(), 1);
}

#[test]
fn ambiguous_short_one_sided_echo_is_rejected() {
    let source = strings(&["before();", "old_a();", "old_b();", "keeper();"]);
    let ops = vec![HashlineOp::Swap {
        start: 2,
        end: 3,
        lines: strings(&["new();", "keeper();"]),
    }];

    let error = normalize_edit_semantics(&source, ops).unwrap_err();

    assert!(error.contains("不足以覆盖扩展后的范围"));
}

#[test]
fn duplicated_balance_neutral_statement_stays_literal() {
    let source = strings(&["const x = 1;", "old();", "const y = 2;"]);
    let ops = vec![HashlineOp::Swap {
        start: 2,
        end: 2,
        lines: strings(&["new();", "const y = 2;"]),
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    let HashlineOp::Swap { lines, .. } = &ops[0] else {
        panic!("应保持 SWAP");
    };
    assert_eq!(lines, &strings(&["new();", "const y = 2;"]));
    assert!(warnings.is_empty());
}

#[test]
fn separately_inserted_closer_covers_first_hunk_only() {
    let source = strings(&[
        "class C {",
        "    old();",
        "}",
        "after();",
        "const obj = {",
        "    a: 1,",
        "};",
    ]);
    let ops = vec![
        HashlineOp::Swap {
            start: 2,
            end: 3,
            lines: strings(&["    new();"]),
        },
        HashlineOp::InsertBefore {
            line: 4,
            lines: strings(&["}"]),
        },
        HashlineOp::Swap {
            start: 7,
            end: 7,
            lines: strings(&["    b: 2,"]),
        },
    ];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[0], HashlineOp::Swap { end: 3, .. }));
    assert!(matches!(ops[2], HashlineOp::InsertBefore { line: 7, .. }));
    assert_eq!(warnings.len(), 1);
}

#[test]
fn earlier_kept_closer_does_not_cover_later_orphan() {
    let source = strings(&["if (a) {", "    old();", "}", "}"]);
    let ops = vec![
        HashlineOp::Swap {
            start: 2,
            end: 3,
            lines: strings(&["    new();"]),
        },
        HashlineOp::Swap {
            start: 4,
            end: 4,
            lines: strings(&["after();"]),
        },
    ];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[0], HashlineOp::Swap { end: 2, .. }));
    assert!(matches!(ops[1], HashlineOp::Swap { end: 4, .. }));
    assert_eq!(warnings.len(), 1);
}

#[test]
fn jsx_closer_detection_rejects_malformed_tag_shape() {
    assert!(is_structural_closer("  </Section>;"));
    assert!(is_structural_closer("  </>;"));
    assert!(!is_structural_closer("  </ bad >"));
}

#[test]
fn stops_outward_shift_after_last_compatible_closer() {
    let source = strings(&[
        "function f() {",
        "    if (x) {",
        "        for (y) {",
        "            a();",
        "        }",
        "    }",
        "}",
    ]);
    let ops = vec![HashlineOp::InsertAfter {
        line: 4,
        lines: strings(&["    c();"]),
        block_start: None,
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[0], HashlineOp::InsertAfter { line: 6, .. }));
    assert_eq!(warnings.len(), 1);
}

#[test]
fn shifts_plain_after_insert_outward() {
    let source = strings(&[
        "function f() {",
        "    if (x) {",
        "        a();",
        "    }",
        "    b();",
        "}",
    ]);
    let ops = vec![HashlineOp::InsertAfter {
        line: 3,
        lines: strings(&["    c();"]),
        block_start: None,
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[0], HashlineOp::InsertAfter { line: 4, .. }));
    assert_eq!(warnings.len(), 1);
}

#[test]
fn shifts_block_after_insert_inward() {
    let source = strings(&[
        "function f() {",
        "    afterEach(() => {",
        "        destroy();",
        "    });",
        "}",
    ]);
    let ops = vec![HashlineOp::InsertAfter {
        line: 4,
        lines: strings(&["        setup();"]),
        block_start: Some(2),
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[0], HashlineOp::InsertAfter { line: 3, .. }));
    assert_eq!(warnings.len(), 1);
}

#[test]
fn blank_anchor_does_not_shift_after_insert() {
    let source = strings(&["function f() {", "    ", "}", "next();"]);
    let ops = vec![HashlineOp::InsertAfter {
        line: 2,
        lines: strings(&["nextSibling();"]),
        block_start: None,
    }];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(ops[0], HashlineOp::InsertAfter { line: 2, .. }));
    assert!(warnings.is_empty());
}

#[test]
fn unterminated_template_does_not_mask_later_missing_closer() {
    let source = strings(&[
        "const log = makeLog(`",
        "prefix",
        "`);",
        "const obj = {",
        "    a: 1",
        "};",
    ]);
    let ops = vec![
        HashlineOp::Swap {
            start: 1,
            end: 1,
            lines: strings(&["const log = createLog(`"]),
        },
        HashlineOp::Swap {
            start: 5,
            end: 6,
            lines: strings(&["    a: 2"]),
        },
    ];

    let (ops, warnings) = normalize_edit_semantics(&source, ops).unwrap();

    assert!(matches!(
        ops[0],
        HashlineOp::Swap {
            start: 1,
            end: 1,
            ..
        }
    ));
    assert!(matches!(
        ops[1],
        HashlineOp::Swap {
            start: 5,
            end: 5,
            ..
        }
    ));
    assert_eq!(warnings.len(), 1);
}

/// 规则表顺序即优先级，必须与重构前的 if/else 判据链逐位一致。
///
/// 这是本 WP 的核心守护：判据链改为数据后，顺序不再由代码位置表达，
/// 任何重排都会静默改变修复语义，只有本测试能拦住。
#[test]
fn boundary_repair_rule_priority_is_locked() {
    let actual = BOUNDARY_REPAIR_RULES
        .iter()
        .map(|rule| rule.kind)
        .collect::<Vec<_>>();

    assert_eq!(
        actual,
        vec![
            BoundaryRepairKind::BothSidesEcho,
            BoundaryRepairKind::ZeroDeltaOneSidedEcho,
            BoundaryRepairKind::BalancesDuplicateSuffix,
            BoundaryRepairKind::BalancesDuplicatePrefix,
        ],
        "规则表顺序即优先级，不得重排"
    );
}

/// 每条规则都必须可区分，避免两条规则被写成同一个判据。
#[test]
fn boundary_repair_rules_have_distinct_kinds() {
    let mut kinds = BOUNDARY_REPAIR_RULES
        .iter()
        .map(|rule| rule.kind)
        .collect::<Vec<_>>();
    let total = kinds.len();
    kinds.sort_by_key(|kind| format!("{kind:?}"));
    kinds.dedup();

    assert_eq!(kinds.len(), total, "规则表中不得出现重复的规则标识");
}
