//! 稳定段与动态段的渲染边界用例。

use super::*;
use crate::{
    constants::{DYNAMIC_RENDER_MAX_BYTES, STABLE_RENDER_MAX_BYTES},
    domain::{
        persona_core::{Identity, RuleCondition, SceneRule},
        scene_frame::PersonaMode,
        voice::{HumorLevel, Verbosity, Voice, VoicePatch},
    },
    infrastructure::StyleEntry,
};

fn core() -> PersonaCore {
    PersonaCore {
        identity: Identity {
            name: "Axon".to_owned(),
            address: "长官".to_owned(),
            role: "战术副官".to_owned(),
        },
        backstory: "来自工程一线。".to_owned(),
        invariants: vec!["依据事实作答".to_owned()],
        traits: vec!["沉稳".to_owned(), "克制".to_owned()],
        voice: Voice::new(40, 60, HumorLevel::Subtle, Verbosity::Concise),
        rules: vec![SceneRule {
            when: RuleCondition {
                mode: Some(PersonaMode::Incident),
                audience: None,
                user_affect: None,
                domain: None,
                fatigue_level: None,
            },
            voice: VoicePatch {
                humor: Some(HumorLevel::Off),
                ..VoicePatch::default()
            },
        }],
        notes: Some("输出习惯：结论先行。".to_owned()),
    }
}

fn style_entry(index: usize, text: &str) -> StyleEntry {
    StyleEntry {
        style_id: format!("s-{index}"),
        text: text.to_owned(),
        published_at_ms: Some(1_000),
    }
}

#[test]
fn stable_render_is_byte_stable_across_calls() {
    let first = render_stable(&core(), &[style_entry(0, "少用寒暄")]);
    let second = render_stable(&core(), &[style_entry(0, "少用寒暄")]);

    assert_eq!(first, second);
    assert!(first.starts_with("[IDENTITY] name=Axon address=长官 role=战术副官"));
}

