//! Codex 兼容的双内置语法主题与终端颜色策略。

use crate::app_shell::style_tokens::{TerminalPaletteDepth, terminal_palette_depth};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use std::{fmt, sync::OnceLock};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Highlighter, Theme},
    parsing::{Scope, SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};
use two_face::{
    syntax::extra_newlines,
    theme::{EmbeddedThemeName, extra},
};

type RgbTuple = (u8, u8, u8);
const LUMA_WEIGHTS: (u16, u16, u16) = (77, 150, 29);
const LUMA_SCALE: u16 = 256;
const THEME_SATURATION_PERCENT: u16 = 85;
const THEME_LUMA_PERCENT: u16 = 15;
const PERCENT_SCALE: u16 = 100;
const PERCENT_ROUNDING_OFFSET: u16 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxThemeName {
    CatppuccinMocha,
    CatppuccinLatte,
}

impl SyntaxThemeName {
    fn from_terminal() -> Self {
        if terminal_background_is_light() {
            Self::CatppuccinLatte
        } else {
            Self::CatppuccinMocha
        }
    }

    fn embedded(self) -> EmbeddedThemeName {
        match self {
            Self::CatppuccinMocha => EmbeddedThemeName::CatppuccinMocha,
            Self::CatppuccinLatte => EmbeddedThemeName::CatppuccinLatte,
        }
    }
}

pub(crate) struct SyntaxTheme {
    syntax_set: SyntaxSet,
    theme: Theme,
    name: SyntaxThemeName,
    depth: TerminalPaletteDepth,
}

impl fmt::Debug for SyntaxTheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyntaxTheme")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl SyntaxTheme {
    pub(crate) fn new(name: SyntaxThemeName) -> Self {
        Self::new_with_depth(name, terminal_palette_depth())
    }

    pub(crate) fn new_with_depth(name: SyntaxThemeName, depth: TerminalPaletteDepth) -> Self {
        Self {
            syntax_set: extra_newlines(),
            theme: extra().get(name.embedded()).clone(),
            name,
            depth,
        }
    }

    pub(crate) fn syntax_for_language(&self, language: &str) -> Option<&SyntaxReference> {
        let language = language.trim();
        if language.is_empty() {
            return None;
        }
        self.syntax_set
            .find_syntax_by_token(language)
            .or_else(|| self.syntax_set.find_syntax_by_name(language))
    }

    pub(crate) fn highlight_line(
        &self,
        syntax: &SyntaxReference,
        line: &str,
    ) -> Vec<Span<'static>> {
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        self.highlight_line_with(&mut highlighter, line)
    }

    pub(crate) fn highlight_line_with(
        &self,
        highlighter: &mut HighlightLines<'_>,
        line: &str,
    ) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for segment in LinesWithEndings::from(line) {
            let highlighted_segments = highlighter
                .highlight_line(segment, &self.syntax_set)
                .unwrap_or_else(|_| Vec::new());
            for (style, text) in highlighted_segments {
                if text.is_empty() {
                    continue;
                }
                spans.push(Span::styled(
                    text.to_owned(),
                    self.style_for_scope(style.font_style, style.foreground),
                ));
            }
        }
        if spans.is_empty() && !line.is_empty() {
            spans.push(Span::styled(line.to_owned(), Style::default()));
        }
        spans
    }

    fn style_for_scope(&self, font_style: FontStyle, color: syntect::highlighting::Color) -> Style {
        let mut style = Style::default().fg(soften_theme_color(color, self.depth));
        if font_style.contains(FontStyle::BOLD) {
            style = style.add_modifier(Modifier::BOLD);
        }
        if font_style.contains(FontStyle::ITALIC) {
            style = style.add_modifier(Modifier::ITALIC);
        }
        style
    }

    pub(crate) fn style_for_scopes(&self, scopes: &[&str]) -> Option<Style> {
        let highlighter = Highlighter::new(&self.theme);
        scopes.iter().find_map(|scope_name| {
            let scope = Scope::new(scope_name).ok()?;
            let color = highlighter.style_mod_for_stack(&[scope]).foreground?;
            Some(Style::default().fg(soften_theme_color(color, self.depth)))
        })
    }
}

pub(crate) fn default_syntax_theme() -> &'static SyntaxTheme {
    static DEFAULT_SYNTAX_THEME: OnceLock<SyntaxTheme> = OnceLock::new();
    DEFAULT_SYNTAX_THEME.get_or_init(|| SyntaxTheme::new(SyntaxThemeName::from_terminal()))
}

pub(crate) fn syntax_style_for_scopes(scopes: &[&str], fallback: Style) -> Style {
    default_syntax_theme()
        .style_for_scopes(scopes)
        .unwrap_or(fallback)
}

