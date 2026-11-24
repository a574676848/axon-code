//! Transcript typed cell family dispatcher。

use crate::{
    surface::{
        cell::{RenderCellKind, RenderCellRecord, render_cell_registry_entry},
        render_policy::RenderPolicyDecision,
        visual_density::validate_cell_layout_contract,
    },
    widgets::terminal_hyperlinks::{HyperlinkLine, plain_hyperlink_lines},
    widgets::transcript_cell_message,
    widgets::transcript_command,
    widgets::transcript_error,
    widgets::transcript_exec,
    widgets::transcript_file_change,
    widgets::transcript_generation_status,
    widgets::transcript_history,
    widgets::transcript_hook,
    widgets::transcript_mcp,
    widgets::transcript_plan,
    widgets::transcript_question,
    widgets::transcript_reasoning,
    widgets::transcript_review,
    widgets::transcript_status,
    widgets::transcript_style::policy_cell_visual,
    widgets::transcript_turn_summary,
    widgets::transcript_usage,
};

type CellRenderer = fn(RenderPolicyDecision, bool, &RenderCellRecord, u16) -> Vec<HyperlinkLine>;

struct RendererEntry {
    kind: RenderCellKind,
    render: CellRenderer,
}

const CELL_RENDERERS: &[RendererEntry] = &[
    RendererEntry::new(RenderCellKind::MESSAGE, render_message),
    RendererEntry::new(RenderCellKind::EXEC, render_exec),
    RendererEntry::new(RenderCellKind::COMMAND, render_command),
    RendererEntry::new(RenderCellKind::DIFF, render_diff),
    RendererEntry::new(RenderCellKind::HOOK, render_hook),
    RendererEntry::new(RenderCellKind::HISTORY, render_history),
    RendererEntry::new(RenderCellKind::MCP, render_mcp),
    RendererEntry::new(RenderCellKind::STATUS, render_status),
    RendererEntry::new(RenderCellKind::GENERATION_STATUS, render_generation_status),
    RendererEntry::new(RenderCellKind::REASONING, render_reasoning),
    RendererEntry::new(RenderCellKind::TURN_SUMMARY, render_turn_summary),
    RendererEntry::new(RenderCellKind::PLAN, render_plan),
    RendererEntry::new(RenderCellKind::REVIEW, render_review),
    RendererEntry::new(RenderCellKind::ERROR, render_error),
    RendererEntry::new(RenderCellKind::USAGE, render_usage),
    RendererEntry::new(RenderCellKind::QUESTION, render_question),
];

impl RendererEntry {
    const fn new(kind: RenderCellKind, render: CellRenderer) -> Self {
        Self { kind, render }
    }
}

pub(super) fn render_structured_hyperlink_lines(
    kind: RenderCellKind,
    selected: bool,
    decision: RenderPolicyDecision,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    debug_assert!(
        render_cell_registry_entry(kind).is_some(),
        "未注册 RenderCell kind: {}",
        kind.as_str()
    );
    debug_assert!(
        matches!(line.cell_payload_envelope(), Ok(Some(_))),
        "结构化 cell payload schema 无法投影"
    );
    let _layout = validate_cell_layout_contract(kind);
    renderer_for(kind)(decision, selected, line, width)
}

fn renderer_for(kind: RenderCellKind) -> CellRenderer {
    CELL_RENDERERS
        .iter()
        .find(|entry| entry.kind == kind)
        .map(|entry| entry.render)
        .unwrap_or_else(|| panic!("RenderCell renderer 未注册: {}", kind.as_str()))
}

fn render_message(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    transcript_cell_message::render_hyperlink_lines(selected, line, width)
}

fn render_exec(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_exec::render_standalone(
        visual,
        decision,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_command(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_command::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_diff(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_file_change::render_patch(
        visual,
        decision,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_hook(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_hook::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_history(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_history::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_mcp(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_mcp::render_standalone(
        visual,
        decision,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_status(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_status::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_generation_status(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_generation_status::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_reasoning(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_reasoning::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_plan(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_plan::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_review(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_review::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_error(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_error::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_usage(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_usage::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_question(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let selected = decision.render_expanded(selected);
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_question::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}

fn render_turn_summary(
    decision: RenderPolicyDecision,
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    let visual = policy_cell_visual(decision);
    plain_hyperlink_lines(transcript_turn_summary::render_standalone(
        visual,
        selected,
        line.data.as_ref(),
        width,
    ))
}
