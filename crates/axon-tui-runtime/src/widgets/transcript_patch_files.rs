//! Patch 文件变更块渲染。

use std::path::{Path, PathBuf};

use axon_protocol::diff::FileChange;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use syntect::parsing::SyntaxReference;

use crate::{
    app_shell::{
        style_tokens::TranscriptColorRole,
        syntax_theme::{SyntaxTheme, default_syntax_theme},
    },
    surface::cell_diff_data::{AgentRenderDiffCell, file_change_label, line_stats, rendered_path},
    surface::display_width::DisplayWidthText,
    surface::render_policy::RenderPolicyDecision,
    surface::visual_density::{BULLET_PREFIX, NESTED_PREFIX, NEUTRAL_PREFIX},
    widgets::{
        transcript_cells::{DIFF_FILE_LIMIT, display_width, normalize_and_truncate_cell_text},
        transcript_style::{CellVisual, role_style},
    },
};

const DIFF_FILE_PREFIX: &str = NEUTRAL_PREFIX;
const DIFF_OMITTED_PREFIX: &str = BULLET_PREFIX;
const DIFF_DIAGNOSTIC_PREFIX: &str = "    ! ";
const DIFF_RENAME_ARROW: &str = " -> ";
const DIFF_LINE_PREFIX: &str = NESTED_PREFIX;
const DIFF_DIAGNOSTIC_LIMIT_PER_FILE: usize = 3;
const DIFF_CONTENT_SPACER: &str = " │ ";
const DIFF_HUNK_SEPARATOR: &str = "⋮";

pub(super) fn render_file_changes(
    visual: CellVisual,
    decision: RenderPolicyDecision,
    selected: bool,
    cell: &AgentRenderDiffCell,
    width: u16,
) -> Vec<Line<'static>> {
    let changes = sorted_changes(cell);
    if changes.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for (index, (path, change)) in changes.iter().take(DIFF_FILE_LIMIT).enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.extend(render_file_header_lines(visual, path, change, width));
        append_diagnostics(&mut lines, cell, path, width);
        if decision.render_expanded(selected) {
            append_diff_lines(&mut lines, path, change, width);
        }
    }
    append_omitted_files(&mut lines, visual, changes.len(), width);
    append_truncation_notice(&mut lines, visual, cell, width);
    lines
}

fn sorted_changes(cell: &AgentRenderDiffCell) -> Vec<(PathBuf, FileChange)> {
    cell.changes
        .iter()
        .map(|(path, change)| (path.clone(), change.clone()))
        .collect()
}

fn render_file_header_lines(
    visual: CellVisual,
    path: &Path,
    change: &FileChange,
    width: u16,
) -> Vec<Line<'static>> {
    let path_text = rendered_path(path, change);
    let label = file_change_label(change);
    let (added, removed) = line_stats(change);
    let first_prefix = format!("{DIFF_FILE_PREFIX}{label} ");
    let stats_text = line_stats_text(added, removed);
    let path_width = available_width(width, &format!("{first_prefix} {stats_text}"));
    let chunks = DisplayWidthText::hard_wrap(&path_text, path_width);
    let chunk_count = chunks.len();
    let label_style = Style::default()
        .fg(change_color(change))
        .add_modifier(Modifier::BOLD);
    let continuation_prefix = " ".repeat(display_width(&first_prefix));

    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let is_first = index == 0;
            let is_last = index + 1 == chunk_count;
            let mut spans = if is_first {
                vec![
                    Span::styled(
                        DIFF_FILE_PREFIX.to_owned(),
                        visual.detail_prefix_style.fg(change_color(change)),
                    ),
                    Span::styled(label.to_owned(), label_style),
                    Span::raw(" "),
                ]
            } else {
                vec![Span::raw(continuation_prefix.clone())]
            };
            spans.extend(file_path_spans(change, &chunk));
            if is_last {
                spans.push(Span::raw(" "));
                spans.extend(line_stats_spans(added, removed));
            }
            Line::from(spans)
        })
        .collect()
}