#[test]
fn stable_render_keeps_fixed_section_order() {
    let text = render_stable(&core(), &[style_entry(0, "少用寒暄")]);
    let positions: Vec<usize> = [
        "[IDENTITY]",
        "[BACKSTORY]",
        "[INVARIANTS]",
        "[TRAITS]",
        "[VOICE_BASELINE]",
        "[SCENE_RULES]",
        "[LEARNED_STYLE",
        "[NOTES]",
    ]
    .iter()
    .map(|marker| {
        text.find(marker)
            .unwrap_or_else(|| panic!("缺少分节 {marker}"))
    })
    .collect();

    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn invariants_never_disappear_when_budget_is_tight() {
    let mut core = core();
    core.notes = Some("正文".repeat(4_000));
    core.invariants = vec!["依据事实作答".to_owned()];

    let text = render_stable(&core, &[style_entry(0, "少用寒暄")]);

    assert!(text.len() <= STABLE_RENDER_MAX_BYTES);
    assert!(text.contains("[INVARIANTS]"));
    assert!(text.contains("依据事实作答"));
}

#[test]
fn learned_style_is_dropped_before_truncation() {
    let styles = (0..40)
        .map(|index| style_entry(index, &"风格条目".repeat(20)))
        .collect::<Vec<_>>();

    let text = render_stable(&core(), &styles);

    assert!(text.len() <= STABLE_RENDER_MAX_BYTES);
    assert!(!text.contains("[LEARNED_STYLE"));
}

#[test]
fn dynamic_render_stays_within_budget_and_carries_readings() {
    let directive = ExpressionDirective {
        mode: PersonaMode::Casual,
        voice: Voice::new(45, 60, HumorLevel::Subtle, Verbosity::Concise),
        cause: "任务完成".to_owned(),
        guard: Some("上一轮过度寒暄".to_owned()),
        allow_reunion_greeting: false,
    };

    let text = render_dynamic(&directive);

    assert!(text.len() <= DYNAMIC_RENDER_MAX_BYTES);
    assert!(text.contains("mode: casual"));
    assert!(text.contains("warmth=45"));
    assert!(text.contains("cause: 任务完成"));
    assert!(text.contains("guard: 上一轮过度寒暄"));
}

#[test]
fn dynamic_render_omits_absent_optional_lines() {
    let directive = ExpressionDirective {
        mode: PersonaMode::Focus,
        voice: Voice::new(40, 60, HumorLevel::Off, Verbosity::Concise),
        cause: "任务完成".to_owned(),
        guard: None,
        allow_reunion_greeting: false,
    };

    let text = render_dynamic(&directive);

    assert!(!text.contains("guard:"));
}

#[test]
fn truncation_respects_char_boundaries() {
    let text = "人格".repeat(1_000);
    let truncated = truncate_to_bytes(&text, 7);

    assert!(truncated.len() <= 7);
    assert!("人格人格人格".starts_with(&truncated));
}

#[test]
fn empty_notes_and_rules_are_not_rendered() {
    let mut core = core();
    core.notes = None;
    core.rules.clear();

    let text = render_stable(&core, &[]);

    assert!(!text.contains("[NOTES]"));
    assert!(!text.contains("[SCENE_RULES]"));
    assert!(!text.contains("[LEARNED_STYLE"));
}

#[test]
fn rules_are_dropped_before_backstory_is_trimmed() {
    let mut core = core();
    core.backstory = "来路".repeat(120);
    core.rules = (0..200)
        .map(|_| SceneRule {
            when: RuleCondition {
                mode: Some(PersonaMode::Incident),
                audience: None,
                user_affect: None,
                domain: None,
                fatigue_level: None,
            },
            voice: VoicePatch {
                humor: Some(HumorLevel::Off),
                verbosity: Some(Verbosity::Minimal),
                ..VoicePatch::default()
            },
        })
        .collect();

    let text = render_stable(&core, &[]);

    assert!(text.len() <= STABLE_RENDER_MAX_BYTES);
    assert!(!text.contains("[SCENE_RULES]"), "规则应先于来路被舍弃");
    assert!(text.contains("[BACKSTORY]"));
    assert!(text.contains("[INVARIANTS]"));
}

#[test]
fn soft_limit_flags_bloat_before_the_hard_cap() {
    let lean = render_stable(&core(), &[]);
    assert!(!exceeds_soft_limit(&lean));

    let mut bloated = core();
    bloated.backstory = "来路".repeat(1_500);
    let text = render_stable(&bloated, &[]);

    assert!(text.len() <= STABLE_RENDER_MAX_BYTES);
    assert!(exceeds_soft_limit(&text), "越过 4 KiB 即应预警");
}

#[test]
fn dynamic_budget_matches_house_environment_context() {
    assert_eq!(DYNAMIC_RENDER_MAX_BYTES, 1_024);
    assert_eq!(STABLE_RENDER_SOFT_BYTES, 4_096);
    assert_eq!(STABLE_RENDER_MAX_BYTES, 6_144);
}

#[test]
fn oversized_invariants_are_never_truncated() {
    let mut core = core();
    core.invariants = vec!["不可违背项".repeat(1_200)];
    core.backstory = String::new();
    core.traits.clear();
    core.rules.clear();
    core.notes = None;

    let text = render_stable(&core, &[]);

    assert!(text.contains("[INVARIANTS]"));
    assert!(
        text.contains(&core.invariants[0]),
        "最高优先级条款不得被截断，即使整段超限"
    );
    assert!(exceeds_hard_limit(&text), "此情形应在调用方告警");
}

#[test]
fn dynamic_readings_never_appear_in_stable_segment() {
    let directive = ExpressionDirective {
        mode: PersonaMode::Incident,
        voice: Voice::new(50, 70, HumorLevel::Off, Verbosity::Minimal),
        cause: "连续第 2 次失败".to_owned(),
        guard: Some("上一轮过度寒暄".to_owned()),
        allow_reunion_greeting: false,
    };
    let dynamic = render_dynamic(&directive);
    let stable = render_stable(&core(), &[]);

    // 规则里的 mode/verbosity 属于内核声明，允许出现在稳定段；只有本轮读数不得泄漏。
    for reading in [directive.cause.as_str(), "上一轮过度寒暄"] {
        assert!(
            !stable.contains(reading),
            "稳定段不得出现本轮读数：{reading}"
        );
    }
    assert!(dynamic.contains("上一轮过度寒暄"));
}

#[test]
fn worst_case_dynamic_segment_stays_within_token_budget() {
    // 最坏情况：事故模式、最长原因与纠偏提示都写满。
    let directive = ExpressionDirective {
        mode: PersonaMode::Incident,
        voice: Voice::new(20, 80, HumorLevel::Off, Verbosity::Detailed),
        cause: "连续第 9 次失败".repeat(8),
        guard: Some("上一轮过度寒暄并且结论后置".repeat(8)),
        allow_reunion_greeting: false,
    };

    let text = render_dynamic(&directive);
    let bytes = text.len();
    let chars = text.chars().count();

    assert!(
        bytes <= DYNAMIC_RENDER_MAX_BYTES,
        "动态段超字节上限：{bytes}"
    );
    // 中文按一字一 token 保守折算，300 token 对应 300 字。
    assert!(chars <= 300, "动态段超 token 预算：{chars} 字");
    println!("dynamic_bytes={bytes} dynamic_chars={chars}");
}
