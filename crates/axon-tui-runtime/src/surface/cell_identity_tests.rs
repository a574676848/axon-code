use std::collections::BTreeMap;

use super::{RenderCellRecord, RenderReasoningCell};
use crate::surface::cell_diff_data::AgentRenderDiffCell;
use crate::surface::cell_event_data::{AgentRenderExecCell, AgentRenderExecCompletion};
use crate::surface::cell_plan_data::{AgentRenderPlanCell, AgentRenderPlanSourceKind};

#[test]
fn part_id_stays_stable_when_render_record_is_replaced() {
    let mut cell = RenderCellRecord::reasoning_with_data(RenderReasoningCell {
        id: "reasoning:provider-part-1".to_owned(),
        text: "第一段".to_owned(),
        done: false,
        elapsed_ms: None,
    });
    let render_id = cell.render_id();
    let part_id = cell.part_id();

    cell.replace_with(RenderCellRecord::reasoning_with_data(RenderReasoningCell {
        id: "reasoning:provider-part-1".to_owned(),
        text: "第一段\n第二段".to_owned(),
        done: true,
        elapsed_ms: Some(10),
    }));

    assert_eq!(cell.part_id(), part_id);
    assert_eq!(cell.render_id(), render_id);
    assert_eq!(cell.render_revision(), 1);
}

#[test]
fn agent_message_part_id_uses_payload_identity() {
    let first = RenderCellRecord::agent_message("assistant:test:stable", "第一版");
    let second = RenderCellRecord::agent_message("assistant:test:stable", "第二版");

    assert_ne!(first.render_id(), second.render_id());
    assert_eq!(first.part_id(), "assistant:test:stable");
    assert_eq!(second.part_id(), "assistant:test:stable");
    assert_eq!(
        first.semantic_part_id().as_deref(),
        Some("assistant:test:stable")
    );
    assert_eq!(
        second.semantic_part_id().as_deref(),
        Some("assistant:test:stable")
    );
}

#[test]
fn typed_cell_part_id_uses_payload_identity_without_wrapping() {
    let cell = RenderCellRecord::exec_with_data(
        AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
            id: "tool-part:session:1".to_owned(),
            tool_name: "shell_command".to_owned(),
            status: "success".to_owned(),
            provider_call_id: "provider-call-1".to_owned(),
            summary: Some("完成".to_owned()),
            arguments_preview: None,
            output_preview: Some("ok".to_owned()),
            duration_ms: None,
        }),
        super::RenderCellAction::Ran,
    );

    assert_eq!(cell.part_id(), "tool-part:session:1");
    assert_eq!(
        cell.semantic_part_id().as_deref(),
        Some("tool-part:session:1")
    );
    assert_ne!(cell.part_id(), "exec:tool-part:session:1");
}

#[test]
fn diff_cell_part_id_uses_payload_identity_without_prefix_rewrite() {
    let cell = RenderCellRecord::file_change_with_data(AgentRenderDiffCell {
        id: "diff-part:provider-owned".to_owned(),
        title: "文件变更".to_owned(),
        changes: BTreeMap::default(),
        diagnostics: BTreeMap::default(),
        text_writes_applied: 0,
        renames_applied: 0,
        truncated: false,
        truncation_boundary: None,
    });

    assert_eq!(cell.part_id(), "diff-part:provider-owned");
    assert_eq!(
        cell.semantic_part_id().as_deref(),
        Some("diff-part:provider-owned")
    );
    assert_ne!(cell.part_id(), "diff:diff-part:provider-owned");
}

#[test]
#[should_panic(expected = "structured render cell must carry a payload-owned part id")]
fn typed_cell_part_id_rejects_missing_payload_identity() {
    let cell = RenderCellRecord::plan_with_data(AgentRenderPlanCell {
        id: "  ".to_owned(),
        title: "执行计划".to_owned(),
        explanation: None,
        steps: Vec::new(),
        source: "# 执行计划".to_owned(),
        source_kind: AgentRenderPlanSourceKind::Markdown,
    });

    let _ = cell.part_id();
}
