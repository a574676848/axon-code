use super::super::{AgentTuiToolKind, AgentTuiToolLifecycleLineInput, AgentTuiToolReceiptDetails};
use super::*;

#[test]
fn grouping_uses_diff_id_as_stable_part_key() {
    let groups = group_transcript_parts([AgentTuiTranscriptPart::Diff(AgentTuiDiffLine::with_id(
        "diff:tool:call-1",
        "文件变更",
        std::collections::BTreeMap::new(),
        0,
        0,
    ))]);

    assert!(matches!(
        &groups[0],
        AgentTuiTranscriptPartGroup::Part { key, .. }
            if key == "part:diff:tool:call-1"
    ));
}

#[test]
fn grouping_uses_tool_part_id_as_stable_key() {
    let groups = group_transcript_parts([
        AgentTuiTranscriptPart::ToolReceipt(AgentTuiToolReceipt::with_id(
            "tool-receipt-part:call-1",
            "call-1",
            "shell",
            AgentTuiToolReceiptDetails {
                kind: AgentTuiToolKind::Shell,
                status: "completed".to_owned(),
                summary: "完成".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
            },
        )),
        AgentTuiTranscriptPart::ToolLifecycle(AgentTuiToolLifecycleLine::with_id(
            AgentTuiToolLifecycleLineInput {
                id: "tool-lifecycle-part:call-2".to_owned(),
                tool_part_id: "tool-part:test:call-2".to_owned(),
                provider_call_id: "call-2".to_owned(),
                tool_name: "search".to_owned(),
                kind: AgentTuiToolKind::Search,
                status: "running".to_owned(),
                summary: "搜索中".to_owned(),
                arguments_preview: None,
                output_preview: None,
            },
        )),
    ]);

    assert!(matches!(
        &groups[0],
        AgentTuiTranscriptPartGroup::Part { key, .. }
            if key == "part:tool-receipt-part:call-1"
    ));
    assert!(matches!(
        &groups[1],
        AgentTuiTranscriptPartGroup::Part { key, .. }
            if key == "part:tool-lifecycle-part:call-2"
    ));
}
