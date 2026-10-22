//! 时间驱动的扫光高亮动画。
//!
//! 参考 codex `shimmer.rs`：以 process start 为锚点的 2 秒周期扫光，
//! 按 cosine band 计算每个字符的强度，对前景色做线性插值。
//!
//! 为保证测试可重复，公开一个 `with_phase` 入口；无 phase 时使用进程启动时刻。

use std::sync::OnceLock;
use std::time::Instant;

use ratatui::{
    style::{Color, Style},
    text::Span,
};

use crate::app_shell::style_tokens::TEXT_SECONDARY;

/// 扫光周期（秒）。
const SHIMMER_PERIOD_SECONDS: f32 = 2.0;
/// band 半宽（字符）。
const BAND_HALF_WIDTH: f32 = 5.0;
/// 文本两端 padding（字符）。
const SHIMMER_PADDING: usize = 10;
const COLOR_BLEND_SCALE: i32 = 1_000;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

fn elapsed_since_start() -> f32 {
    PROCESS_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_secs_f32()
}

/// 文本当前相位（0..period_seconds）。
#[must_use]
pub(crate) fn shimmer_phase_seconds() -> f32 {
    elapsed_since_start() % SHIMMER_PERIOD_SECONDS
}

/// 把一段文本拆成带 shimmer 高亮的多个 span。
///
/// `base_color` 是不在高亮区的字符颜色；高亮区按强度向
/// `TEXT_SECONDARY` 插值。强度由 cosine band 决定，band 内平滑过渡。
#[must_use]
pub(crate) fn shimmer_spans(text: &str, base_color: Color) -> Vec<Span<'static>> {
    shimmer_spans_with_phase(text, base_color, shimmer_phase_seconds())
}

/// 与 [`shimmer_spans`] 同算法，但允许显式注入相位（便于测试）。
#[must_use]
pub(crate) fn shimmer_spans_with_phase(
    text: &str,
    base_color: Color,
    phase_seconds: f32,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let period = chars.len() + SHIMMER_PADDING * 2;
    let pos = ((phase_seconds / SHIMMER_PERIOD_SECONDS) * period as f32).floor();

    chars
        .into_iter()
        .enumerate()
        .map(|(i, ch)| {
            let i_pos = (i + SHIMMER_PADDING) as f32;
            let dist = (i_pos - pos).abs();
            let intensity = if dist <= BAND_HALF_WIDTH {
                let x = std::f32::consts::PI * (dist / BAND_HALF_WIDTH);
                0.5 * (1.0 + x.cos())
            } else {
                0.0
            };
            let style = shimmer_style(intensity, base_color, TEXT_SECONDARY);
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn shimmer_style(intensity: f32, base: Color, highlight: Color) -> Style {
    Style::default().fg(blend_color(base, highlight, intensity.clamp(0.0, 1.0)))
}

fn blend_color(base: Color, highlight: Color, t: f32) -> Color {
    match (base, highlight) {
        (Color::Rgb(br, bg, bb), Color::Rgb(hr, hg, hb)) => {
            let r = lerp_u8(br, hr, t);
            let g = lerp_u8(bg, hg, t);
            let b = lerp_u8(bb, hb, t);
            Color::Rgb(r, g, b)
        }
        _ => base,
    }
}

fn lerp_u8(base: u8, highlight: u8, t: f32) -> u8 {
    let t_scaled = scaled_intensity(t);
    let low = i32::from(base);
    let high = i32::from(highlight);
    let value = low + ((high - low) * t_scaled + COLOR_BLEND_SCALE / 2) / COLOR_BLEND_SCALE;
    u8::try_from(value.clamp(i32::from(u8::MIN), i32::from(u8::MAX))).unwrap_or(u8::MAX)
}

fn scaled_intensity(t: f32) -> i32 {
    if t <= 0.0 {
        return 0;
    }
    if t >= 1.0 {
        return COLOR_BLEND_SCALE;
    }
    let step = 1.0 / COLOR_BLEND_SCALE as f32;
    let mut threshold = step;
    let mut scaled = 0;
    while scaled < COLOR_BLEND_SCALE {
        if t < threshold {
            return scaled;
        }
        threshold += step;
        scaled += 1;
    }
    COLOR_BLEND_SCALE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_emits_one_span_per_char() {
        let spans = shimmer_spans_with_phase("abc", Color::Rgb(120, 120, 120), 0.0);
        assert_eq!(spans.len(), 3);
    }

    #[test]
    fn shimmer_peak_uses_secondary_without_font_modifiers() {
        let text = "hello world";
        let period = text.chars().count() + SHIMMER_PADDING * 2;
        let peak_phase = SHIMMER_PERIOD_SECONDS * (SHIMMER_PADDING as f32 + 0.5) / period as f32;
        let spans = shimmer_spans_with_phase(text, Color::Rgb(120, 120, 120), peak_phase);

        assert_eq!(spans[0].style.fg, Some(TEXT_SECONDARY));
        assert!(
            spans.iter().all(|span| {
                span.style.add_modifier.is_empty() && span.style.sub_modifier.is_empty()
            }),
            "扫光只能改变前景色，不能闪烁字体粗细"
        );
    }

    #[test]
    fn shimmer_at_zero_phase_differs_from_mid_period() {
        let at_zero = shimmer_spans_with_phase("生成中", Color::Rgb(150, 150, 150), 0.0);
        let at_mid = shimmer_spans_with_phase(
            "生成中",
            Color::Rgb(150, 150, 150),
            SHIMMER_PERIOD_SECONDS * 0.5,
        );
        // 不同相位下，高亮带的位置不同，至少有一个 span 的 style 不同。
        let style_changed = at_zero
            .iter()
            .zip(at_mid.iter())
            .any(|(a, b)| a.style != b.style);
        assert!(style_changed, "不同时刻应至少有一个字符强度不同");
    }

    #[test]
    fn empty_text_returns_empty_spans() {
        let spans = shimmer_spans_with_phase("", Color::Rgb(120, 120, 120), 0.0);
        assert!(spans.is_empty());
    }
}
