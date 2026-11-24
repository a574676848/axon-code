use super::{
    AgentTuiRunOutput, AgentTuiTokenUsage, AgentTuiUsageLine,
    run_output::{
        AgentTuiHookOutputEntry, AgentTuiHookOutputKind, AgentTuiHookStatusView,
        AgentTuiLifecycleLine, AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput,
        AgentTuiReasoningLine, AgentTuiRunLine,
    },
};
use crate::surface::cell::{RenderCellData, RenderCellKind, RenderCellMode};
use std::{collections::BTreeMap, path::PathBuf};

use crate::r#loop::agent_tui::{AgentTuiDiffLine, file_change_from_parts};
use axon_protocol::diff::FileChange;

#[test]
fn ready_lifecycle_stays_out_of_transcript_cells() {
    let status = AgentTuiRunLine::Status(super::AgentTuiStatusLine::new(
        "运行",
        vec![super::AgentTuiStatusItem::new(
            "Ready",
            "ready status",
            None,
            Some("ready".to_owned()),
            None,
            None,
        )],
    ));

    assert!(status.render_cell().is_none());
}

#[test]
fn lifecycle_events_use_dedicated_generation_status_data() {
    let generation_status = AgentTuiRunLine::GenerationStatus(AgentTuiLifecycleLine::with_id(
        "test:generation:line",
        "运行",
        vec![AgentTuiLifecycleStatusView::with_id(
            AgentTuiLifecycleStatusViewInput {
                id: "test:generation:event".to_owned(),
                title: "运行".to_owned(),
                kind: "Generation".to_owned(),
                summary: "生成回复".to_owned(),
                phase: "generation".to_owned(),
                tool_name: None,
                duration_ms: None,
                attempt_index: None,
            },
        )],
    ))
    .render_cell()
    .expect("generation lifecycle 应可渲染");
    let streaming = AgentTuiRunLine::GenerationStatus(AgentTuiLifecycleLine::with_id(
        "test:streaming:line",
        "运行",
        vec![AgentTuiLifecycleStatusView::with_id(
            AgentTuiLifecycleStatusViewInput {
                id: "test:streaming:event".to_owned(),
                title: "运行".to_owned(),
                kind: "streaming".to_owned(),
                summary: "openai/gpt-5".to_owned(),
                phase: "streaming".to_owned(),
                tool_name: None,
                duration_ms: None,
                attempt_index: None,
            },
        )],
    ))
    .render_cell()
    .expect("streaming lifecycle 应可渲染");

    assert!(matches!(
        generation_status.mode,
        RenderCellMode::Cell(RenderCellKind::GENERATION_STATUS)
    ));
    assert!(matches!(
        generation_status.data,
        Some(RenderCellData::GenerationStatus(_))
    ));
    assert!(matches!(
        streaming.mode,
        RenderCellMode::Cell(RenderCellKind::GENERATION_STATUS)
    ));
    assert!(matches!(
        streaming.data,
        Some(RenderCellData::GenerationStatus(_))
    ));
}

#[test]
fn status_run_line_with_generation_phase_does_not_create_generation_status_cell() {
    let generation_status = AgentTuiRunLine::Status(super::AgentTuiStatusLine::new(
        "生成",
        vec![super::AgentTuiStatusItem {
            kind: "Generation".to_owned(),
            summary: "生成回复".to_owned(),
            phase: Some(super::AgentTuiLifecyclePhase::Generation),
            tool_name: None,
            duration_ms: None,
            turn_id: None,
        }],
    ));
    let command = AgentTuiRunLine::Status(super::AgentTuiStatusLine::new(
        "设置",
        vec![super::AgentTuiStatusItem {
            kind: "Model".to_owned(),
            summary: "gpt-5".to_owned(),
            phase: Some(super::AgentTuiLifecyclePhase::Command),
            tool_name: None,
            duration_ms: None,
            turn_id: None,
        }],
    ))
    .render_cell()
    .expect("command status 应可渲染");

    assert!(generation_status.render_cell().is_none());
    assert!(matches!(
        command.mode,
        RenderCellMode::Cell(RenderCellKind::COMMAND)
    ));
    assert!(matches!(command.data, Some(RenderCellData::Command(_))));
}

