//! Transcript 渲染样式语义，集中管理首屏与工作区的视觉层级。

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use crate::app_shell::style_tokens::{TranscriptColorRole, transcript_palette};
use crate::app_shell::syntax_theme::syntax_style_for_scopes;
use crate::surface::cell::{
    ROLE_AGENT, ROLE_ERROR, ROLE_NOTICE, ROLE_TOOL, ROLE_USER, ROLE_VERIFY,
};
use crate::surface::render_policy::{
    RenderPolicyColorRole, RenderPolicyDecision, RenderPolicyVisualRoles,
};

#[derive(Clone, Copy)]
pub(crate) struct RoleVisual {
    pub(crate) marker_style: Style,
    pub(crate) text_style: Style,
}

#[derive(Clone, Copy)]
pub(crate) struct CellVisual {
    pub(crate) title_style: Style,
    pub(crate) action_text_style: Style,
    pub(crate) detail_prefix_style: Style,
    pub(crate) detail_text_style: Style,
}

pub(crate) fn role_visual(role: &str) -> RoleVisual {
    let palette = transcript_palette();
    match role {
        ROLE_USER => RoleVisual {
            marker_style: Style::default().add_modifier(Modifier::BOLD),
            text_style: Style::default(),
        },
        ROLE_AGENT => RoleVisual {
            marker_style: strong(palette.color(TranscriptColorRole::Accent)),
            text_style: Style::default(),
        },
        ROLE_ERROR => RoleVisual {
            marker_style: strong(palette.color(TranscriptColorRole::Error)),
            text_style: Style::default().fg(palette.color(TranscriptColorRole::Error)),
        },
        ROLE_TOOL => RoleVisual {
            marker_style: strong(palette.color(TranscriptColorRole::ToolRunning)),
            text_style: Style::default().fg(palette.color(TranscriptColorRole::Assistant)),
        },
        ROLE_VERIFY => RoleVisual {
            marker_style: strong(palette.color(TranscriptColorRole::Loading)),
            text_style: Style::default().fg(palette.color(TranscriptColorRole::Assistant)),
        },
        ROLE_NOTICE => RoleVisual {
            marker_style: Style::default().fg(palette.color(TranscriptColorRole::Notice)),
            text_style: Style::default().fg(palette.color(TranscriptColorRole::Notice)),
        },
        _ => RoleVisual {
            marker_style: Style::default().fg(palette.color(TranscriptColorRole::Muted)),
            text_style: Style::default().fg(palette.color(TranscriptColorRole::Muted)),
        },
    }
}

pub(crate) fn startup_line_spans(line: String) -> Vec<Span<'static>> {
    let palette = transcript_palette();
    if is_logo_line(&line) {
        return vec![Span::styled(
            line,
            strong(palette.color(TranscriptColorRole::Accent)),
        )];
    }
    if line.starts_with("Axon TUI") || line.starts_with("Axon Personal Agent") {
        return vec![Span::styled(
            line,
            strong(palette.color(TranscriptColorRole::Assistant)),
        )];
    }
    if line.starts_with("知你所想") || line.starts_with("模型路由") {
        return vec![Span::styled(
            line,
            strong(palette.color(TranscriptColorRole::ToolCompleted)),
        )];
    }
    if line.starts_with("融入你的工作区") || line.starts_with("Knowing Your Intent") {
        return vec![Span::styled(
            line,
            Style::default().fg(palette.color(TranscriptColorRole::Muted)),
        )];
    }
    if line.starts_with("入口已就绪") {
        return vec![Span::styled(
            line,
            Style::default().fg(palette.color(TranscriptColorRole::Muted)),
        )];
    }
    if let Some(url) = line.strip_prefix("Tip: 访问 Axon GitHub：") {
        return vec![
            Span::styled(
                "Tip: 访问 Axon GitHub：",
                Style::default().fg(palette.color(TranscriptColorRole::Muted)),
            ),
            Span::styled(
                url.to_owned(),
                Style::default().fg(palette.color(TranscriptColorRole::Accent)),
            ),
        ];
    }
    vec![Span::styled(
        line,
        Style::default().fg(palette.color(TranscriptColorRole::Assistant)),
    )]
}

pub(crate) fn policy_cell_visual(decision: RenderPolicyDecision) -> CellVisual {
    cell_visual_from_roles(decision.visual_roles)
}

fn cell_visual_from_roles(roles: RenderPolicyVisualRoles) -> CellVisual {
    CellVisual {
        title_style: strong(color_role(roles.title)),
        action_text_style: Style::default().fg(color_role(roles.body)),
        detail_prefix_style: Style::default().fg(color_role(roles.detail)),
        detail_text_style: Style::default().fg(color_role(roles.body)),
    }
}

pub(crate) fn role_style(role: TranscriptColorRole) -> Style {
    Style::default().fg(transcript_palette().color(role))
}

pub(crate) fn strong_role_style(role: TranscriptColorRole) -> Style {
    strong(transcript_palette().color(role))
}

pub(crate) fn syntax_role_style(scopes: &[&str], fallback_role: TranscriptColorRole) -> Style {
    syntax_style_for_scopes(scopes, role_style(fallback_role))
}

pub(crate) fn syntax_role_strong_style(
    scopes: &[&str],
    fallback_role: TranscriptColorRole,
) -> Style {
    syntax_role_style(scopes, fallback_role).add_modifier(Modifier::BOLD)
}

fn is_logo_line(line: &str) -> bool {
    line.contains("AXON")
        || line.contains("<>")
        || line.contains('⚡')
        || line.starts_with('╭')
        || line.starts_with('╰')
        || line.starts_with('│')
}

fn strong(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn color_role(role: RenderPolicyColorRole) -> Color {
    let palette = transcript_palette();
    palette.color(match role {
        RenderPolicyColorRole::User => TranscriptColorRole::User,
        RenderPolicyColorRole::Assistant => TranscriptColorRole::Assistant,
        RenderPolicyColorRole::ToolRunning => TranscriptColorRole::ToolRunning,
        RenderPolicyColorRole::ToolCompleted => TranscriptColorRole::ToolCompleted,
        RenderPolicyColorRole::ToolFailed => TranscriptColorRole::ToolFailed,
        RenderPolicyColorRole::DiffAdded => TranscriptColorRole::DiffAdded,
        RenderPolicyColorRole::DiffHunk => TranscriptColorRole::DiffHunk,
        RenderPolicyColorRole::Error => TranscriptColorRole::Error,
        RenderPolicyColorRole::Muted => TranscriptColorRole::Muted,
        RenderPolicyColorRole::Accent => TranscriptColorRole::Accent,
        RenderPolicyColorRole::Loading => TranscriptColorRole::Loading,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_narrative_inherits_terminal_foreground_without_dimming_user_text() {
        assert_eq!(role_visual(ROLE_AGENT).text_style, Style::default());
        assert_eq!(role_visual(ROLE_USER).text_style.fg, None,);
    }
}