fn file_path_spans(change: &FileChange, rendered_path: &str) -> Vec<Span<'static>> {
    let path_style = Style::default().fg(change_color(change));
    if matches!(
        change,
        FileChange::Update {
            move_path: Some(_),
            ..
        }
    ) {
        renamed_path_spans(rendered_path, path_style)
    } else {
        vec![Span::styled(rendered_path.to_owned(), path_style)]
    }
}

fn renamed_path_spans(rendered_path: &str, path_style: Style) -> Vec<Span<'static>> {
    let Some((previous, current)) = rendered_path.split_once(DIFF_RENAME_ARROW) else {
        return vec![Span::styled(rendered_path.to_owned(), path_style)];
    };
    vec![
        Span::styled(previous.to_owned(), role_style(TranscriptColorRole::Muted)),
        Span::styled(
            DIFF_RENAME_ARROW.to_owned(),
            role_style(TranscriptColorRole::Muted),
        ),
        Span::styled(current.to_owned(), path_style),
    ]
}

fn line_stats_spans(added: usize, removed: usize) -> Vec<Span<'static>> {
    vec![
        Span::styled("(".to_owned(), role_style(TranscriptColorRole::Muted)),
        Span::styled(
            format!("+{added}"),
            role_style(TranscriptColorRole::DiffAdded),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{removed}"),
            role_style(TranscriptColorRole::DiffRemoved),
        ),
        Span::styled(")".to_owned(), role_style(TranscriptColorRole::Muted)),
    ]
}

fn line_stats_text(added: usize, removed: usize) -> String {
    format!("(+{added} -{removed})")
}

fn append_diff_lines(lines: &mut Vec<Line<'static>>, path: &Path, change: &FileChange, width: u16) {
    let render_lines = diff_render_lines(change);
    if render_lines.is_empty() {
        return;
    }
    let layout = DiffLayout::for_lines(&render_lines);
    let syntax = syntax_for_path(path);
    for line in &render_lines {
        lines.extend(render_diff_content_line(line, layout, width, syntax));
    }
}

fn append_diagnostics(
    lines: &mut Vec<Line<'static>>,
    cell: &AgentRenderDiffCell,
    path: &Path,
    width: u16,
) {
    let Some(diagnostics) = cell.diagnostics.get(path) else {
        return;
    };
    let omitted = diagnostics
        .len()
        .saturating_sub(DIFF_DIAGNOSTIC_LIMIT_PER_FILE);
    for diagnostic in diagnostics.iter().take(DIFF_DIAGNOSTIC_LIMIT_PER_FILE) {
        let text = normalize_and_truncate_cell_text(
            diagnostic,
            available_width(width, DIFF_DIAGNOSTIC_PREFIX),
        );
        lines.push(Line::from(vec![
            Span::styled(
                DIFF_DIAGNOSTIC_PREFIX.to_owned(),
                role_style(TranscriptColorRole::Accent),
            ),
            Span::styled(text, role_style(TranscriptColorRole::Accent)),
        ]));
    }
    if omitted > 0 {
        let text = normalize_and_truncate_cell_text(
            &format!("另 {omitted} 条诊断"),
            available_width(width, DIFF_DIAGNOSTIC_PREFIX),
        );
        lines.push(Line::from(vec![
            Span::styled(
                DIFF_DIAGNOSTIC_PREFIX.to_owned(),
                role_style(TranscriptColorRole::Muted),
            ),
            Span::styled(text, role_style(TranscriptColorRole::Muted)),
        ]));
    }
}

#[derive(Clone, Debug)]
struct DiffRenderLine {
    kind: DiffLineKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLineKind {
    Hunk,
    Context,
    Added,
    Deleted,
    Separator,
    Notice,
}

#[derive(Clone, Copy)]
struct DiffLayout {
    old_width: usize,
    new_width: usize,
}

impl DiffLayout {
    fn for_lines(lines: &[DiffRenderLine]) -> Self {
        let old_max = lines
            .iter()
            .filter_map(|line| line.old_line)
            .max()
            .unwrap_or(0);
        let new_max = lines
            .iter()
            .filter_map(|line| line.new_line)
            .max()
            .unwrap_or(0);
        Self {
            old_width: line_number_width(old_max),
            new_width: line_number_width(new_max),
        }
    }

