//! Unicode 显示宽度边界合同。

use crate::{surface::display_width::DisplayWidthText, widgets::transcript_cells::display_width};

#[cfg(test)]
mod cjk_tests {
    use super::*;

    #[test]
    fn single_cjk_character() {
        assert_eq!(display_width("中"), 2);
        assert_eq!(display_width("文"), 2);
        assert_eq!(display_width("日"), 2);
        assert_eq!(display_width("한"), 2);
    }

    #[test]
    fn mixed_cjk_and_ascii() {
        assert_eq!(display_width("中abc文"), 7);
        assert_eq!(display_width("hello世界"), 9);
    }

    #[test]
    fn cjk_with_punctuation() {
        assert_eq!(display_width("你好，世界。"), 12);
        assert_eq!(display_width("こんにちは！"), 12);
    }

    #[test]
    fn cjk_truncation_at_boundary() {
        let text = "中文测试字符串";
        let truncated = DisplayWidthText::ellipsize(text, 5);
        assert!(display_width(&truncated) <= 5);
        assert_eq!(truncated, "中文…");
    }

    #[test]
    fn cjk_truncation_not_split_character() {
        let text = "汉字";
        let truncated = DisplayWidthText::ellipsize(text, 3);
        assert_eq!(truncated, "汉…");
    }

    #[test]
    fn cjk_zero_width() {
        let truncated = DisplayWidthText::ellipsize("中文", 0);
        assert_eq!(truncated, "…");
    }

    #[test]
    fn cjk_one_width() {
        let truncated = DisplayWidthText::ellipsize("中文", 1);
        assert_eq!(truncated, "…");
    }
}

#[cfg(test)]
mod emoji_tests {
    use super::*;

    #[test]
    fn simple_emoji() {
        assert_eq!(display_width("😀"), 2);
        assert_eq!(display_width("👍"), 2);
        assert_eq!(display_width("❤️"), 2);
    }

    #[test]
    fn emoji_with_variation_selector() {
        assert_eq!(display_width("✈️"), 2);
        assert_eq!(display_width("❤️"), 2);
    }

    #[test]
    fn emoji_with_skin_tone_modifier() {
        assert_eq!(display_width("👍🏽"), 2);
        assert_eq!(display_width("👋🏿"), 2);
    }

    #[test]
    fn emoji_zwj_sequence_family() {
        assert_eq!(display_width("👨‍👩‍👧‍👦"), 2);
    }

    #[test]
    fn emoji_zwj_sequence_couple() {
        assert_eq!(display_width("👩‍❤️‍👨"), 2);
        assert_eq!(display_width("👩‍❤️‍👩"), 2);
    }

    #[test]
    fn emoji_zwj_sequence_profession() {
        assert_eq!(display_width("👨‍💻"), 2);
        assert_eq!(display_width("👩‍⚕️"), 2);
    }

    #[test]
    fn emoji_flag_sequence() {
        assert_eq!(display_width("🇨🇳"), 2);
        assert_eq!(display_width("🇺🇸"), 2);
        assert_eq!(display_width("🇯🇵"), 2);
    }

    #[test]
    fn multiple_emoji() {
        assert_eq!(display_width("😀👍❤️"), 6);
        assert_eq!(display_width("🇨🇳🇺🇸🇯🇵"), 6);
    }

    #[test]
    fn emoji_truncation() {
        let text = "👍🏽abc";
        let truncated = DisplayWidthText::ellipsize(text, 3);
        assert_eq!(truncated, "👍🏽…");
        assert!(display_width(&truncated) <= 3);
    }

    #[test]
    fn emoji_zwj_not_split() {
        let text = "👨‍👩‍👧‍👦abc";
        let truncated = DisplayWidthText::ellipsize(text, 3);
        assert_eq!(truncated, "👨‍👩‍👧‍👦…");
        assert!(display_width(&truncated) <= 4);
    }

    #[test]
    fn emoji_flag_not_split() {
        let text = "🇨🇳abc";
        let truncated = DisplayWidthText::ellipsize(text, 3);
        assert!(truncated.starts_with("🇨🇳") || truncated == "…");
    }
}

