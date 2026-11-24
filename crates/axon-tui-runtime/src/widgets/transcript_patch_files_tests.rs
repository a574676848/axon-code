use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use axon_protocol::diff::FileChange;
use ratatui::style::{Modifier, Style};

use crate::{
    surface::{
        cell::{RenderCellData, RenderCellRecord},
        cell_diff_data::AgentRenderDiffCell,
        display_width::DisplayWidthText,
        render_policy::RenderPolicy,
    },
    widgets::{
        transcript_file_change::render_patch, transcript_patch_files::render_file_changes,
        transcript_style::CellVisual,
    },
};

#[test]
fn renders_add_delete_update_and_rename_with_file_summaries() {
    let patch = diffy::create_patch("alpha\nold\n", "alpha\nnew\n").to_string();
    let rename_patch = diffy::create_patch("fn old() {}\n", "fn new() {}\n").to_string();
    let cell = diff_cell([
        (
            "src/new.rs",
            FileChange::Add {
                content: "pub fn 新增() {}\n".to_owned(),
            },
        ),
        (
            "src/old.rs",
            FileChange::Delete {
                content: "pub fn 删除() {}\n".to_owned(),
            },
        ),
        (
            "src/edit.rs",
            FileChange::Update {
                unified_diff: patch,
                move_path: None,
            },
        ),
        (
            "src/from.rs",
            FileChange::Update {
                unified_diff: rename_patch,
                move_path: Some(PathBuf::from("src/to.rs")),
            },
        ),
    ]);

    let text = rendered_text(&cell, 96);

    assert!(text.contains("Added src/new.rs (+1 -0)"));
    assert!(text.contains("Deleted src/old.rs (+0 -1)"));
    assert!(text.contains("Writed src/edit.rs (+1 -1)"));
    assert!(text.contains("Renamed src/from.rs -> src/to.rs (+1 -1)"));
    assert!(text.contains("+pub fn 新增() {}"));
    assert!(text.contains("-pub fn 删除() {}"));
    assert!(text.contains("@@ -1,2 +1,2 @@"));
    assert!(text.contains("-old"));
    assert!(text.contains("+new"));
}

#[test]
fn renders_multi_hunk_update_with_separator_and_line_numbers() {
    let original = numbered_lines(12, |index| format!("line {index}"));
    let modified = numbered_lines(12, |index| match index {
        2 => "line 2 changed".to_owned(),
        10 => "line 10 changed".to_owned(),
        _ => format!("line {index}"),
    });
    let cell = diff_cell([(
        "src/multi.rs",
        FileChange::Update {
            unified_diff: diffy::create_patch(&original, &modified).to_string(),
            move_path: None,
        },
    )]);

    let text = rendered_text(&cell, 96);

    assert!(text.contains("⋮"));
    assert!(text.contains("@@ -1,5 +1,5 @@"));
    assert!(
        text.matches("@@").count() >= 2,
        "必须渲染多个 hunk 标题：{text}"
    );
    assert_diff_line(&text, "2", "│ -line 2");
    assert_diff_line(&text, "2", "│ +line 2 changed");
    assert_diff_line(&text, "10", "│ -line 10");
    assert_diff_line(&text, "10", "│ +line 10 changed");
}

#[test]
fn known_language_context_lines_use_syntax_theme_colors() {
    let cell = diff_cell([(
        "src/context.rs",
        FileChange::Update {
            unified_diff: diffy::create_patch(
                "let value = 1;\nlet keep = 7;\n",
                "let value = 2;\nlet keep = 7;\n",
            )
            .to_string(),
            move_path: None,
        },
    )]);

    let lines = render_file_changes(test_visual(), diff_decision(&cell), true, &cell, 96);
    let context_line = lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .contains("let keep")
        })
        .expect("必须找到 Rust context 行");
    let colors = context_line
        .spans
        .iter()
        .filter_map(|span| span.style.fg)
        .collect::<HashSet<_>>();

    assert!(
        colors.len() >= 2,
        "context 行应包含多种语法颜色：{context_line:?}"
    );
}

#[test]
fn wraps_long_cjk_lines_without_losing_gutter_alignment() {
    let cell = diff_cell([(
        "src/wide.rs",
        FileChange::Add {
            content: "let message = \"中文中文中文中文中文中文中文中文\";\n".to_owned(),
        },
    )]);

    let rendered = render_file_changes(test_visual(), diff_decision(&cell), true, &cell, 32);
    let body: Vec<String> = rendered
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();

    assert!(
        body.iter().any(|line| line.contains(" 1 │ +let message")),
        "首行必须保留新增行号与 + 号：{body:?}"
    );
    assert!(
        body.iter().any(|line| line.contains("│  文中")),
        "续行必须以空 gutter 对齐：{body:?}"
    );
}

#[test]
fn wraps_long_file_header_without_exceeding_viewport() {
    let path = "E:/project/epaas-dev-ops/epaas-skills/skills/eaip-connector-config-json/scripts/validate-eaip-form-json.ps1";
    let cell = diff_cell([(
        path,
        FileChange::Add {
            content: "param()\n".to_owned(),
        },
    )]);

    let lines = render_file_changes(test_visual(), diff_decision(&cell), false, &cell, 48);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(rendered.len() > 1, "长文件头必须换行：{rendered:?}");
    assert!(
        rendered
            .iter()
            .all(|line| DisplayWidthText::width(line) <= 48),
        "文件头每一行都必须处于 viewport 宽度内：{rendered:?}"
    );
    let compact = rendered
        .iter()
        .flat_map(|line| line.chars().filter(|character| !character.is_whitespace()))
        .collect::<String>();
    assert!(
        compact.contains("validate-eaip-form-json.ps1"),
        "{rendered:?}"
    );
    assert!(rendered.last().is_some_and(|line| line.contains("(+1 -0)")));
}

