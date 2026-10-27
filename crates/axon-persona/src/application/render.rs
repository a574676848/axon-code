//! 人格片段渲染。
//!
//! 稳定段分节顺序固定、字段顺序固定、换行统一，保证内容不变时字节不变；
//! 动态段只承载本轮读数，且硬性限制在体积预算内。

use crate::{
    constants::{DYNAMIC_RENDER_MAX_BYTES, STABLE_RENDER_MAX_BYTES, STABLE_RENDER_SOFT_BYTES},
    domain::{expression_policy::ExpressionDirective, persona_core::PersonaCore},
    infrastructure::StyleEntry,
};

/// 渲染稳定段正文（不含片段标记）。
///
/// 体积超限时按分节优先级收敛，顺序为：习得风格、正文、场景规则、来路正文、特质与表达基线。
/// `IDENTITY` 与 `INVARIANTS` 永不截断：若二者本身已无法装进预算，宁可让整段超限，
/// 由调用方记录告警，也不静默截半最高优先级条款。
#[must_use]
pub fn render_stable(core: &PersonaCore, styles: &[StyleEntry]) -> String {
    let mut include_style = !styles.is_empty();
    let mut include_notes = core
        .notes
        .as_deref()
        .is_some_and(|notes| !notes.trim().is_empty());
    let mut include_rules = !core.rules.is_empty();
    let mut include_support = true;
    let mut backstory = core.backstory.trim().to_owned();

    loop {
        let text = assemble_stable(
            core,
            styles,
            &backstory,
            include_style,
            include_notes,
            include_rules,
            include_support,
        );
        if text.len() <= STABLE_RENDER_MAX_BYTES {
            return text;
        }
        if include_style {
            include_style = false;
        } else if include_notes {
            include_notes = false;
        } else if include_rules {
            include_rules = false;
        } else if !backstory.is_empty() {
            let overflow = text.len() - STABLE_RENDER_MAX_BYTES;
            let keep = backstory.len().saturating_sub(overflow + 8);
            backstory = truncate_to_bytes(&backstory, keep);
        } else if include_support {
            include_support = false;
        } else {
            // 只剩 IDENTITY 与 INVARIANTS：按合同不得截断，原样返回并由调用方告警。
            return text;
        }
    }
}

/// 稳定段是否超出硬上限；调用方据此记录告警。
#[must_use]
pub fn exceeds_hard_limit(text: &str) -> bool {
    text.len() > STABLE_RENDER_MAX_BYTES
}

#[allow(clippy::too_many_arguments)]
fn assemble_stable(
    core: &PersonaCore,
    styles: &[StyleEntry],
    backstory: &str,
    include_style: bool,
    include_notes: bool,
    include_rules: bool,
    include_support: bool,
) -> String {
    let mut sections: Vec<String> = Vec::new();
    sections.push(format!(
        "[IDENTITY] name={} address={} role={}",
        core.identity.name, core.identity.address, core.identity.role
    ));
    sections.push(format!("[BACKSTORY]\n{backstory}"));
    sections.push(format!("[INVARIANTS]\n{}", bullet_list(&core.invariants)));
    if include_support {
        sections.push(format!("[TRAITS] {}", core.traits.join("、")));
        sections.push(format!("[VOICE_BASELINE] {}", core.voice.render()));
    }
    if include_rules {
        sections.push(format!("[SCENE_RULES]\n{}", rules_text(core)));
    }
    if include_style {
        let entries = styles
            .iter()
            .map(|entry| format!("- {}", entry.text.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "[LEARNED_STYLE revision={}]\n{entries}",
            styles.len()
        ));
    }
    if let Some(notes) = core
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty())
        .filter(|_| include_notes)
    {
        sections.push(format!("[NOTES]\n{notes}"));
    }
    sections.join("\n")
}

/// 按字节数判断是否越过软门槛，供装配方在不持有渲染字符串时复用。
#[must_use]
pub fn exceeds_soft_limit_len(bytes: usize) -> bool {
    bytes > STABLE_RENDER_SOFT_BYTES
}

/// 按字节数判断是否越过硬上限。
#[must_use]
pub fn exceeds_hard_limit_len(bytes: usize) -> bool {
    bytes > STABLE_RENDER_MAX_BYTES
}

/// 稳定段是否越过软门槛，用于在装配与诊断处提前预警。
#[must_use]
pub fn exceeds_soft_limit(text: &str) -> bool {
    text.len() > STABLE_RENDER_SOFT_BYTES
}
fn bullet_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {}", item.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn rules_text(core: &PersonaCore) -> String {
    core.rules
        .iter()
        .map(|rule| {
            let mut condition = Vec::new();
            if let Some(mode) = rule.when.mode {
                condition.push(format!("mode={}", mode.as_token()));
            }
            if let Some(audience) = rule.when.audience {
                condition.push(format!("audience={}", audience.as_token()));
            }
            let mut patch = Vec::new();
            if let Some(warmth) = rule.voice.warmth {
                patch.push(format!("warmth={warmth}"));
            }
            if let Some(formality) = rule.voice.formality {
                patch.push(format!("formality={formality}"));
            }
            if let Some(humor) = rule.voice.humor {
                patch.push(format!("humor={}", humor.as_token()));
            }
            if let Some(verbosity) = rule.voice.verbosity {
                patch.push(format!("verbosity={}", verbosity.as_token()));
            }
            format!("- when {} -> {}", condition.join(" "), patch.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}
/// 渲染动态段正文（不含片段标记）。
///
/// 字段集与合同第 5.6 条一致：模式、表达参数、原因与可选纠偏提示。
#[must_use]
pub fn render_dynamic(directive: &ExpressionDirective) -> String {
    let mut lines = vec![
        format!("mode: {}", directive.mode.as_token()),
        format!("voice: {}", directive.voice.render()),
        format!("cause: {}", directive.cause.trim()),
    ];
    if let Some(guard) = directive.guard.as_deref() {
        lines.push(format!("guard: {}", guard.trim()));
    }

    let text = lines.join("\n");
    if text.len() > DYNAMIC_RENDER_MAX_BYTES {
        truncate_to_bytes(&text, DYNAMIC_RENDER_MAX_BYTES)
    } else {
        text
    }
}

/// 按字节上限截断，且保证落在字符边界上。
fn truncate_to_bytes(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
