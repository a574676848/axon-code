//! 表现指令与拟人停顿：正文给眼睛看，表现给耳朵听。
//!
//! 支持两类内联表现：
//! - 轻量语气词：`[whisper]`、`[sigh]`、`[laugh]`
//! - 结构化标签：`<express mood="teasing" speed="1.2" pitch="2" energy="-1">`
//!
//! 它们不会进入正文或朗读稿，只映射成当句的合成参数；省略号与破折号映射为停顿。

use crate::domain::profile::Verbosity;
use crate::render::render_sentences;

/// 一句的表现参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpressionStyle {
    /// 语速倍率，1.0 为基准。
    pub rate: f32,
    /// 音高增量（半音）。
    pub pitch: f32,
    /// 能量增量（dB）。
    pub energy_db: f32,
}

impl Default for ExpressionStyle {
    fn default() -> Self {
        Self {
            rate: 1.0,
            pitch: 0.0,
            energy_db: 0.0,
        }
    }
}

impl ExpressionStyle {
    /// 与另一组参数叠加。
    #[must_use]
    pub fn merged(self, other: Self) -> Self {
        Self {
            rate: (self.rate * other.rate).clamp(0.8, 1.3),
            pitch: (self.pitch + other.pitch).clamp(-3.0, 3.0),
            energy_db: (self.energy_db + other.energy_db).clamp(-6.0, 3.0),
        }
    }
}

/// 一段带停顿的朗读片段。
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSegment {
    /// 朗读文本。
    pub text: String,
    /// 句后停顿毫秒。
    pub pause_ms: u32,
}

/// 解析表现指令并剥离出正文。
#[must_use]
pub fn parse_expression(markdown: &str) -> (String, ExpressionStyle) {
    let mut style = ExpressionStyle::default();
    let mut text = String::with_capacity(markdown.len());
    let mut rest = markdown;

    while let Some(index) = rest.find('<') {
        text.push_str(&rest[..index]);
        let tail = &rest[index..];
        if let Some(tag) = parse_express_tag(tail) {
            style = style.merged(tag.0);
            rest = &tail[tag.1..];
        } else {
            text.push('<');
            rest = &tail[1..];
        }
    }
    text.push_str(rest);

    for (token, delta) in [
        (
            "[whisper]",
            ExpressionStyle {
                rate: 0.95,
                pitch: -1.0,
                energy_db: -3.0,
            },
        ),
        (
            "[sigh]",
            ExpressionStyle {
                rate: 0.94,
                pitch: -0.5,
                energy_db: -1.0,
            },
        ),
        (
            "[laugh]",
            ExpressionStyle {
                rate: 1.05,
                pitch: 1.0,
                energy_db: 1.0,
            },
        ),
    ] {
        if text.contains(token) {
            text = text.replace(token, "");
            style = style.merged(delta);
        }
    }
    (text.trim().to_owned(), style)
}

fn parse_express_tag(tail: &str) -> Option<(ExpressionStyle, usize)> {
    let end = tail.find('>')?;
    let inner = tail.get(1..end)?;
    let mut parts = inner.split_whitespace();
    if parts.next()? != "express" {
        return None;
    }
    let mut style = ExpressionStyle::default();
    for part in parts {
        let (key, value) = part.split_once('=')?;
        let value = value.trim_matches('"');
        match key {
            "speed" => style.rate = value.parse::<f32>().ok()?.clamp(0.8, 1.3),
            "pitch" => style.pitch = value.parse::<f32>().ok()?.clamp(-3.0, 3.0),
            "energy" => style.energy_db = value.parse::<f32>().ok()?.clamp(-6.0, 3.0),
            "mood" => {
                // 情绪只影响音色倾向，不进入文本。
                style = style.merged(mood_style(value));
            }
            _ => {}
        }
    }
    Some((style, end + 1))
}

fn mood_style(mood: &str) -> ExpressionStyle {
    match mood {
        "teasing" => ExpressionStyle {
            rate: 1.05,
            pitch: 1.0,
            energy_db: 0.5,
        },
        "comforting" => ExpressionStyle {
            rate: 0.92,
            pitch: -0.5,
            energy_db: -1.5,
        },
        "serious" => ExpressionStyle {
            rate: 0.96,
            pitch: -1.0,
            energy_db: -0.5,
        },
        "cheerful" => ExpressionStyle {
            rate: 1.08,
            pitch: 1.5,
            energy_db: 1.0,
        },
        _ => ExpressionStyle::default(),
    }
}

/// 渲染朗读稿并带回表现参数。
#[must_use]
pub fn render_with_expression(markdown: &str, verbosity: Verbosity) -> (String, ExpressionStyle) {
    let (cleaned, style) = parse_expression(markdown);
    let sentences = render_sentences(&cleaned, verbosity);
    (sentences.join("，"), style)
}

/// 按句切分并给出句后停顿：省略号 480 ms，破折号 260 ms，普通句 120 ms（WP-14）。
#[must_use]
pub fn segments_with_pauses(markdown: &str, verbosity: Verbosity) -> Vec<SpeechSegment> {
    let (cleaned, _) = parse_expression(markdown);
    render_sentences(&cleaned, verbosity)
        .into_iter()
        .map(|text| {
            let pause_ms = if text.contains('…') {
                480
            } else if text.contains('—') {
                260
            } else {
                120
            };
            SpeechSegment { text, pause_ms }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ExpressionStyle, parse_expression, render_with_expression, segments_with_pauses};
    use crate::domain::profile::Verbosity;

    #[test]
    fn whisper_tag_is_stripped_and_lowers_energy() {
        let (text, style) = parse_expression("[whisper] 小声说一句");
        assert!(!text.contains("[whisper]"));
        assert!(style.energy_db < 0.0);
        assert!(style.rate < 1.0);
    }

    #[test]
    fn express_tag_sets_rate_and_pitch() {
        let (text, style) = parse_expression("<express speed=\"1.2\" pitch=\"2\"> 好呀");
        assert_eq!(text, "好呀");
        assert!((style.rate - 1.2).abs() < f32::EPSILON);
        assert!((style.pitch - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mood_maps_to_style() {
        let (_, teasing) = parse_expression("<express mood=\"teasing\">");
        let (_, comforting) = parse_expression("<express mood=\"comforting\">");
        assert!(teasing.rate > comforting.rate);
        assert!(comforting.energy_db < 0.0);
    }

    #[test]
    fn unknown_angle_brackets_survive() {
        let (text, style) = parse_expression("3 < 4 是常识");
        assert!(text.contains("3 < 4"));
        assert_eq!(style, ExpressionStyle::default());
    }

    #[test]
    fn render_strips_expression_from_speech() {
        let (speech, style) =
            render_with_expression("[sigh] 好吧，我重新跑一遍。", Verbosity::Detailed);
        assert!(!speech.contains("[sigh]"));
        assert!(style.rate < 1.0);
    }

    #[test]
    fn ellipsis_and_dash_get_longer_pauses() {
        let segments = segments_with_pauses(
            "我需要一点时间……。我马上回来——告诉你结果。",
            Verbosity::Detailed,
        );
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].pause_ms, 480);
        assert_eq!(segments[1].pause_ms, 260);
    }
}
