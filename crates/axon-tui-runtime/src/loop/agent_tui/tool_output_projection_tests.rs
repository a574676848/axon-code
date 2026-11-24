use super::super::run_output::{
    AgentTuiToolExecutionView, AgentTuiToolKind, AgentTuiToolLifecycleLine,
    AgentTuiToolLifecycleLineInput, AgentTuiToolOutputChunk, AgentTuiToolProgressView,
    AgentTuiToolReceiptDetails,
};
use super::{
    REDACTED_VALUE, TOOL_OUTPUT_CHUNK_MAX_CHARS, compact_tool_output_chunk,
    tool_execution_started_line, tool_kind_action, tool_lifecycle_line, tool_progress_preview,
    tool_result_line,
};
use crate::AgentTuiToolReceipt;
use crate::surface::cell::{RenderCellAction, RenderCellData};
use crate::surface::cell_event_data::AgentRenderLifecycleStatus;
use axon_domain::ToolExecutionPermission;

#[path = "tool_output_terminal_tests.rs"]
mod tool_output_terminal_tests;

#[test]
fn image_read_file_receipt_renders_as_read_row_with_path() {
    let receipt = tool_receipt(
        "call-image",
        "builtin.read_file",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::ReadFile,
            status: "completed".to_owned(),
            summary: "read_file path=shot.png content_kind=image mime=image/png bytes=68\n<path>shot.png</path>\n<type>image</type>\n<content>\nimage/png，68 字节；图片已随本条工具结果附上，请直接依据图片像素判断。\n</content>".to_owned(),
            metadata: Some(serde_json::json!({
                "kind": "read_file",
                "contentKind": "image",
                "path": "shot.png",
                "mimeType": "image/png",
                "byteLen": 68,
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    );

    let line = tool_result_line(&receipt);

    let Some(RenderCellData::Exec(cell)) = line.data.as_ref() else {
        panic!("图片读取回执必须渲染为 exec 单元");
    };
    // 成功的读取保持安静成功：只显示动作与路径，不把机器摘要灌进终端。
    assert_eq!(cell.arguments_preview.as_deref(), Some("shot.png"));
    assert!(cell.summary.is_none());
}

#[test]
fn failed_image_read_receipt_surfaces_the_actionable_message() {
    let receipt = tool_receipt(
        "call-image-failed",
        "builtin.read_file",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::ReadFile,
            status: "failed".to_owned(),
            summary: "当前模型未声明图片输入能力，图片无法交给模型判断".to_owned(),
            metadata: Some(serde_json::json!({ "path": "shot.png" })),
            error_type: Some("execution_failed".to_owned()),
            filesystem_write_performed: false,
        },
    );

    let line = tool_result_line(&receipt);

    let Some(RenderCellData::Exec(cell)) = line.data.as_ref() else {
        panic!("失败的图片读取必须渲染为 exec 单元");
    };
    assert!(
        cell.summary
            .as_deref()
            .is_some_and(|summary| summary.contains("图片输入能力")),
        "失败原因必须对用户可见，实际：{:?}",
        cell.summary
    );
}

#[test]
fn tool_started_preview_redacts_bare_token_values() {
    let view = AgentTuiToolExecutionView {
        part_id: "tool-part:test:fixture".to_owned(),
        request_id: 8,
        provider_call_id: "call-token".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        kind: "shell".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        arguments_preview: "command=curl token=abc123 url=https://example.test".to_owned(),
        intent: None,
        execution_permission: ToolExecutionPermission::Allowed,
    };

    let line = tool_execution_started_line(&view);

    assert_eq!(line.action, RenderCellAction::Running);
    assert!(!line.text.contains("abc123"));
    let arguments = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => cell
            .arguments_preview
            .as_deref()
            .expect("tool start should expose sanitized arguments"),
        _ => panic!("tool start should render as typed exec data"),
    };
    assert!(arguments.contains("token=[已隐藏]"));
    assert!(arguments.contains("https://example.test"));
}

#[test]
fn output_chunk_preview_redacts_authorization_and_truncates() {
    let long_tail = "x".repeat(TOOL_OUTPUT_CHUNK_MAX_CHARS + 64);
    let chunk = AgentTuiToolOutputChunk {
        part_id: "tool-part:test:fixture".to_owned(),
        provider_call_id: "call-output".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        stream: "stdout".to_owned(),
        sequence: 1,
        text: format!("Authorization: Bearer secret-token\nstdout={long_tail}"),
    };

    let text = compact_tool_output_chunk(&chunk.text);

    assert!(!text.contains("secret-token"));
    assert!(text.contains(REDACTED_VALUE));
    assert!(text.contains("已截断"));
}

#[test]
fn output_chunk_preview_keeps_line_budget_like_exec_cell() {
    let text =
        compact_tool_output_chunk("line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8");

    assert!(text.contains("line 1"));
    assert!(text.contains("line 2"));
    assert!(text.contains("… +4 lines"));
    assert!(text.contains("line 7"));
    assert!(text.contains("line 8"));
    assert!(!text.contains("line 3"));
    assert!(!text.contains("line 4"));
    assert!(!text.contains("line 5"));
    assert!(!text.contains("line 6"));
    assert_eq!(text.lines().count(), 5);
}

#[test]
fn tool_progress_preview_keeps_completed_count_without_total() {
    let progress = AgentTuiToolProgressView {
        part_id: "tool-part:test:fixture".to_owned(),
        provider_call_id: "call-search".to_owned(),
        tool_name: "builtin.search".to_owned(),
        semantic_kind: AgentTuiToolKind::Search,
        phase: "searching".to_owned(),
        items_completed: Some(2),
        items_total: None,
        current_item: Some("local".to_owned()),
        elapsed_ms: Some(2100),
        message: Some("返回 2 条结果".to_owned()),
    };

    let preview = tool_progress_preview(&progress);

    assert!(preview.contains("searching"));
    assert!(preview.contains("2 completed"));
}

#[test]
fn tool_progress_preview_omits_zero_elapsed_noise() {
    let progress = AgentTuiToolProgressView {
        part_id: "tool-part:test:fixture".to_owned(),
        provider_call_id: "call-web".to_owned(),
        tool_name: "builtin.search".to_owned(),
        semantic_kind: AgentTuiToolKind::Search,
        phase: "batch_running".to_owned(),
        items_completed: Some(1),
        items_total: Some(2),
        current_item: Some("builtin.search".to_owned()),
        elapsed_ms: Some(0),
        message: Some("并行工具批次执行中".to_owned()),
    };

    let preview = tool_progress_preview(&progress);

    assert!(preview.contains("batch_running"));
    assert!(preview.contains("1/2"));
    assert!(preview.contains("当前 builtin.search"));
    assert!(preview.contains("并行工具批次执行中"));
    assert!(!preview.contains("0ms"));
}

#[test]
fn tool_kind_action_matches_codex_style_actions() {
    let cases = [
        (AgentTuiToolKind::ReadFile, RenderCellAction::Read),
        (AgentTuiToolKind::Glob, RenderCellAction::List),
        (AgentTuiToolKind::Search, RenderCellAction::Search),
        (AgentTuiToolKind::Edit, RenderCellAction::Patched),
        (AgentTuiToolKind::Mcp, RenderCellAction::Mcp),
        (AgentTuiToolKind::Shell, RenderCellAction::Run),
    ];

    for (kind, expected) in cases {
        assert_eq!(tool_kind_action(kind), expected);
    }
}

#[test]
fn builtin_tools_project_codex_style_display_labels_without_raw_main_title() {
    let cases = [
        (
            "builtin_memory_search",
            AgentTuiToolKind::MemorySearch,
            "记忆检索",
            Some("记忆"),
        ),
        (
            "builtin.read_file",
            AgentTuiToolKind::ReadFile,
            "读取",
            None,
        ),
        ("builtin.edit", AgentTuiToolKind::Edit, "编辑", None),
        (
            "builtin.shell_command",
            AgentTuiToolKind::Shell,
            "执行",
            None,
        ),
    ];

    for (tool_name, kind, expected_label, expected_target) in cases {
        let line = tool_result_line(&tool_receipt(
            "call-display",
            tool_name,
            AgentTuiToolReceiptDetails {
                kind: kind.to_owned(),
                status: "completed".to_owned(),
                summary: "ok".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
            },
        ));
        let Some(RenderCellData::Exec(cell)) = line.data.as_ref() else {
            panic!("builtin tool should render as exec cell");
        };

        assert_eq!(line.text, expected_label);
        assert_eq!(cell.display_label, expected_label);
        assert_eq!(cell.display_target.as_deref(), expected_target);
        assert_eq!(cell.raw_tool_name, tool_name);
        assert!(!line.text.contains("builtin_"));
        assert!(!line.text.contains("builtin."));
    }
}

#[test]
fn agent_tui_tool_kind_display_labels_are_chinese() {
    let cases = [
        (AgentTuiToolKind::ReadFile, "读取"),
        (AgentTuiToolKind::Glob, "匹配"),
        (AgentTuiToolKind::Search, "检索"),
        (AgentTuiToolKind::MemorySearch, "记忆检索"),
        (AgentTuiToolKind::MemoryWrite, "记忆写入"),
        (AgentTuiToolKind::Edit, "编辑"),
        (AgentTuiToolKind::Shell, "执行"),
        (AgentTuiToolKind::Mcp, "MCP"),
        (AgentTuiToolKind::InvokeSkill, "技能"),
    ];

    for (kind, expected) in cases {
        assert_eq!(kind.display_label(), expected);
    }
}

#[test]
fn mcp_tool_receipt_projects_fixed_label_and_server_tool_target() {
    let line = tool_result_line(&tool_receipt(
        "call-mcp",
        "mcp__gitnexus__query",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Mcp.to_owned(),
            status: "completed".to_owned(),
            summary: "query ok".to_owned(),
            metadata: Some(serde_json::json!({
                "server": "gitnexus",
                "tool": "query"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    assert_eq!(line.text, "MCP");
    let Some(RenderCellData::Mcp(cell)) = line.data.as_ref() else {
        panic!("mcp tool should render as typed MCP data");
    };
    let item = &cell.items[0];
    assert_eq!(item.display_label, "MCP");
    assert_eq!(item.display_target.as_deref(), Some("gitnexus.query"));
    assert_eq!(item.raw_tool_name, "mcp__gitnexus__query");
    assert!(!line.text.contains("mcp__gitnexus__query"));
}

#[test]
fn mcp_metadata_projects_structured_server_tool_target() {
    let line = tool_result_line(&tool_receipt(
        "call-mcp-metadata",
        "mcp__knowledge__lookup",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Mcp.to_owned(),
            status: "completed".to_owned(),
            summary: "query ok".to_owned(),
            metadata: Some(serde_json::json!({
                "server": "knowledge",
                "tool": "lookup"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    let Some(RenderCellData::Mcp(cell)) = line.data.as_ref() else {
        panic!("mcp tool should render as typed MCP data");
    };
    let item = &cell.items[0];
    assert_eq!(item.display_label, "MCP");
    assert_eq!(item.display_target.as_deref(), Some("knowledge.lookup"));
    assert_eq!(item.raw_tool_name, "mcp__knowledge__lookup");
}

#[test]
fn completed_shell_and_search_receipts_use_terminal_action_labels() {
    let shell = tool_result_line(&tool_receipt(
        "call-shell",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "completed".to_owned(),
            summary: "ok".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    ));
    let search = tool_result_line(&tool_receipt(
        "call-search",
        "search",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Search.to_owned(),
            status: "completed".to_owned(),
            summary: "ok".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    ));
    let queued = tool_result_line(&tool_receipt(
        "call-wait",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "queued".to_owned(),
            summary: "wait".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    ));
    let failed = tool_result_line(&tool_receipt(
        "call-failed",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "failed".to_owned(),
            summary: "permission denied".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    assert_eq!(shell.action, RenderCellAction::Ran);
    assert_eq!(search.action, RenderCellAction::Searched);
    assert_eq!(queued.action, RenderCellAction::Waiting);
    assert_eq!(failed.action, RenderCellAction::Error);
}

#[test]
fn machine_tool_summary_does_not_render_as_exec_result_detail() {
    let line = tool_result_line(&tool_receipt(
        "call-shell",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "completed".to_owned(),
            summary: "shell command=cargo check exit_code=0 timed_out=false output_handler=compact full_output_path=none stdout=ok stderr=".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    let summary = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => cell.summary.as_deref(),
        _ => panic!("shell receipt should render as typed exec data"),
    };

    assert_eq!(summary, None);
}

#[test]
fn machine_metadata_summary_does_not_render_as_exec_result_detail() {
    let line = tool_result_line(&tool_receipt(
        "call-shell",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "failed".to_owned(),
            summary: "shell command=powershell -Command New-Item exit_code=1 timed_out=false stdout= stderr=error".to_owned(),
            metadata: Some(serde_json::json!({
                "summary": "shell command=powershell -Command New-Item exit_code=1 timed_out=false stdout= stderr=error",
                "argumentsPreview": "powershell -Command New-Item"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    let summary = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => cell.summary.as_deref(),
        _ => panic!("shell receipt should render as typed exec data"),
    };

    assert_eq!(summary, None);
}

#[test]
fn successful_structured_tool_summary_stays_out_of_default_exec_body() {
    let line = tool_result_line(&tool_receipt(
        "call-shell",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "completed".to_owned(),
            summary: "shell command=cargo check exit_code=0 timed_out=false stdout=ok stderr="
                .to_owned(),
            metadata: Some(serde_json::json!({
                "summary": "cargo check · 通过",
                "argumentsPreview": "cargo check"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    let (summary, arguments) = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => {
            (cell.summary.as_deref(), cell.arguments_preview.as_deref())
        }
        _ => panic!("shell receipt should render as typed exec data"),
    };

    assert_eq!(summary, None);
    assert_eq!(arguments, Some("cargo check"));
}

#[test]
fn failed_structured_tool_summary_stays_visible() {
    let line = tool_result_line(&tool_receipt(
        "call-shell",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "failed".to_owned(),
            summary: "shell command=cargo test elapsed_ms=65000 exit_code=1 timed_out=false stdout=fail stderr=fail".to_owned(),
            metadata: Some(serde_json::json!({
                "summary": "cargo test · 失败 · duration_ms=65000",
                "argumentsPreview": "cargo test"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    let summary = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => cell.summary.as_deref(),
        _ => panic!("shell receipt should render as typed exec data"),
    };

    assert_eq!(summary, Some("cargo test · 失败 · duration_ms=1m 05s"));
}

#[test]
fn structured_tool_output_preview_uses_readable_units() {
    let line = tool_result_line(&tool_receipt(
        "call-shell",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell.to_owned(),
            status: "failed".to_owned(),
            summary: "shell command=cargo test exit_code=1 timed_out=false stdout=fail stderr=fail"
                .to_owned(),
            metadata: Some(serde_json::json!({
                "summary": "cargo test · 失败",
                "argumentsPreview": "cargo test",
                "outputPreview": "elapsed_ms=65000 ctx:5.1k/1000k"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    ));

    let output = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => cell.output_preview.as_deref(),
        _ => panic!("shell receipt should render as typed exec data"),
    };

    assert_eq!(output, Some("elapsed_ms=1m 05s ctx:5.1K/1M"));
}

#[test]
fn successful_tool_lifecycle_summary_stays_out_of_default_exec_body() {
    let line = tool_lifecycle_line(&tool_lifecycle(
        "call-shell",
        "shell_command",
        AgentTuiToolKind::Shell,
        "completed",
        "cargo check · 通过",
        Some("cargo check".to_owned()),
        None,
    ));

    let (summary, arguments) = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => {
            (cell.summary.as_deref(), cell.arguments_preview.as_deref())
        }
        _ => panic!("shell lifecycle should render as typed exec data"),
    };

    assert_eq!(summary, None);
    assert_eq!(arguments, Some("cargo check"));
}

#[test]
fn failed_tool_lifecycle_summary_stays_visible() {
    let line = tool_lifecycle_line(&tool_lifecycle(
        "call-shell",
        "shell_command",
        AgentTuiToolKind::Shell,
        "failed",
        "cargo test · 失败",
        Some("cargo test".to_owned()),
        None,
    ));

    let summary = match line.data.as_ref() {
        Some(RenderCellData::Exec(cell)) => cell.summary.as_deref(),
        _ => panic!("shell lifecycle should render as typed exec data"),
    };

    assert_eq!(summary, Some("cargo test · 失败"));
}

#[test]
fn mcp_tool_receipt_uses_typed_mcp_cell() {
    let receipt = tool_receipt(
        "call-mcp",
        "mcp__gitnexus__query",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Mcp.to_owned(),
            status: "completed".to_owned(),
            summary: "query ok".to_owned(),
            metadata: Some(serde_json::json!({
                "server": "gitnexus",
                "tool": "query",
                "argumentsPreview": r#"{"q":"auth"}"#,
                "outputPreview": "2 条结果"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    );

    let line = tool_result_line(&receipt);

    assert_eq!(line.action, RenderCellAction::Mcp);
    match line.data.as_ref() {
        Some(RenderCellData::Mcp(cell)) => {
            assert_eq!(cell.title, "MCP");
            assert_eq!(cell.items.len(), 1);
            assert_eq!(cell.items[0].provider_call_id.as_deref(), Some("call-mcp"));
            assert_eq!(cell.items[0].server, "gitnexus");
            assert_eq!(cell.items[0].tool.as_deref(), Some("query"));
            assert_eq!(
                cell.items[0].arguments_preview.as_deref(),
                Some(r#"{"q":"auth"}"#)
            );
            assert_eq!(cell.items[0].output_preview.as_deref(), Some("2 条结果"));
            assert_eq!(cell.items[0].summary, None);
        }
        _ => panic!("mcp receipt should render as typed MCP data"),
    }
}

fn tool_receipt(
    provider_call_id: &str,
    tool_name: &str,
    details: crate::r#loop::agent_tui::run_output::AgentTuiToolReceiptDetails,
) -> AgentTuiToolReceipt {
    AgentTuiToolReceipt::with_id(
        "tool-part:test:tool-receipt",
        provider_call_id,
        tool_name,
        details,
    )
}

fn tool_lifecycle(
    provider_call_id: &str,
    tool_name: &str,
    kind: AgentTuiToolKind,
    status: &str,
    summary: &str,
    arguments_preview: Option<String>,
    output_preview: Option<String>,
) -> AgentTuiToolLifecycleLine {
    AgentTuiToolLifecycleLine::with_id(AgentTuiToolLifecycleLineInput {
        id: "tool-lifecycle:test:tool".to_owned(),
        tool_part_id: "tool-part:test:tool-lifecycle".to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        tool_name: tool_name.to_owned(),
        kind,
        status: status.to_owned(),
        summary: summary.to_owned(),
        arguments_preview,
        output_preview,
    })
}

#[test]
fn mcp_tool_started_uses_typed_mcp_cell() {
    let view = AgentTuiToolExecutionView {
        part_id: "tool-part:test:fixture".to_owned(),
        request_id: 9,
        provider_call_id: "call-mcp".to_owned(),
        tool_name: "mcp__gitnexus__query".to_owned(),
        kind: "mcp".to_owned(),
        semantic_kind: AgentTuiToolKind::Mcp,
        arguments_preview: r#"{"q":"auth"}"#.to_owned(),
        intent: None,
        execution_permission: ToolExecutionPermission::Allowed,
    };

    let line = tool_execution_started_line(&view);

    assert_eq!(line.action, RenderCellAction::Mcp);
    match line.data.as_ref() {
        Some(RenderCellData::Mcp(cell)) => {
            assert_eq!(cell.items.len(), 1);
            assert_eq!(cell.items[0].provider_call_id.as_deref(), Some("call-mcp"));
            assert_eq!(cell.items[0].server, "gitnexus");
            assert_eq!(cell.items[0].tool.as_deref(), Some("query"));
            assert_eq!(
                cell.items[0].arguments_preview.as_deref(),
                Some(r#"{"q":"auth"}"#)
            );
            assert_eq!(cell.items[0].status, AgentRenderLifecycleStatus::Running);
        }
        _ => panic!("mcp start should render as typed MCP data"),
    }
}
