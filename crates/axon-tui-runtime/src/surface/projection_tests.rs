use super::*;
use crate::surface::cell::RenderCellRecord;
use crate::surface::cell_event_data::AgentRenderExecCompletion;
use crate::surface::payloads::{ExecCellPayload, MessageCellPayload};

#[test]
fn exec_cell_projects_to_valid_payload_envelope() {
    let data = RenderCellData::Exec(
        crate::surface::cell_event_data::AgentRenderExecCell::completed_with_id(
            AgentRenderExecCompletion {
                id: "exec-part:call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                status: "success".to_owned(),
                provider_call_id: "call-1".to_owned(),
                summary: Some("完成".to_owned()),
                arguments_preview: Some("{\"cmd\":\"test\"}".to_owned()),
                output_preview: Some("ok".to_owned()),
                duration_ms: Some(42),
            },
        ),
    );

    let envelope = cell_payload_envelope(&data)
        .expect("exec envelope")
        .validate()
        .expect("exec payload schema");
    let payload: ExecCellPayload =
        serde_json::from_value(envelope.payload).expect("exec payload must deserialize");

    assert_eq!(envelope.kind, ExecCellPayload::KIND.as_str());
    assert_eq!(payload.id, "exec-part:call-1");
    assert_eq!(payload.tool_name, "shell_command");
    assert_eq!(payload.display_label, "执行");
    assert_eq!(payload.display_target, None);
    assert_eq!(payload.raw_tool_name, "shell_command");
    assert_eq!(payload.status, ExecStatusPayload::Completed);
    assert_eq!(payload.provider_call_id.as_deref(), Some("call-1"));
    assert_eq!(payload.duration_ms, Some(42));
}

#[test]
fn hook_and_mcp_payloads_preserve_part_ids() {
    let hook_data = RenderCellData::Hook(crate::surface::cell_event_data::AgentRenderHookCell {
        id: "hook:run-1".to_owned(),
        title: "Hook".to_owned(),
        items: Vec::new(),
    });
    let mcp_data = RenderCellData::Mcp(crate::surface::cell_event_data::AgentRenderMcpCell {
        id: "mcp:tool:call-1".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP 调用".to_owned(),
        source: crate::surface::cell_event_data::AgentRenderMcpCellSource::ToolCall,
        items: Vec::new(),
    });

    let hook_envelope = cell_payload_envelope(&hook_data)
        .expect("hook envelope")
        .validate()
        .expect("hook payload schema");
    let mcp_envelope = cell_payload_envelope(&mcp_data)
        .expect("mcp envelope")
        .validate()
        .expect("mcp payload schema");
    let hook_payload: HookCellPayload =
        serde_json::from_value(hook_envelope.payload).expect("hook payload must deserialize");
    let mcp_payload: McpCellPayload =
        serde_json::from_value(mcp_envelope.payload).expect("mcp payload must deserialize");

    assert_eq!(hook_envelope.kind, HookCellPayload::KIND.as_str());
    assert_eq!(mcp_envelope.kind, McpCellPayload::KIND.as_str());
    assert_eq!(hook_payload.id, "hook:run-1");
    assert_eq!(mcp_payload.id, "mcp:tool:call-1");
}

#[test]
fn diff_payload_preserves_part_id() {
    let data = RenderCellData::Diff(crate::surface::cell_diff_data::AgentRenderDiffCell {
        id: "diff:tool:call-1".to_owned(),
        title: "文件变更".to_owned(),
        changes: std::collections::BTreeMap::new(),
        diagnostics: std::collections::BTreeMap::new(),
        text_writes_applied: 0,
        renames_applied: 0,
        truncated: false,
        truncation_boundary: None,
    });

    let envelope = cell_payload_envelope(&data)
        .expect("diff envelope")
        .validate()
        .expect("diff payload schema");
    let payload: DiffCellPayload =
        serde_json::from_value(envelope.payload).expect("diff payload must deserialize");

    assert_eq!(envelope.kind, DiffCellPayload::KIND.as_str());
    assert_eq!(payload.id, "diff:tool:call-1");
}

