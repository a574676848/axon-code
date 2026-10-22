//! 用户问答单元 TUI 渲染实现（UQ-203）。
//!
//! 只读呈现：题面、选项与最终作答。可交互选择在底部面板里完成，
//! 转录单元负责把「问了什么、可以怎么答、最后答了什么」留成可回看的事实。

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    surface::{cell::RenderCellData, cell_question_data::AgentRenderQuestionCell},
    widgets::transcript_style::{CellVisual, role_style},
};

/// 跳过作答在转录里的统一文案，与 Rust 唯一编码器的「（跳过）」同措辞。
const SKIPPED_ANSWER_LABEL: &str = "（跳过）";
/// 选项与答案行的统一缩进。
const DETAIL_INDENT: &str = "  ";
/// 单选与多选选项前的复选框符号。
const OPTION_UNCHECKED: &str = "[ ]";
/// 保留给底部面板交互的提示文案。
const QUESTION_CELL_HINT: &str = "在底部面板作答；Esc 收起面板不等于取消。";

/// 还能作答的提问状态：这一相位由底部面板承担交互与选项列表。
///
/// 单元在等待相位只留题面与状态：选项同样画一遍就是同一份内容在屏幕上出现两次，
/// 而此刻用户真正要看的是面板里那份可选中、带焦点的列表。
fn is_answerable(status: &str) -> bool {
    !matches!(status, "answered" | "settled" | "suspended")
}

/// 渲染独立的提问单元。
pub(super) fn render_standalone(
    _visual: CellVisual,
    selected: bool,
    data: Option<&RenderCellData>,
    _width: u16,
) -> Vec<Line<'static>> {
    let Some(cell) = question_cell_data(data) else {
        return Vec::new();
    };

    let mut lines = Vec::new();

    // 状态样式与标题行
    let (status_text, status_style) = match cell.status.as_str() {
        "answered" | "settled" => ("已回答", Style::default().fg(Color::Green)),
        "suspended" => ("已挂起", Style::default().fg(Color::Yellow)),
        _ => ("待回答", Style::default().fg(Color::Cyan)),
    };

    let title_style = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    lines.push(Line::from(vec![
        Span::styled(
            "❓ [用户决策] ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(cell.call_id.clone(), title_style),
        Span::raw(" ("),
        Span::styled(status_text, status_style),
        Span::raw(")"),
    ]));

    if cell.questions.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{DETAIL_INDENT}{QUESTION_CELL_HINT}"),
            role_style(TranscriptColorRole::Muted),
        )));
        return lines;
    }

    // 逐题渲染
    for (idx, q) in cell.questions.iter().enumerate() {
        let prefix = if cell.questions.len() > 1 {
            format!("  {}. ", idx + 1)
        } else {
            DETAIL_INDENT.to_owned()
        };
        let mut question_spans = vec![Span::raw(prefix)];
        if let Some(header) = q.header.as_deref().filter(|value| !value.trim().is_empty()) {
            question_spans.push(Span::styled(
                format!("[{header}] "),
                role_style(TranscriptColorRole::Muted),
            ));
        }
        question_spans.push(Span::styled(
            q.question.clone(),
            role_style(TranscriptColorRole::Assistant),
        ));
        if q.multi_select {
            question_spans.push(Span::styled(
                " (多选)".to_owned(),
                role_style(TranscriptColorRole::Muted),
            ));
        }
        lines.push(Line::from(question_spans));

        if is_answerable(cell.status.as_str()) {
            lines.push(Line::from(Span::styled(
                format!("{DETAIL_INDENT}{QUESTION_CELL_HINT}"),
                role_style(TranscriptColorRole::Muted),
            )));
            continue;
        }

        let answer = cell.answers.iter().find(|answer| answer.id == q.id);
        for option in &q.options {
            let picked = answer.is_some_and(|answer| {
                answer
                    .selected
                    .iter()
                    .any(|selected| selected == &option.label)
            });
            let marker = if picked { "[x]" } else { OPTION_UNCHECKED };
            let mut spans = vec![
                Span::styled(
                    format!("{DETAIL_INDENT}  {marker} "),
                    role_style(TranscriptColorRole::Muted),
                ),
                Span::styled(
                    option.label.clone(),
                    role_style(TranscriptColorRole::Assistant),
                ),
            ];
            if let Some(description) = option
                .description
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                spans.push(Span::styled(
                    format!(" {description}"),
                    role_style(TranscriptColorRole::Muted),
                ));
            }
            lines.push(Line::from(spans));
        }

        lines.extend(answer_lines(answer));
    }

    lines
}