    fn gutter_width(self) -> usize {
        self.old_width + 1 + self.new_width
    }
}

fn diff_render_lines(change: &FileChange) -> Vec<DiffRenderLine> {
    match change {
        FileChange::Add { content } => content_render_lines(content, DiffLineKind::Added),
        FileChange::Delete { content } => content_render_lines(content, DiffLineKind::Deleted),
        FileChange::Update { unified_diff, .. } => update_render_lines(unified_diff),
    }
}

fn content_render_lines(content: &str, kind: DiffLineKind) -> Vec<DiffRenderLine> {
    content
        .lines()
        .enumerate()
        .map(|(index, line)| DiffRenderLine {
            kind,
            old_line: matches!(kind, DiffLineKind::Deleted).then_some(index + 1),
            new_line: matches!(kind, DiffLineKind::Added).then_some(index + 1),
            text: line.to_owned(),
        })
        .collect()
}

fn update_render_lines(unified_diff: &str) -> Vec<DiffRenderLine> {
    if unified_diff.trim().is_empty() {
        return Vec::new();
    }
    if let Ok(patch) = diffy::Patch::from_str(unified_diff) {
        let parsed = patch_render_lines(&patch);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    raw_update_render_lines(unified_diff)
}

fn patch_render_lines(patch: &diffy::Patch<'_, str>) -> Vec<DiffRenderLine> {
    let mut output = Vec::new();
    for (hunk_index, hunk) in patch.hunks().iter().enumerate() {
        if hunk_index > 0 {
            output.push(DiffRenderLine {
                kind: DiffLineKind::Separator,
                old_line: None,
                new_line: None,
                text: DIFF_HUNK_SEPARATOR.to_owned(),
            });
        }
        output.push(DiffRenderLine {
            kind: DiffLineKind::Hunk,
            old_line: None,
            new_line: None,
            text: hunk_header(hunk),
        });
        append_hunk_render_lines(&mut output, hunk);
    }
    output
}

fn append_hunk_render_lines(output: &mut Vec<DiffRenderLine>, hunk: &diffy::Hunk<'_, str>) {
    let mut old_line = hunk.old_range().start();
    let mut new_line = hunk.new_range().start();
    for line in hunk.lines() {
        let (kind, old, new, text) = match line {
            diffy::Line::Insert(text) => {
                let current = new_line;
                new_line = new_line.saturating_add(1);
                (DiffLineKind::Added, None, Some(current), text)
            }
            diffy::Line::Delete(text) => {
                let current = old_line;
                old_line = old_line.saturating_add(1);
                (DiffLineKind::Deleted, Some(current), None, text)
            }
            diffy::Line::Context(text) => {
                let current_old = old_line;
                let current_new = new_line;
                old_line = old_line.saturating_add(1);
                new_line = new_line.saturating_add(1);
                (
                    DiffLineKind::Context,
                    Some(current_old),
                    Some(current_new),
                    text,
                )
            }
        };
        output.push(parsed_diff_line(kind, old, new, text));
    }
}

fn raw_update_render_lines(unified_diff: &str) -> Vec<DiffRenderLine> {
    unified_diff
        .lines()
        .filter_map(raw_update_render_line)
        .collect()
}

fn raw_update_render_line(line: &str) -> Option<DiffRenderLine> {
    if line.is_empty() || line.starts_with("---") || line.starts_with("+++") {
        return None;
    }
    let (kind, text) = if let Some(text) = line.strip_prefix('+') {
        (DiffLineKind::Added, text)
    } else if let Some(text) = line.strip_prefix('-') {
        (DiffLineKind::Deleted, text)
    } else if line.starts_with("@@") {
        (DiffLineKind::Hunk, line)
    } else if let Some(text) = line.strip_prefix(' ') {
        (DiffLineKind::Context, text)
    } else {
        (DiffLineKind::Notice, line)
    };
    Some(parsed_diff_line(kind, None, None, text))
}

fn parsed_diff_line(
    kind: DiffLineKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: &str,
) -> DiffRenderLine {
    DiffRenderLine {
        kind,
        old_line,
        new_line,
        text: text.trim_end_matches('\n').to_owned(),
    }
}

fn hunk_header(hunk: &diffy::Hunk<'_, str>) -> String {
    format!(
        "@@ -{} +{} @@",
        hunk_range_label(hunk.old_range()),
        hunk_range_label(hunk.new_range())
    )
}

fn hunk_range_label(range: diffy::HunkRange) -> String {
    let start = range.start();
    let len = range.len();
    if len == 1 {
        start.to_string()
    } else {
        format!("{start},{len}")
    }
}

fn render_diff_content_line(
    line: &DiffRenderLine,
    layout: DiffLayout,
    width: u16,
    syntax: Option<(&SyntaxTheme, &SyntaxReference)>,
) -> Vec<Line<'static>> {
    let syntax = matches!(line.kind, DiffLineKind::Context)
        .then_some(syntax)
        .flatten();
    render_wrapped_diff_line(DiffWrappedLineRequest {
        gutter: diff_line_gutter(line, layout),
        sign: diff_line_sign(line.kind),
        text: &line.text,
        content_style: diff_line_style(line.kind),
        gutter_style: gutter_style(line.kind),
        width,
        layout,
        syntax,
    })
}

struct DiffWrappedLineRequest<'a> {
    gutter: String,
    sign: &'static str,
    text: &'a str,
    content_style: Style,
    gutter_style: Style,
    width: u16,
    layout: DiffLayout,
    syntax: Option<(&'a SyntaxTheme, &'a SyntaxReference)>,
}