#[test]
fn reasoning_run_line_projects_dedicated_reasoning_cell() {
    let line = AgentTuiRunLine::Reasoning(AgentTuiReasoningLine::new(
        "reason-1",
        "分析输入并确认执行路径",
    ))
    .render_cell()
    .expect("reasoning 应可渲染");

    assert!(matches!(
        line.mode,
        RenderCellMode::Cell(RenderCellKind::REASONING)
    ));
    assert!(matches!(line.data, Some(RenderCellData::Reasoning(_))));
}

#[test]
fn status_run_line_with_unknown_phase_does_not_render_noise() {
    let status = AgentTuiRunLine::Status(super::AgentTuiStatusLine::new(
        "状态",
        vec![super::AgentTuiStatusItem {
            kind: "UnknownRuntimeEvent".to_owned(),
            summary: "machine detail".to_owned(),
            phase: Some(super::AgentTuiLifecyclePhase::Unknown),
            tool_name: None,
            duration_ms: None,
            turn_id: None,
        }],
    ));

    assert!(status.render_cell().is_none());
}

#[test]
fn status_run_line_with_notice_phase_keeps_explicit_notice() {
    let status = AgentTuiRunLine::Status(super::AgentTuiStatusLine::new(
        "状态",
        vec![super::AgentTuiStatusItem {
            kind: "Notice".to_owned(),
            summary: "权限检查".to_owned(),
            phase: Some(super::AgentTuiLifecyclePhase::Notice),
            tool_name: None,
            duration_ms: None,
            turn_id: None,
        }],
    ))
    .render_cell()
    .expect("notice status 应保留");

    assert!(matches!(
        status.mode,
        RenderCellMode::Cell(RenderCellKind::STATUS)
    ));
    assert!(matches!(status.data, Some(RenderCellData::Status(_))));
}

#[test]
fn run_output_diff_projects_single_file_change_cell() {
    let cells = AgentTuiRunLine::Diff(AgentTuiDiffLine::new(
        "文件变更",
        diff_changes([(
            "src/old.rs",
            FileChange::Update {
                unified_diff: String::new(),
                move_path: Some(PathBuf::from("src/new.rs")),
            },
        )]),
        0,
        1,
    ))
    .render_cells();

    assert_eq!(cells.len(), 1);
    let line = &cells[0];
    assert!(matches!(
        line.mode,
        RenderCellMode::Cell(RenderCellKind::DIFF)
    ));
    assert!(matches!(&line.data, Some(RenderCellData::Diff(diff))
        if diff.changes.len() == 1
            && matches!(diff.changes.get(&PathBuf::from("src/old.rs")),
                Some(FileChange::Update { move_path: Some(path), .. })
                    if path == &PathBuf::from("src/new.rs"))));
}

#[test]
fn codex_file_change_kind_with_move_path_projects_rename() {
    let change = file_change_from_parts(
        "src/old.rs",
        &serde_json::json!({"type": "update", "movePath": "src/new.rs"}),
        "--- src/old.rs\n+++ src/new.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n",
    )
    .expect("Codex file change 应可投射");

    assert!(matches!(
        change,
        FileChange::Update { move_path: Some(path), unified_diff }
            if path.as_path() == std::path::Path::new("src/new.rs")
                && unified_diff.contains("-old")
                && unified_diff.contains("+new")
    ));
}

#[test]
fn run_output_usage_projects_dedicated_usage_cell() {
    let line = AgentTuiRunLine::Usage(AgentTuiUsageLine::with_id(
        "usage:test:render",
        AgentTuiTokenUsage::new(5_009, 0, 0, 104, 0),
    ))
    .render_cell()
    .expect("usage 应可渲染");

    assert!(matches!(
        line.mode,
        RenderCellMode::Cell(RenderCellKind::USAGE)
    ));
    assert!(matches!(line.data, Some(RenderCellData::Usage(_))));
}