/// 单题作答的只读呈现：自定义答案优先，其次是勾选集合，空集即显式跳过。
fn answer_lines(answer: Option<&axon_domain::UserAnswerItem>) -> Vec<Line<'static>> {
    let Some(answer) = answer else {
        return Vec::new();
    };
    let text = match answer
        .custom
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(custom) => custom.to_owned(),
        None if answer.selected.is_empty() => SKIPPED_ANSWER_LABEL.to_owned(),
        None => answer.selected.join("、"),
    };
    vec![Line::from(vec![
        Span::styled(
            format!("{DETAIL_INDENT}  └─ 答复: "),
            role_style(TranscriptColorRole::Muted),
        ),
        Span::styled(text, role_style(TranscriptColorRole::ToolCompleted)),
    ])]
}

fn question_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderQuestionCell> {
    match data {
        Some(RenderCellData::Question(cell)) => Some(cell),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{UserAnswerItem, UserQuestionItem, UserQuestionOption};
    use ratatui::style::Style;

    /// 渲染结果拼成纯文本：断言只关心「屏幕上到底出现了什么」。
    fn flatten(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }

    fn sample_question() -> UserQuestionItem {
        UserQuestionItem {
            id: "q1".to_owned(),
            // 题面刻意不与选项同名：否则「单元里没有选项」的断言会被题面自己命中。
            question: "是否继续执行本次变更？".to_owned(),
            header: Some("部署确认".to_owned()),
            detail: None,
            options: vec![UserQuestionOption {
                label: "继续部署".to_owned(),
                description: Some("立即推进".to_owned()),
            }],
            multi_select: false,
        }
    }

    fn render(status: &str, answers: Vec<UserAnswerItem>) -> String {
        let data = RenderCellData::Question(AgentRenderQuestionCell::new(
            "question:cell",
            "call-cell",
            status,
            vec![sample_question()],
            answers,
            None,
            1_700_000_000_000,
        ));
        flatten(&render_standalone(visual(), false, Some(&data), 80))
    }

    #[test]
    fn answerable_question_cell_leaves_the_options_to_the_panel() {
        for status in ["pending", "extended"] {
            let text = render(status, Vec::new());
            assert!(
                text.contains("是否继续执行本次变更？"),
                "{status}: 单元仍要留下题面"
            );
            assert!(
                text.contains(QUESTION_CELL_HINT),
                "{status}: 单元要说明去哪里作答"
            );
            assert!(
                !text.contains("继续部署"),
                "{status}: 等待相位的选项由底部面板承担，单元不得再画一遍"
            );
        }
    }

    #[test]
    fn settled_question_cell_keeps_options_and_answer() {
        let answers = vec![UserAnswerItem {
            id: "q1".to_owned(),
            selected: vec!["继续部署".to_owned()],
            custom: None,
        }];

        for status in ["settled", "answered", "suspended"] {
            let text = render(status, answers.clone());
            assert!(
                text.contains("继续部署"),
                "{status}: 结算与挂起后选项是回看事实，必须留着"
            );
            assert!(
                text.contains("答复: 继续部署"),
                "{status}: 作答结果要能看到"
            );
            assert!(
                !text.contains(QUESTION_CELL_HINT),
                "{status}: 已经不作答了，不再提示去底部面板"
            );
        }
    }
}