fn render_wrapped_diff_line(request: DiffWrappedLineRequest<'_>) -> Vec<Line<'static>> {
    let DiffWrappedLineRequest {
        gutter,
        sign,
        text,
        content_style,
        gutter_style,
        width,
        layout,
        syntax,
    } = request;
    let available = diff_content_width(width, layout);
    let chunks = wrap_text(text, available);
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let current_gutter = if index == 0 {
                gutter.clone()
            } else {
                " ".repeat(layout.gutter_width())
            };
            let current_sign = if index == 0 { sign } else { " " };
            let mut spans = vec![
                Span::styled(
                    DIFF_LINE_PREFIX.to_owned(),
                    role_style(TranscriptColorRole::Muted),
                ),
                Span::styled(current_gutter, gutter_style),
                Span::styled(
                    DIFF_CONTENT_SPACER.to_owned(),
                    role_style(TranscriptColorRole::Muted),
                ),
                Span::styled(current_sign.to_owned(), content_style),
            ];
            if let Some((theme, syntax)) = syntax {
                spans.extend(theme.highlight_line(syntax, &chunk));
            } else {
                spans.push(Span::styled(chunk, content_style));
            }
            Line::from(spans)
        })
        .collect()
}

fn syntax_for_path(path: &Path) -> Option<(&'static SyntaxTheme, &'static SyntaxReference)> {
    let theme = default_syntax_theme();
    let language = path
        .extension()
        .and_then(|extension| extension.to_str())
        .or_else(|| path.file_name().and_then(|name| name.to_str()))?;
    theme
        .syntax_for_language(language)
        .map(|syntax| (theme, syntax))
}

fn diff_content_width(width: u16, layout: DiffLayout) -> usize {
    usize::from(width)
        .saturating_sub(display_width(DIFF_LINE_PREFIX))
        .saturating_sub(layout.gutter_width())
        .saturating_sub(display_width(DIFF_CONTENT_SPACER))
        .saturating_sub(1)
        .max(1)
}

