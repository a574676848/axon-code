use axon_domain::{
    TokenBudget, ToolDefinition, ToolId, ToolInput, ToolOutput, ToolOutputPolicy,
    ToolPermissionPolicy, ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};

use super::{
    ContentArchiveRuntime, ContextBudgetEvent, ContextBudgetRequest, ContextBudgetRuntime,
    ContextSectionDecision, ContextSectionInput, ContextSectionKind,
    DEFAULT_SUMMARY_TOKEN_ESTIMATE, HashlineApplyFile, HashlineApplyRequest, OverflowAction,
    ToolRegistry, WorkspaceFileSession, apply_hashline_edit,
};

#[test]
fn hashline_edit_preflights_multiple_files() {
    let first = observed_file("src/lib.rs", "alpha\nbeta\ngamma\n", [1, 2, 3]);
    let second = observed_file("src/main.rs", "one\ntwo\n", [1, 2]);
    let first_tag = first.snapshot.tag();
    let second_tag = second.snapshot.tag();
    let report = apply_hashline_edit(HashlineApplyRequest {
        input: format!(
            "[src/lib.rs#{first_tag}]\nSWAP 2.=2:\n+BETTER\n\
             [src/main.rs#{second_tag}]\nINS.POST 2:\n+three\n"
        ),
        files: vec![first, second],
    })
    .expect("hashline multi-file preflight");

    assert_eq!(report.files.len(), 2);
    assert_eq!(report.files[0].next_content, "alpha\nBETTER\ngamma\n");
    assert_eq!(report.files[1].next_content, "one\ntwo\nthree\n");
}

#[test]
fn hashline_edit_recovers_stale_anchor_after_unrelated_insert() {
    let mut file = observed_file("src/lib.rs", "alpha\nbeta\ngamma\n", [2]);
    let tag = file.snapshot.tag().to_owned();
    file.current_content = "intro\nalpha\nbeta\ngamma\n".to_owned();

    let report = apply_hashline_edit(HashlineApplyRequest {
        input: format!("[src/lib.rs#{tag}]\nSWAP 2.=2:\n+BETTER\n"),
        files: vec![file],
    })
    .expect("stale anchor should recover");

    assert_eq!(
        report.files[0].next_content,
        "intro\nalpha\nBETTER\ngamma\n"
    );
    assert_eq!(report.files[0].warnings.len(), 1);
}

#[test]
fn hashline_edit_rejects_unseen_anchor_and_unknown_tag() {
    let file = observed_file("src/lib.rs", "alpha\nbeta\n", [1]);
    let tag = file.snapshot.tag().to_owned();
    let unseen = apply_hashline_edit(HashlineApplyRequest {
        input: format!("[src/lib.rs#{tag}]\nDEL 2.=2\n"),
        files: vec![file.clone()],
    })
    .expect_err("unseen line must be rejected");
    let unknown = apply_hashline_edit(HashlineApplyRequest {
        input: "[src/lib.rs#FFFF12]\nDEL 1.=1\n".to_owned(),
        files: vec![file],
    })
    .expect_err("unknown tag must be rejected");

    assert!(unseen.contains("未完整读取"));
    assert!(unknown.contains("不属于当前会话"));
}

#[test]
fn hashline_edit_preserves_bom_crlf_and_trailing_newline() {
    let content = "\u{feff}alpha\r\nbeta\r\n";
    let file = observed_file("src/lib.rs", content, [1, 2]);
    let tag = file.snapshot.tag();

    let report = apply_hashline_edit(HashlineApplyRequest {
        input: format!("[src/lib.rs#{tag}]\nSWAP 2.=2:\n+BETTER\n"),
        files: vec![file],
    })
    .expect("formatted edit");

    assert_eq!(report.files[0].next_content, "\u{feff}alpha\r\nBETTER\r\n");
}