#[cfg(test)]
mod mixed_content_tests {
    use super::*;

    #[test]
    fn ascii_cjk_emoji_mix() {
        let text = "Hello 世界 👋🏿";
        assert_eq!(display_width(text), 13);
    }

    #[test]
    fn path_with_cjk() {
        let text = "src/中文路径/file.rs";
        let width = display_width(text);
        assert!(width > 0);
        assert!(width < 100);
    }

    #[test]
    fn url_with_unicode() {
        let text = "https://example.com/中文/path";
        let width = display_width(text);
        assert!(width > 0);
    }

    #[test]
    fn json_with_unicode() {
        let text = r#"{"message": "你好👋", "status": "ok"}"#;
        let width = display_width(text);
        assert!(width > 0);
    }

    #[test]
    fn code_with_emoji_comment() {
        let text = "let x = 42; // 测试 👍";
        let width = display_width(text);
        assert!(width > 0);
    }
}

#[cfg(test)]
mod edge_case_tests {
    use super::*;

    #[test]
    fn empty_string() {
        assert_eq!(display_width(""), 0);
        let truncated = DisplayWidthText::ellipsize("", 10);
        assert_eq!(truncated, "");
    }

    #[test]
    fn only_whitespace() {
        assert_eq!(display_width("   "), 3);
        assert_eq!(DisplayWidthText::width("\t"), 4);
        assert_eq!(display_width("\n"), 1);
    }

    #[test]
    fn zero_width_characters() {
        let text = "a\u{200B}b";
        assert_eq!(display_width(text), 2);

        let text = "a\u{200D}b";
        assert_eq!(display_width(text), 2);
    }

    #[test]
    fn combining_diacritics() {
        let text = "e\u{0301}";
        assert_eq!(display_width(text), 1);

        let text = "a\u{0300}\u{0301}";
        assert_eq!(display_width(text), 1);
    }

    #[test]
    fn control_characters() {
        assert_eq!(display_width("\u{0007}"), 1);
        assert_eq!(display_width("\u{0008}"), 1);
    }

    #[test]
    fn rtl_marks() {
        let text = "abc\u{200F}def";
        assert_eq!(display_width(text), 6);
    }

    #[test]
    fn very_long_string() {
        let text = "a".repeat(10000);
        assert_eq!(display_width(&text), 10000);
    }

    #[test]
    fn very_long_cjk_string() {
        let text = "中".repeat(10000);
        assert_eq!(display_width(&text), 20000);
    }
}

#[cfg(test)]
mod truncation_direction_tests {
    use super::*;

    #[test]
    fn tail_truncation_ascii() {
        let text = "hello world";
        let truncated = DisplayWidthText::ellipsize(text, 8);
        assert_eq!(truncated, "hello w…");
    }

    #[test]
    fn tail_truncation_cjk() {
        let text = "中文测试字符串";
        let truncated = DisplayWidthText::ellipsize(text, 7);
        assert_eq!(truncated, "中文测…");
    }

    #[test]
    fn no_truncation_needed() {
        let text = "short";
        let truncated = DisplayWidthText::ellipsize(text, 100);
        assert_eq!(truncated, text);
    }

    #[test]
    fn exact_width_match() {
        let text = "12345";
        let truncated = DisplayWidthText::ellipsize(text, 5);
        assert_eq!(truncated, text);
    }
}

#[cfg(test)]
mod special_unicode_blocks_tests {
    use super::*;

    #[test]
    fn box_drawing_characters() {
        assert_eq!(display_width("─"), 1);
        assert_eq!(display_width("│"), 1);
        assert_eq!(display_width("┌┐"), 2);
    }

    #[test]
    fn braille_patterns() {
        assert_eq!(display_width("⠀⠁⠂"), 3);
    }

    #[test]
    fn mathematical_alphanumeric() {
        assert_eq!(display_width("𝐀𝐁𝐂"), 3);
    }

    #[test]
    fn tags() {
        let text = "a\u{E0001}b";
        assert_eq!(display_width(text), 2);
    }

    #[test]
    fn private_use_area() {
        let text = "\u{E000}\u{E001}";
        assert_eq!(display_width(text), 2);
    }
}