fn diff_line_gutter(line: &DiffRenderLine, layout: DiffLayout) -> String {
    format!(
        "{} {}",
        line.old_line.map_or_else(
            || " ".repeat(layout.old_width),
            |value| format!("{value:>width$}", width = layout.old_width)
        ),
        line.new_line.map_or_else(
            || " ".repeat(layout.new_width),
            |value| format!("{value:>width$}", width = layout.new_width)
        )
    )
}

const fn diff_line_sign(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Added => "+",
        DiffLineKind::Deleted => "-",
        DiffLineKind::Separator
        | DiffLineKind::Hunk
        | DiffLineKind::Context
        | DiffLineKind::Notice => " ",
    }
}

fn diff_line_style(kind: DiffLineKind) -> Style {
    match kind {
        DiffLineKind::Added => role_style(TranscriptColorRole::DiffAdded),
        DiffLineKind::Deleted => role_style(TranscriptColorRole::DiffRemoved),
        DiffLineKind::Hunk => {
            role_style(TranscriptColorRole::DiffHunk).add_modifier(Modifier::BOLD)
        }
        DiffLineKind::Separator | DiffLineKind::Notice => role_style(TranscriptColorRole::Muted),
        DiffLineKind::Context => role_style(TranscriptColorRole::Assistant),
    }
}

fn gutter_style(kind: DiffLineKind) -> Style {
    match kind {
        DiffLineKind::Added => role_style(TranscriptColorRole::DiffAdded),
        DiffLineKind::Deleted => role_style(TranscriptColorRole::DiffRemoved),
        DiffLineKind::Hunk
        | DiffLineKind::Separator
        | DiffLineKind::Notice
        | DiffLineKind::Context => role_style(TranscriptColorRole::Muted),
    }
}

fn append_omitted_files(
    lines: &mut Vec<Line<'static>>,
    visual: CellVisual,
    original_count: usize,
    width: u16,
) {
    let omitted = original_count.saturating_sub(DIFF_FILE_LIMIT);
    if omitted == 0 {
        return;
    }
    let text = normalize_and_truncate_cell_text(
        &format!("另 {omitted} 个文件"),
        available_width(width, DIFF_OMITTED_PREFIX),
    );
    lines.push(Line::from(vec![
        Span::styled(DIFF_OMITTED_PREFIX.to_owned(), visual.detail_prefix_style),
        Span::styled(text, visual.detail_text_style),
    ]));
}

fn append_truncation_notice(
    lines: &mut Vec<Line<'static>>,
    visual: CellVisual,
    cell: &AgentRenderDiffCell,
    width: u16,
) {
    if !cell.truncated {
        return;
    }
    let message = cell.truncation_boundary.as_deref().map_or_else(
        || "diff 已截断".to_owned(),
        |boundary| format!("diff 已截断：{boundary}"),
    );
    let text =
        normalize_and_truncate_cell_text(&message, available_width(width, DIFF_OMITTED_PREFIX));
    lines.push(Line::from(vec![
        Span::styled(DIFF_OMITTED_PREFIX.to_owned(), visual.detail_prefix_style),
        Span::styled(text, role_style(TranscriptColorRole::Muted)),
    ]));
}

fn available_width(width: u16, prefix: &str) -> usize {
    usize::from(width)
        .saturating_sub(display_width(prefix))
        .max(1)
}

fn change_color(change: &FileChange) -> Color {
    let palette = crate::app_shell::style_tokens::transcript_palette();
    match change {
        FileChange::Add { .. } => palette.color(TranscriptColorRole::DiffAdded),
        FileChange::Delete { .. } => palette.color(TranscriptColorRole::DiffRemoved),
        FileChange::Update { .. } => palette.color(TranscriptColorRole::DiffHunk),
    }
}

fn line_number_width(max_line_number: usize) -> usize {
    max_line_number.to_string().len().max(1)
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    DisplayWidthText::hard_wrap(text, width)
}
