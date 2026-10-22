use super::super::run_output::AgentTuiToolKind;
use super::tool_result_line;
use crate::AgentTuiToolReceipt;
use crate::surface::cell::RenderCellData;

#[test]
fn resumed_structured_tool_metadata_restores_argument_summary_without_raw_arguments() {
    let cases = [
        (
            "builtin.search",
            AgentTuiToolKind::Search,
            serde_json::json!({"kind": "search", "query": "error handling"}),
            "error handling",
        ),
        (
            "builtin.read_file",
            AgentTuiToolKind::ReadFile,
            serde_json::json!({"kind": "read_file", "path": "src/lib.rs"}),
            "src/lib.rs",
        ),
        (
            "builtin.glob",
            AgentTuiToolKind::Glob,
            serde_json::json!({"kind": "glob", "root": "crates"}),
            "crates",
        ),
        (
            "builtin.memory_write",
            AgentTuiToolKind::MemoryWrite,
            serde_json::json!({"kind": "memory_write", "path": "src/lib.rs"}),
            "src/lib.rs",
        ),
        (
            "builtin.edit",
            AgentTuiToolKind::Edit,
            serde_json::json!({
                "kind": "edit",
                "paths": ["src/main.rs", "src/lib.rs"]
            }),
            "src/main.rs",
        ),
    ];

    for (tool_name, kind, metadata, expected) in cases {
        let receipt = AgentTuiToolReceipt::with_id(
            "tool-part:test:resume-preview",
            "call-resume-preview",
            tool_name,
            crate::AgentTuiToolReceiptDetails {
                kind,
                status: "completed".to_owned(),
                summary: "ok".to_owned(),
                metadata: Some(metadata),
                error_type: None,
                filesystem_write_performed: false,
            },
        );
        let line = tool_result_line(&receipt);
        let arguments = match line.data.as_ref() {
            Some(RenderCellData::Exec(cell)) => cell.arguments_preview.as_deref(),
            _ => panic!("structured tool receipt should render as typed exec data"),
        };
        assert_eq!(arguments, Some(expected), "tool={tool_name}");
    }
}