fn soften_theme_color(color: syntect::highlighting::Color, depth: TerminalPaletteDepth) -> Color {
    let luma = (u16::from(color.r) * LUMA_WEIGHTS.0
        + u16::from(color.g) * LUMA_WEIGHTS.1
        + u16::from(color.b) * LUMA_WEIGHTS.2)
        / LUMA_SCALE;
    let rgb = (
        soften_channel(color.r, luma),
        soften_channel(color.g, luma),
        soften_channel(color.b, luma),
    );
    quantize_theme_color(rgb, depth)
}

fn quantize_theme_color(rgb: (u8, u8, u8), depth: TerminalPaletteDepth) -> Color {
    match depth {
        TerminalPaletteDepth::NoColor => Color::Reset,
        TerminalPaletteDepth::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
        TerminalPaletteDepth::Ansi256 => Color::Indexed(nearest_ansi256(rgb)),
        TerminalPaletteDepth::Ansi16 => ansi16_color(nearest_ansi16(rgb)),
    }
}

fn nearest_ansi16(rgb: (u8, u8, u8)) -> u8 {
    nearest_color_index(rgb, &ANSI16_RGB)
}

fn nearest_ansi256(rgb: (u8, u8, u8)) -> u8 {
    let mut best_index = 0u8;
    let mut best_distance = u32::MAX;
    for index in 0..=255u8 {
        let candidate = ansi256_rgb(index);
        let distance = color_distance(rgb, candidate);
        if distance < best_distance {
            best_index = index;
            best_distance = distance;
        }
    }
    best_index
}

fn nearest_color_index(rgb: (u8, u8, u8), palette: &[(u8, u8, u8)]) -> u8 {
    let mut best_index = 0u8;
    let mut best_distance = u32::MAX;
    for (index, candidate) in palette.iter().copied().enumerate() {
        let distance = color_distance(rgb, candidate);
        if distance < best_distance {
            best_index = u8::try_from(index).unwrap_or(u8::MAX);
            best_distance = distance;
        }
    }
    best_index
}

fn color_distance(left: (u8, u8, u8), right: (u8, u8, u8)) -> u32 {
    let red = i32::from(left.0) - i32::from(right.0);
    let green = i32::from(left.1) - i32::from(right.1);
    let blue = i32::from(left.2) - i32::from(right.2);
    u32::try_from(red * red + green * green + blue * blue).unwrap_or(u32::MAX)
}

fn ansi16_color(index: u8) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        _ => Color::White,
    }
}

fn ansi256_rgb(index: u8) -> (u8, u8, u8) {
    if index < 16 {
        return ANSI16_RGB[usize::from(index)];
    }
    if index < 232 {
        let index = index - 16;
        let red = index / 36;
        let green = (index % 36) / 6;
        let blue = index % 6;
        return (
            ansi_cube_channel(red),
            ansi_cube_channel(green),
            ansi_cube_channel(blue),
        );
    }
    let gray = 8 + (index - 232) * 10;
    (gray, gray, gray)
}

fn ansi_cube_channel(value: u8) -> u8 {
    if value == 0 { 0 } else { 55 + value * 40 }
}