#[test]
fn generation_status_payload_preserves_part_and_event_ids() {
    let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
        id: "generation:wire-event-1".to_owned(),
        title: "生成".to_owned(),
        items: vec![RenderLifecycleItem {
            id: "wire-event-1".to_owned(),
            kind: "StreamingStarted".to_owned(),
            summary: "cpa/gpt-5 生成回复中".to_owned(),
            phase: AgentTuiLifecyclePhase::Streaming,
            indicator: None,
            tool_name: Some("gpt-5".to_owned()),
            duration_ms: Some(123),
            attempt_index: Some(1),
        }],
    });

    let envelope = cell_payload_envelope(&data)
        .expect("generation status envelope")
        .validate()
        .expect("generation status payload schema");
    let payload: GenerationStatusCellPayload = serde_json::from_value(envelope.payload)
        .expect("generation status payload must deserialize");

    assert_eq!(envelope.kind, GenerationStatusCellPayload::KIND.as_str());
    assert_eq!(payload.id, "generation:wire-event-1");
    assert_eq!(payload.items[0].id, "wire-event-1");
}

#[test]
fn status_and_command_payloads_preserve_part_ids() {
    let status_data = RenderCellData::Status(RenderStatusCell {
        id: "status:wire:1".to_owned(),
        title: "状态".to_owned(),
        items: Vec::new(),
    });
    let command_data =
        RenderCellData::Command(RenderCommandCell::new("command:wire:1", "命令", Vec::new()));

    let status_envelope = cell_payload_envelope(&status_data)
        .expect("status envelope")
        .validate()
        .expect("status payload schema");
    let command_envelope = cell_payload_envelope(&command_data)
        .expect("command envelope")
        .validate()
        .expect("command payload schema");
    let status_payload: StatusCellPayload =
        serde_json::from_value(status_envelope.payload).expect("status payload must deserialize");
    let command_payload: CommandCellPayload =
        serde_json::from_value(command_envelope.payload).expect("command payload must deserialize");

    assert_eq!(status_envelope.kind, StatusCellPayload::KIND.as_str());
    assert_eq!(command_envelope.kind, CommandCellPayload::KIND.as_str());
    assert_eq!(status_payload.id, "status:wire:1");
    assert_eq!(command_payload.id, "command:wire:1");
}

#[test]
fn history_usage_and_error_payloads_preserve_part_ids() {
    let history_data =
        RenderCellData::History(crate::surface::cell_event_data::AgentRenderHistoryCell {
            id: "history:wire:1".to_owned(),
            title: "历史".to_owned(),
            empty_text: "无历史".to_owned(),
            items: Vec::new(),
        });
    let usage_data = RenderCellData::Usage(
        crate::surface::cell_event_data::AgentRenderUsageCell::from_usage(
            "usage:wire:1",
            crate::AgentTuiTokenUsage::new(10, 0, 0, 5, 0),
        ),
    );
    let error_data = RenderCellData::Error(crate::surface::cell_event_data::AgentRenderErrorCell {
        id: "error:wire:1".to_owned(),
        title: "运行错误".to_owned(),
        message: "provider failed".to_owned(),
        details: Vec::new(),
        next_step: None,
        severity: crate::surface::error_model::TuiErrorSeverity::UserFacing,
        action: crate::surface::error_model::TuiErrorAction::InspectDetails,
        cell_kind: crate::surface::error_model::TuiErrorCellKind::Runtime,
    });

    let history_envelope = cell_payload_envelope(&history_data)
        .expect("history envelope")
        .validate()
        .expect("history payload schema");
    let usage_envelope = cell_payload_envelope(&usage_data)
        .expect("usage envelope")
        .validate()
        .expect("usage payload schema");
    let error_envelope = cell_payload_envelope(&error_data)
        .expect("error envelope")
        .validate()
        .expect("error payload schema");
    let history_payload: HistoryCellPayload =
        serde_json::from_value(history_envelope.payload).expect("history payload must deserialize");
    let usage_payload: UsageCellPayload =
        serde_json::from_value(usage_envelope.payload).expect("usage payload must deserialize");
    let error_payload: ErrorCellPayload =
        serde_json::from_value(error_envelope.payload).expect("error payload must deserialize");

    assert_eq!(history_envelope.kind, HistoryCellPayload::KIND.as_str());
    assert_eq!(usage_envelope.kind, UsageCellPayload::KIND.as_str());
    assert_eq!(error_envelope.kind, ErrorCellPayload::KIND.as_str());
    assert_eq!(history_payload.id, "history:wire:1");
    assert_eq!(usage_payload.id, "usage:wire:1");
    assert_eq!(error_payload.id, "error:wire:1");
}