#[test]
fn context_budget_runtime_compresses_low_priority_and_keeps_refs() {
    let runtime = ContextBudgetRuntime;
    let plan = runtime.plan(ContextBudgetRequest {
        total_limit: TokenBudget::new(1_200).unwrap(),
        reserved_for_tools: TokenBudget::new(100).unwrap(),
        sections: vec![
            context_section(ContextSectionKind::History, "history", 600, 80, true, false),
            context_section(ContextSectionKind::Policy, "policy", 160, 0, false, false),
            context_section(
                ContextSectionKind::Selection,
                "selection",
                280,
                0,
                false,
                false,
            ),
            context_section(
                ContextSectionKind::Attachment,
                "image",
                900,
                DEFAULT_SUMMARY_TOKEN_ESTIMATE,
                false,
                true,
            ),
            context_section(
                ContextSectionKind::CommandOutput,
                "command-output",
                180,
                DEFAULT_SUMMARY_TOKEN_ESTIMATE,
                true,
                false,
            ),
        ],
    });

    assert_eq!(plan.available_for_context, 1_100);
    assert!(plan.overflowed);
    assert_eq!(plan.overflow_action, OverflowAction::StripMediaKeepRefs);
    assert!(!plan.summary_is_original_evidence);
    assert!(matches!(
        plan.sections
            .iter()
            .find(|section| section.label == "policy")
            .unwrap()
            .decision,
        ContextSectionDecision::IncludeOriginal
    ));
    assert!(matches!(
        plan.sections
            .iter()
            .find(|section| section.label == "selection")
            .unwrap()
            .decision,
        ContextSectionDecision::IncludeOriginal
    ));
    assert!(matches!(
        plan.sections
            .iter()
            .find(|section| section.label == "history")
            .unwrap()
            .decision,
        ContextSectionDecision::Summarize {
            original_evidence_allowed: false
        }
    ));
    assert!(matches!(
        plan.sections
            .iter()
            .find(|section| section.label == "image")
            .unwrap()
            .decision,
        ContextSectionDecision::KeepRef
    ));
    assert!(plan.events.iter().any(|event| matches!(
        event,
        ContextBudgetEvent::SectionSummarized { label } if label == "history"
    )));
}

#[test]
fn context_budget_runtime_charges_refs_and_drops_the_ref_that_cannot_fit() {
    let runtime = ContextBudgetRuntime;
    let plan = runtime.plan(ContextBudgetRequest {
        total_limit: TokenBudget::new(200).unwrap(),
        reserved_for_tools: TokenBudget::new(100).unwrap(),
        sections: vec![
            context_section(
                ContextSectionKind::Memory,
                "first-ref",
                200,
                60,
                false,
                true,
            ),
            context_section(
                ContextSectionKind::Memory,
                "second-ref",
                200,
                60,
                false,
                true,
            ),
        ],
    });

    let first = plan
        .sections
        .iter()
        .find(|section| section.label == "first-ref")
        .unwrap();
    let second = plan
        .sections
        .iter()
        .find(|section| section.label == "second-ref")
        .unwrap();

    assert_eq!(first.included_token_estimate, 60);
    assert_eq!(first.decision, ContextSectionDecision::KeepRef);
    assert_eq!(second.included_token_estimate, 0);
    assert_eq!(second.decision, ContextSectionDecision::Drop);
    assert_eq!(plan.overflow_action, OverflowAction::StripMediaKeepRefs);
    assert!(plan.events.iter().any(|event| matches!(
        event,
        ContextBudgetEvent::SectionDropped { label } if label == "second-ref"
    )));
}

#[test]
fn context_budget_runtime_blocks_when_protected_context_cannot_fit() {
    let runtime = ContextBudgetRuntime;
    let plan = runtime.plan(ContextBudgetRequest {
        total_limit: TokenBudget::new(200).unwrap(),
        reserved_for_tools: TokenBudget::new(50).unwrap(),
        sections: vec![context_section(
            ContextSectionKind::Policy,
            "policy",
            200,
            0,
            false,
            false,
        )],
    });

    assert_eq!(plan.overflow_action, OverflowAction::BlockUntilUserChooses);
    assert!(
        plan.events
            .iter()
            .any(|event| matches!(event, ContextBudgetEvent::UserChoiceRequired { .. }))
    );
}

#[test]
fn context_budget_receipts_are_one_to_one_with_sections() {
    let plan = ContextBudgetRuntime.plan(ContextBudgetRequest {
        total_limit: TokenBudget::new(500).unwrap(),
        reserved_for_tools: TokenBudget::new(50).unwrap(),
        sections: vec![
            context_section(ContextSectionKind::Policy, "policy", 100, 0, false, false),
            context_section(ContextSectionKind::Memory, "memory", 400, 40, true, false),
        ],
    });

    assert_eq!(plan.receipts.len(), plan.sections.len());
    for (receipt, section) in plan.receipts.iter().zip(&plan.sections) {
        assert_eq!(receipt.label, section.label);
        assert_eq!(receipt.decision, section.decision);
        assert_eq!(
            receipt.original_token_estimate,
            section.original_token_estimate
        );
        assert_eq!(
            receipt.included_token_estimate,
            section.included_token_estimate
        );
    }
}

#[test]
fn context_budget_receipts_serialize_without_event_log() {
    let plan = ContextBudgetRuntime.plan(ContextBudgetRequest {
        total_limit: TokenBudget::new(400).unwrap(),
        reserved_for_tools: TokenBudget::new(50).unwrap(),
        sections: vec![
            context_section(ContextSectionKind::Policy, "policy", 100, 0, false, false),
            context_section(
                ContextSectionKind::Attachment,
                "image",
                200,
                60,
                false,
                true,
            ),
        ],
    });

    let json = serde_json::to_value(&plan.receipts).unwrap();
    assert_eq!(plan.receipts.len(), 2);
    assert_eq!(json[1]["decision"], "keep_ref");
    assert!(json.get("events").is_none());
}