#[test]
fn renders_large_diff_tail_without_renderer_truncation() {
    let original = numbered_lines(150, |index| format!("value {index}"));
    let modified = numbered_lines(150, |index| format!("value {index} changed"));
    let cell = diff_cell([(
        "src/large.rs",
        FileChange::Update {
            unified_diff: diffy::create_patch(&original, &modified).to_string(),
            move_path: None,
        },
    )]);

    let text = rendered_text(&cell, 120);

    assert!(!text.contains("另 "));
    assert_diff_line(&text, "150", "│ -value 150");
    assert_diff_line(&text, "150", "│ +value 150 changed");
}

#[test]
fn diff_click_expansion_reveals_real_hunk_and_toggle_hint() {
    let cell = diff_cell([(
        "src/inline.rs",
        FileChange::Add {
            content: "pub fn real_result() {}\n".to_owned(),
        },
    )]);

    let collapsed = rendered_patch_text(&cell, false);
    let expanded = rendered_patch_text(&cell, true);

    assert!(collapsed.contains("点击查看详情"));
    assert!(!collapsed.contains("+pub fn real_result() {}"));
    assert!(expanded.contains("点击收起"));
    assert!(expanded.contains("+pub fn real_result() {}"));
}

#[test]
fn nonstandard_update_falls_back_to_raw_change_lines() {
    let cell = diff_cell([(
        "docs/migration.md",
        FileChange::Update {
            unified_diff: "--- a/docs/migration.md\n+++ b/docs/migration.md\n-old\n+new\n"
                .to_owned(),
            move_path: None,
        },
    )]);

    let expanded = rendered_patch_text(&cell, true);

    assert!(expanded.contains("点击收起"));
    assert!(expanded.contains("│ -old"));
    assert!(expanded.contains("│ +new"));
}

#[test]
fn summary_only_diff_omits_expand_hint() {
    let cell = diff_cell([(
        "docs/migration.md",
        FileChange::Update {
            unified_diff: String::new(),
            move_path: None,
        },
    )]);

    let collapsed = rendered_patch_text(&cell, false);

    assert!(!collapsed.contains("点击查看详情"));
    assert!(!collapsed.contains("点击收起"));
}

#[test]
fn expanded_diff_does_not_reverse_selected_summary_line() {
    let cell = diff_cell([(
        "src/inline.rs",
        FileChange::Add {
            content: "pub fn real_result() {}\n".to_owned(),
        },
    )]);

    let data = RenderCellData::Diff(cell.clone());
    let lines = render_patch(test_visual(), diff_decision(&cell), true, Some(&data), 96);

    assert!(
        lines
            .first()
            .expect("展开 diff 至少应有摘要行")
            .spans
            .iter()
            .all(|span| !span.style.add_modifier.contains(Modifier::REVERSED)),
        "展开 diff 的摘要行不应使用反选样式"
    );
}

fn diff_cell(items: impl IntoIterator<Item = (&'static str, FileChange)>) -> AgentRenderDiffCell {
    AgentRenderDiffCell {
        id: "diff:test:patch-files".to_owned(),
        title: "文件变更".to_owned(),
        changes: items
            .into_iter()
            .map(|(path, change)| (PathBuf::from(path), change))
            .collect::<BTreeMap<_, _>>(),
        diagnostics: BTreeMap::new(),
        text_writes_applied: 0,
        renames_applied: 0,
        truncated: false,
        truncation_boundary: None,
    }
}

fn rendered_text(cell: &AgentRenderDiffCell, width: u16) -> String {
    render_file_changes(test_visual(), diff_decision(cell), true, cell, width)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn rendered_patch_text(cell: &AgentRenderDiffCell, selected: bool) -> String {
    let data = RenderCellData::Diff(cell.clone());
    render_patch(
        test_visual(),
        diff_decision(cell),
        selected,
        Some(&data),
        96,
    )
    .into_iter()
    .map(|line| {
        line.spans
            .into_iter()
            .map(|span| span.content)
            .collect::<String>()
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn diff_decision(
    cell: &AgentRenderDiffCell,
) -> crate::surface::render_policy::RenderPolicyDecision {
    RenderPolicy::decide(&RenderCellRecord::file_change_with_data(cell.clone()))
}

fn numbered_lines(count: usize, line: impl Fn(usize) -> String) -> String {
    let mut output = String::new();
    for index in 1..=count {
        output.push_str(&line(index));
        output.push('\n');
    }
    output
}

fn assert_diff_line(rendered: &str, line_number: &str, content: &str) {
    assert!(
        rendered
            .lines()
            .any(|line| line.contains(line_number) && line.contains(content)),
        "未找到带行号 {line_number} 的 diff 行 {content}：{rendered}"
    );
}

fn test_visual() -> CellVisual {
    CellVisual {
        title_style: Style::default(),
        action_text_style: Style::default(),
        detail_prefix_style: Style::default(),
        detail_text_style: Style::default(),
    }
}