#[test]
fn plan_and_review_payloads_preserve_part_ids() {
    let plan_data = RenderCellData::Plan(crate::surface::cell_plan_data::AgentRenderPlanCell {
        id: "plan:event:payload".to_owned(),
        title: "计划".to_owned(),
        explanation: None,
        steps: Vec::new(),
        source: "# 计划".to_owned(),
        source_kind: crate::surface::cell_plan_data::AgentRenderPlanSourceKind::Markdown,
    });
    let review_data =
        RenderCellData::Review(crate::surface::cell_review_data::AgentRenderReviewCell {
            id: "review:event:payload".to_owned(),
            subject: "验证".to_owned(),
            status: None,
            details: Vec::new(),
        });

    let plan_envelope = cell_payload_envelope(&plan_data)
        .expect("plan envelope")
        .validate()
        .expect("plan payload schema");
    let review_envelope = cell_payload_envelope(&review_data)
        .expect("review envelope")
        .validate()
        .expect("review payload schema");
    let plan_payload: PlanCellPayload =
        serde_json::from_value(plan_envelope.payload).expect("plan payload must deserialize");
    let review_payload: ReviewCellPayload =
        serde_json::from_value(review_envelope.payload).expect("review payload must deserialize");

    assert_eq!(plan_envelope.kind, PlanCellPayload::KIND.as_str());
    assert_eq!(review_envelope.kind, ReviewCellPayload::KIND.as_str());
    assert_eq!(plan_payload.id, "plan:event:payload");
    assert_eq!(review_payload.id, "review:event:payload");
}

#[test]
fn message_payload_uses_record_owned_source() {
    let record = RenderCellRecord::agent_message(
        "assistant:payload:source",
        "## 标题\n\n| A | B |\n|---|---|\n| 1 | 2 |",
    );

    let envelope = record
        .cell_payload_envelope()
        .expect("message envelope")
        .expect("message payload")
        .validate()
        .expect("message payload schema");
    let payload: MessageCellPayload =
        serde_json::from_value(envelope.payload).expect("message payload must deserialize");

    assert_eq!(payload.id, "assistant:payload:source");
    assert_eq!(payload.source, record.text);
}

#[test]
fn turn_summary_payload_preserves_generation_performance() {
    let data = RenderCellData::TurnSummary(
        crate::surface::cell_turn_summary_data::AgentRenderTurnSummaryCell::new(
            "turn-summary:payload:performance",
            Some(5_000),
            Some(crate::AgentTuiTokenUsage::new(1_000, 500, 0, 100, 0)),
        )
        .with_performance(Some(8_000), Some(400)),
    );

    let envelope = cell_payload_envelope(&data)
        .expect("turn summary envelope")
        .validate()
        .expect("turn summary payload schema");
    let payload: crate::surface::payloads::TurnSummaryCellPayload =
        serde_json::from_value(envelope.payload).expect("turn summary payload must deserialize");

    assert_eq!(payload.first_token_avg_ms, Some(8_000));
    assert_eq!(payload.generation_tps_x10, Some(400));
}