#[test]
fn tool_registry_exposes_enabled_schema_hash_and_output_ref() {
    let mut registry = ToolRegistry::new();
    registry
        .register(tool_definition(
            1,
            "builtin",
            "read",
            true,
            ToolOutputPolicy::BoundedSummary,
        ))
        .unwrap();
    registry
        .register(tool_definition(
            2,
            "external",
            "search",
            false,
            ToolOutputPolicy::BoundedSummary,
        ))
        .unwrap();
    registry
        .register(tool_definition(
            3,
            "builtin",
            "run",
            true,
            ToolOutputPolicy::RefOnOverflow,
        ))
        .unwrap();

    let model_tools = registry.model_tools();
    assert_eq!(model_tools.len(), 2);
    assert!(
        model_tools
            .iter()
            .any(|tool| tool.qualified_name == "builtin.read" && !tool.schema_hash.is_empty())
    );
    assert!(
        !model_tools
            .iter()
            .any(|tool| tool.qualified_name == "external.search")
    );

    assert!(
        registry
            .validate_input(
                "builtin",
                "read",
                &ToolInput {
                    fields: vec!["path".to_owned()],
                },
            )
            .is_ok()
    );
    assert!(
        registry
            .validate_input("builtin", "read", &ToolInput { fields: Vec::new() })
            .is_err()
    );

    let result = registry
        .materialize_output(
            "builtin",
            "run",
            &ToolOutput {
                text: "0123456789abcdef".to_owned(),
            },
            Some(8),
        )
        .expect_err("没有 archive root 时不应生成不可恢复的引用");
    assert!(format!("{result:?}").contains("RefOnOverflow"));

    let archive_root = axon_test_support::test_temp_root().join(format!(
        "axon-tool-registry-{}-{}",
        std::process::id(),
        "materialize"
    ));
    let result = registry
        .materialize_output_with_archive_root(
            "builtin",
            "run",
            &ToolOutput {
                text: "0123456789abcdef".to_owned(),
            },
            Some(8),
            Some(&archive_root),
        )
        .unwrap();

    assert!(result.metadata.truncated);
    assert_eq!(result.summary, "01234567");
    assert!(result.inline_output.is_none());
    let output_ref = result.output_ref.unwrap();
    assert_eq!(output_ref.byte_len, 16);
    assert_eq!(output_ref.preview, "01234567");
    assert_eq!(output_ref.source_trace, "tool://builtin/run");
    assert_eq!(output_ref.sha256, result.metadata.sha256);
    assert_eq!(result.metadata.sha256.len(), 64);
    assert_eq!(
        ContentArchiveRuntime::default()
            .read_output(&archive_root, &output_ref)
            .unwrap(),
        b"0123456789abcdef"
    );

    let unicode_result = registry
        .materialize_output_with_archive_root(
            "builtin",
            "run",
            &ToolOutput {
                text: "你好世界".to_owned(),
            },
            Some(5),
            Some(&archive_root),
        )
        .unwrap();
    assert_eq!(unicode_result.summary, "你");
    let _ = std::fs::remove_dir_all(archive_root);
}

fn context_section(
    kind: ContextSectionKind,
    label: &str,
    token_estimate: u64,
    summary_token_estimate: u64,
    compressible: bool,
    ref_only: bool,
) -> ContextSectionInput {
    ContextSectionInput {
        kind,
        label: label.to_owned(),
        token_estimate,
        summary_token_estimate,
        compressible,
        ref_only,
        source_ref: Some(format!("timeline://{label}")),
    }
}

fn tool_definition(
    id: u64,
    namespace: &str,
    name: &str,
    enabled: bool,
    output_policy: ToolOutputPolicy,
) -> ToolDefinition {
    ToolDefinition {
        id: ToolId(id),
        namespace: namespace.to_owned(),
        name: name.to_owned(),
        description: "稳定短描述".to_owned(),
        schema: ToolSchema::object(
            1,
            ["path"],
            vec![ToolSchemaField::new(
                "path",
                ToolSchemaFieldKind::String,
                "测试路径。",
            )],
        ),
        enabled,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        output_policy,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn observed_file<const N: usize>(
    path: &str,
    content: &str,
    seen_lines: [usize; N],
) -> HashlineApplyFile {
    let session = WorkspaceFileSession::default();
    let snapshot = session
        .record_read(path, content, seen_lines)
        .expect("snapshot should be recorded");
    HashlineApplyFile {
        path: path.to_owned(),
        current_content: content.to_owned(),
        snapshot,
    }
}
