//! 语音版人格：把 `PersonaCore` 压成 realtime 会话能每轮携带的短提示。
//!
//! 语音通道每轮都重发 `instructions`，整份 [`render_stable`] 会拖慢首包并把
//! 指令稀释成噪音，所以这里只保留人格骨架：身份、一句来处、不可违背项、
//! 核心特质、表达基线，外加语音场景红线。完整人格仍由桌面文字侧承载。
//!
//! [`render_stable`]: crate::application::render_stable

use crate::domain::{HumorLevel, PersonaCore, Verbosity};

/// 语音提示的字节上限：约 400 个汉字，够撑人格又不至于拖慢首包。
pub const VOICE_INSTRUCTIONS_MAX_BYTES: usize = 1_200;

/// 渲染语音版人格提示词。
///
/// 输出是单段纯文本，不带 markdown 结构：语音模型按句子消费，多一层结构
/// 只会让它念出符号。
#[must_use]
pub fn render_voice_instructions(core: &PersonaCore) -> String {
    let mut lines = Vec::with_capacity(5);
    lines.push(format!(
        "你是{}，{}的{}。",
        core.identity.name.trim(),
        core.identity.address.trim(),
        core.identity.role.trim()
    ));
    let backstory = clamp_bytes(core.backstory.trim(), 220);
    if !backstory.is_empty() {
        lines.push(backstory);
    }
    let invariants = join_limited(&core.invariants, "；", 3, 320);
    if !invariants.is_empty() {
        lines.push(format!("底线：{invariants}。"));
    }
    let traits = join_limited(&core.traits, "、", 4, 160);
    if !traits.is_empty() {
        lines.push(format!("特质：{traits}。"));
    }
    lines.push(format!("语气：{}。", render_voice(core)));
    lines.push(
        "语音红线：短句、口语、先说结论，一次最多两句话；不用列表、不用 markdown、不复述对方的话。"
            .to_owned(),
    );
    clamp_bytes(&lines.join("\n"), VOICE_INSTRUCTIONS_MAX_BYTES)
}

/// 表达基线转一句可朗读的描述，数字保留下来供模型校准。
fn render_voice(core: &PersonaCore) -> String {
    let voice = &core.voice;
    let warmth = match voice.warmth {
        75..=100 => "温度偏高，先照顾情绪再谈事",
        40..=74 => "温度适中，就事论事但不冷硬",
        _ => "温度偏低，直接给结论",
    };
    let formality = match voice.formality {
        70..=100 => "措辞正式",
        40..=69 => "半正式",
        _ => "日常口语",
    };
    let humor = match voice.humor {
        HumorLevel::Off => "不开玩笑",
        HumorLevel::Subtle => "含蓄的冷幽默",
        HumorLevel::Playful => "可以活泼一点",
    };
    let verbosity = match voice.verbosity {
        Verbosity::Minimal => "只给结论",
        Verbosity::Concise => "简明",
        Verbosity::Detailed => "可以适当展开",
    };
    format!(
        "温度 {}/100（{}），正式度 {}/100（{}），幽默{}，篇幅{}",
        voice.warmth, warmth, voice.formality, formality, humor, verbosity
    )
}

/// 取前若干条拼接，超出预算就停：宁可少一条，也不砍半句。
fn join_limited(items: &[String], separator: &str, max_items: usize, max_bytes: usize) -> String {
    let mut out = String::new();
    for item in items
        .iter()
        .filter(|item| !item.trim().is_empty())
        .take(max_items)
    {
        let piece = clamp_bytes(item.trim(), max_bytes.saturating_sub(out.len()));
        if piece.is_empty() {
            break;
        }
        if !out.is_empty() {
            out.push_str(separator);
        }
        out.push_str(&piece);
        if out.len() >= max_bytes {
            break;
        }
    }
    out
}

/// 按字节上限截断，且不切坏 UTF-8；截断处补省略号。
fn clamp_bytes(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].trim_end().to_owned();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::{VOICE_INSTRUCTIONS_MAX_BYTES, render_voice_instructions};
    use crate::domain::{HumorLevel, Identity, PersonaCore, Verbosity, Voice};

    fn core() -> PersonaCore {
        PersonaCore {
            identity: Identity {
                name: "知微".to_owned(),
                address: "先生".to_owned(),
                role: "私人助理".to_owned(),
            },
            backstory: "在先生身边共事多年。".to_owned(),
            invariants: vec!["没有实测不说没问题".to_owned()],
            traits: vec!["严谨".to_owned(), "干练".to_owned()],
            voice: Voice::new(60, 50, HumorLevel::Subtle, Verbosity::Concise),
            rules: Vec::new(),
            notes: None,
        }
    }

    #[test]
    fn carries_identity_and_voice_red_lines() {
        let text = render_voice_instructions(&core());
        assert!(text.contains("你是知微，先生的私人助理。"));
        assert!(text.contains("底线：没有实测不说没问题。"));
        assert!(text.contains("特质：严谨、干练。"));
        assert!(text.contains("语音红线"));
        assert!(!text.contains('#'), "语音提示不带 markdown 结构");
    }

    #[test]
    fn empty_sections_are_skipped() {
        let mut core = core();
        core.backstory = "   ".to_owned();
        core.invariants.clear();
        core.traits.clear();
        let text = render_voice_instructions(&core);
        assert!(!text.contains("底线："), "没有不可违背项就不出现空段");
        assert!(!text.contains("特质："));
        assert!(text.starts_with("你是知微"));
    }

    #[test]
    fn long_input_is_clamped_within_budget() {
        let mut core = core();
        core.backstory = "很长的来处。".repeat(400);
        core.traits = vec!["特质".repeat(200)];
        let text = render_voice_instructions(&core);
        assert!(
            text.len() <= VOICE_INSTRUCTIONS_MAX_BYTES,
            "总长必须守住上限"
        );
        assert!(text.is_char_boundary(text.len()), "截断不能切坏 UTF-8");
    }

    #[test]
    fn humor_and_verbosity_shift_the_tone_line() {
        let mut playful = core();
        playful.voice = Voice::new(60, 50, HumorLevel::Playful, Verbosity::Minimal);
        let mut serious = core();
        serious.voice = Voice::new(30, 90, HumorLevel::Off, Verbosity::Detailed);
        let playful_text = render_voice_instructions(&playful);
        let serious_text = render_voice_instructions(&serious);
        assert!(playful_text.contains("可以活泼一点"));
        assert!(playful_text.contains("只给结论"));
        assert!(serious_text.contains("不开玩笑"));
        assert!(serious_text.contains("可以适当展开"));
        assert_ne!(playful_text, serious_text);
    }
}