#[test]
fn run_output_hook_projects_persistent_history_cell() {
    let line = AgentTuiRunLine::Hook(AgentTuiHookStatusView::new(
        "PostToolUse",
        "failed",
        Some("权限不足".to_owned()),
    ))
    .render_cell()
    .expect("hook 应可渲染");

    assert!(matches!(
        line.mode,
        RenderCellMode::Cell(RenderCellKind::HISTORY)
    ));
    assert_eq!(line.text, "Hook");
    assert!(matches!(
        line.data,
        Some(RenderCellData::History(ref cell))
            if cell.items.len() == 1
                && cell.items[0].name == "权限不足"
                && cell.items[0].kind_label.as_deref() == Some("PostToolUse")
                && cell.items[0].status_label.as_deref() == Some("失败")
    ));
}

#[test]
fn run_output_completed_hook_entries_do_not_project_history_noise() {
    let line = AgentTuiRunLine::Hook(AgentTuiHookStatusView::with_run_id_and_entries(
        "skill-run",
        "Skill",
        "completed",
        None,
        vec![AgentTuiHookOutputEntry::new(
            AgentTuiHookOutputKind::Feedback,
            "- builtin_glob\n- builtin_read_file",
        )],
    ))
    .render_cell();

    assert!(line.is_none());
}

#[test]
fn run_output_completed_hook_status_ignores_output_entry_noise() {
    let line = AgentTuiRunLine::Hook(AgentTuiHookStatusView::with_run_id_and_entries(
        "skill-run",
        "Skill",
        "completed",
        Some("已增强 draft-vibe-coding".to_owned()),
        vec![AgentTuiHookOutputEntry::new(
            AgentTuiHookOutputKind::Feedback,
            "- builtin_glob\n- builtin_read_file",
        )],
    ))
    .render_cell()
    .expect("带成功摘要的 hook 应投影为紧凑历史状态");

    assert!(matches!(
        line.data,
        Some(RenderCellData::History(ref cell))
            if cell.items.len() == 1
                && cell.items[0].name == "已增强 draft-vibe-coding"
                && cell.items[0].kind_label.as_deref() == Some("Skill")
    ));
    assert!(!line.text.contains("builtin_read_file"));
}

#[test]
fn run_output_push_filters_noop_changes_and_keeps_rename() {
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Diff(AgentTuiDiffLine::new(
        "文件变更",
        diff_changes([
            (
                "src/lib.rs",
                FileChange::Update {
                    unified_diff: String::new(),
                    move_path: None,
                },
            ),
            (
                "src/new.rs",
                FileChange::Add {
                    content: String::new(),
                },
            ),
            (
                "src/old.rs",
                FileChange::Delete {
                    content: String::new(),
                },
            ),
            (
                "src/old_name.rs",
                FileChange::Update {
                    unified_diff: String::new(),
                    move_path: Some(PathBuf::from("src/new_name.rs")),
                },
            ),
        ]),
        1,
        1,
    )));

    let cells = output
        .lines()
        .iter()
        .cloned()
        .flat_map(AgentTuiRunLine::render_cells)
        .collect::<Vec<_>>();

    assert_eq!(cells.len(), 1);
    assert!(matches!(
        cells[0].mode,
        RenderCellMode::Cell(RenderCellKind::DIFF)
    ));
    assert!(matches!(&cells[0].data, Some(RenderCellData::Diff(diff))
        if diff.changes.len() == 1
            && matches!(diff.changes.get(&PathBuf::from("src/old_name.rs")),
                Some(FileChange::Update { move_path: Some(path), .. })
                    if path == &PathBuf::from("src/new_name.rs"))));
}

fn diff_changes<const N: usize>(items: [(&str, FileChange); N]) -> BTreeMap<PathBuf, FileChange> {
    items
        .into_iter()
        .map(|(path, change)| (PathBuf::from(path), change))
        .collect()
}