const ANSI16_RGB: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (128, 0, 0),
    (0, 128, 0),
    (128, 128, 0),
    (0, 0, 128),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (0, 0, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

fn soften_channel(channel: u8, luma: u16) -> u8 {
    let value = (u16::from(channel) * THEME_SATURATION_PERCENT
        + luma * THEME_LUMA_PERCENT
        + PERCENT_ROUNDING_OFFSET)
        / PERCENT_SCALE;
    u8::try_from(value.min(u16::from(u8::MAX))).unwrap_or(u8::MAX)
}

pub(crate) fn terminal_background_style() -> Style {
    let Some(background) = terminal_background_rgb() else {
        return Style::default();
    };
    terminal_contrast_style(background)
}

fn terminal_contrast_style(background: RgbTuple) -> Style {
    let (overlay, alpha) = if rgb_luma(background) >= 150 {
        ((0, 0, 0), 4)
    } else {
        ((255, 255, 255), 12)
    };
    let highlighted_background = (
        blend_channel(overlay.0, background.0, alpha),
        blend_channel(overlay.1, background.1, alpha),
        blend_channel(overlay.2, background.2, alpha),
    );
    let foreground = if rgb_luma(highlighted_background) >= 150 {
        (0, 0, 0)
    } else {
        (255, 255, 255)
    };
    Style::default()
        .fg(Color::Rgb(foreground.0, foreground.1, foreground.2))
        .bg(Color::Rgb(
            highlighted_background.0,
            highlighted_background.1,
            highlighted_background.2,
        ))
}

pub(crate) fn table_separator_style() -> Style {
    let Some((foreground, background)) = terminal_foreground_background_rgb() else {
        return Style::default().add_modifier(Modifier::DIM);
    };
    if !terminal_background_is_light() {
        return Style::default().add_modifier(Modifier::DIM);
    }
    Style::default().fg(Color::Rgb(
        blend_channel(foreground.0, background.0, 20),
        blend_channel(foreground.1, background.1, 20),
        blend_channel(foreground.2, background.2, 20),
    ))
}

fn terminal_background_is_light() -> bool {
    terminal_background_rgb().is_some_and(|rgb| rgb_luma(rgb) >= 150)
}

fn terminal_background_rgb() -> Option<(u8, u8, u8)> {
    let value = std::env::var("COLORFGBG").ok()?;
    let index = value.rsplit(';').next()?.parse::<u8>().ok()?;
    Some(ansi16_rgb(index))
}

fn terminal_foreground_background_rgb() -> Option<(RgbTuple, RgbTuple)> {
    let value = std::env::var("COLORFGBG").ok()?;
    let mut fields = value.rsplit(';');
    let background = fields.next()?.parse::<u8>().ok().map(ansi16_rgb)?;
    let foreground = fields.next()?.parse::<u8>().ok().map(ansi16_rgb)?;
    Some((foreground, background))
}

fn ansi16_rgb(index: u8) -> (u8, u8, u8) {
    ANSI16_RGB[usize::from(index.min(15))]
}

fn rgb_luma(rgb: (u8, u8, u8)) -> u16 {
    (u16::from(rgb.0) * LUMA_WEIGHTS.0
        + u16::from(rgb.1) * LUMA_WEIGHTS.1
        + u16::from(rgb.2) * LUMA_WEIGHTS.2)
        / LUMA_SCALE
}

fn blend_channel(foreground: u8, background: u8, alpha: u8) -> u8 {
    let alpha = u16::from(alpha);
    let value = (u16::from(foreground) * alpha + u16::from(background) * (100 - alpha) + 50) / 100;
    u8::try_from(value.min(u16::from(u8::MAX))).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_line_emits_multiple_theme_colors() {
        let theme = SyntaxTheme::new_with_depth(
            SyntaxThemeName::CatppuccinMocha,
            TerminalPaletteDepth::TrueColor,
        );
        let syntax = theme.syntax_for_language("rust").expect("rust syntax");
        let spans = theme.highlight_line(syntax, "let value = 42;");
        let colors = spans
            .iter()
            .filter_map(|span| span.style.fg)
            .collect::<std::collections::HashSet<_>>();

        assert!(spans.len() >= 2);
        assert!(colors.len() >= 2);
    }

    #[test]
    fn unknown_language_returns_no_syntax() {
        let theme = SyntaxTheme::new(SyntaxThemeName::CatppuccinMocha);

        assert!(theme.syntax_for_language("nosuchlang").is_none());
        assert_eq!(theme.name, SyntaxThemeName::CatppuccinMocha);
    }

    #[test]
    fn theme_softening_preserves_luma_and_reduces_channel_distance() {
        let source = syntect::highlighting::Color {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };
        let softened = soften_theme_color(source, TerminalPaletteDepth::TrueColor);

        assert_eq!(softened, Color::Rgb(228, 11, 11));
    }

    #[test]
    fn theme_colors_quantize_for_supported_terminal_depths() {
        let source = syntect::highlighting::Color {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        };

        assert!(matches!(
            soften_theme_color(source, TerminalPaletteDepth::Ansi256),
            Color::Indexed(_)
        ));
        assert!(matches!(
            soften_theme_color(source, TerminalPaletteDepth::Ansi16),
            Color::Black
                | Color::Red
                | Color::Green
                | Color::Yellow
                | Color::Blue
                | Color::Magenta
                | Color::Cyan
                | Color::Gray
                | Color::DarkGray
                | Color::LightRed
                | Color::LightGreen
                | Color::LightYellow
                | Color::LightBlue
                | Color::LightMagenta
                | Color::LightCyan
                | Color::White
        ));
        assert_eq!(
            soften_theme_color(source, TerminalPaletteDepth::NoColor),
            Color::Reset
        );
    }

    #[test]
    fn terminal_message_background_always_has_contrasting_foreground() {
        let light = terminal_contrast_style((255, 255, 255));
        assert_eq!(light.fg, Some(Color::Rgb(0, 0, 0)));
        assert_eq!(light.bg, Some(Color::Rgb(245, 245, 245)));

        let dark = terminal_contrast_style((0, 0, 0));
        assert_eq!(dark.fg, Some(Color::Rgb(255, 255, 255)));
        assert_eq!(dark.bg, Some(Color::Rgb(31, 31, 31)));
    }
}
