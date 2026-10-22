use std::{collections::BTreeMap, path::PathBuf};

use axon_protocol::diff::FileChange;
use ratatui::style::Style;

use crate::{
    surface::display_width::DisplayWidthText,
    surface::{
        cell::RenderCellRecord, cell_diff_data::AgentRenderDiffCell, render_policy::RenderPolicy,
    },
    widgets::{
        adaptive_wrap::adaptive_wrap_text,
        transcript_cells::{display_width, normalize_and_truncate_cell_text},
        transcript_patch_files::render_file_changes,
        transcript_style::CellVisual,
    },
};

const CONTRACT_WIDTH: usize = 10;

#[test]
fn display_width_contract_handles_cjk_and_emoji_clusters() {
    let samples = ["汉字，。", "✈️", "👍🏽", "🇨🇳", "👨‍👩‍👧‍👦"];
    for sample in samples {
        let ellipsized = DisplayWidthText::ellipsize(sample, CONTRACT_WIDTH);
        assert!(
            display_width(&ellipsized) <= CONTRACT_WIDTH,
            "截断结果不能超过显示宽度预算：{sample} -> {ellipsized}"
        );
    }

    assert_eq!(
        normalize_and_truncate_cell_text("中文中文中文English", 9),
        "中文中文…"
    );
    assert_eq!(DisplayWidthText::ellipsize("✈️abc", 3), "✈️…");
    assert_eq!(DisplayWidthText::ellipsize("👨‍👩‍👧‍👦abc", 3), "👨‍👩‍👧‍👦…");
    assert!(DisplayWidthText::ellipsize("路径/👨‍👩‍👧‍👦/文件.rs", 12).ends_with('…'));
}

#[test]
fn adaptive_wrap_keeps_each_cjk_emoji_line_within_display_budget() {
    let source = "中文，中文。👍🏽👨‍👩‍👧‍👦🇨🇳路径路径路径";
    let lines = adaptive_wrap_text(source, CONTRACT_WIDTH);

    assert!(lines.len() > 1, "连续宽字符必须按显示宽度换行：{lines:?}");
    for line in lines {
        assert!(
            display_width(&line) <= CONTRACT_WIDTH,
            "换行结果超过显示宽度预算：{line:?}"
        );
        assert!(!line.ends_with('\u{200d}') && !line.ends_with('\u{fe0f}'));
    }
}

#[test]
fn diff_rendering_aligns_wide_content_at_eighty_columns() {
    let cell = AgentRenderDiffCell {
        id: "diff:test:unicode-width".to_owned(),
        title: "文件变更".to_owned(),
        changes: BTreeMap::from([(
            PathBuf::from("src/中文路径/emoji.rs"),
            FileChange::Add {
                content: "let message = \"中文，emoji 👍🏽👨‍👩‍👧‍👦🇨🇳 variation ✈️\";\n".to_owned(),
            },
        )]),
        diagnostics: BTreeMap::new(),
        text_writes_applied: 0,
        renames_applied: 0,
        truncated: false,
        truncation_boundary: None,
    };

    let decision = RenderPolicy::decide(&RenderCellRecord::file_change_with_data(cell.clone()));
    let lines = render_file_changes(test_visual(), decision, true, &cell, 80);
    let rendered = lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content)
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains(" 1 │ +let message")),
        "首行必须保留行号、gutter 与新增符号：{rendered:?}"
    );
    for line in rendered {
        assert!(
            display_width(&line) <= 80,
            "diff 行不能超过 80 列显示宽度：{line:?}"
        );
    }
}

fn test_visual() -> CellVisual {
    CellVisual {
        title_style: Style::default(),
        action_text_style: Style::default(),
        detail_prefix_style: Style::default(),
        detail_text_style: Style::default(),
    }
}
