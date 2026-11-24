//! Skill prompt injection and selection reporting.

use super::model::{Skill, SkillLoadOutcome, SkillScope};
use super::render::SkillsRenderer;

/// How a skill was selected for the current request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSelectionKind {
    /// User referenced `skill://source/name`.
    SkillUri,
    /// User used an explicit skill mention such as `@skill:name`.
    ExplicitMention,
}

/// Structured skill invocation record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInvocationRecord {
    /// 官方 Skill 名称。
    pub skill_name: String,
    /// 稳定来源引用。
    pub source_ref: String,
    /// 选择方式。
    pub kind: SkillSelectionKind,
    /// 来源作用域。
    pub scope: SkillScope,
    /// 本次加载的内容摘要。
    pub content_digest: String,
}

/// Prompt injection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInjectionReport {
    /// 保持干净的用户请求正文。
    pub user_prompt: String,
    /// 选中 skill 的模型可见指令正文。
    pub selected_skill_prompt: Option<String>,
    /// Selected invocation record, if any.
    pub selected: Option<SkillInvocationRecord>,
    /// Non-fatal selection or load warnings.
    pub warnings: Vec<String>,
}

impl SkillInjectionReport {
    /// Selected skill name.
    #[must_use]
    pub fn selected_skill_name(&self) -> Option<&str> {
        self.selected
            .as_ref()
            .map(|record| record.skill_name.as_str())
    }
}

/// Build a prompt injection report from loaded skills and user prompt.
#[must_use]
pub fn build_skill_injection(outcome: &SkillLoadOutcome, prompt: &str) -> SkillInjectionReport {
    let mut warnings = load_warnings(outcome);
    let Some(selection) = parse_explicit_selection(prompt) else {
        return SkillInjectionReport {
            user_prompt: prompt.to_owned(),
            selected_skill_prompt: None,
            selected: None,
            warnings,
        };
    };
    let Some(skill) = find_requested_skill(outcome, &selection.request) else {
        warnings.push(format!("skill not found: {}", selection.request));
        return SkillInjectionReport {
            user_prompt: prompt.to_owned(),
            selected_skill_prompt: None,
            selected: None,
            warnings,
        };
    };
    let selected_skill_prompt = SkillsRenderer::render_selected(&skill, None);
    let selected = SkillInvocationRecord {
        skill_name: skill.metadata.name.clone(),
        source_ref: skill.metadata.source_ref(),
        kind: selection.kind,
        scope: skill.metadata.scope,
        content_digest: skill.metadata.content_digest.clone(),
    };
    SkillInjectionReport {
        user_prompt: selection.remainder,
        selected_skill_prompt: Some(selected_skill_prompt),
        selected: Some(selected),
        warnings,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSelection {
    request: String,
    remainder: String,
    kind: SkillSelectionKind,
}

fn parse_explicit_selection(prompt: &str) -> Option<ParsedSelection> {
    parse_skill_uri(prompt).or_else(|| parse_skill_mention(prompt))
}

fn parse_skill_uri(prompt: &str) -> Option<ParsedSelection> {
    let start = prompt.find("skill://")?;
    let after = &prompt[start..];
    let reference = after
        .split_whitespace()
        .next()?
        .trim_matches([')', ']', ',']);
    let request = reference.rsplit('/').next()?.trim();
    if request.is_empty() {
        return None;
    }
    Some(ParsedSelection {
        request: request.to_owned(),
        remainder: prompt.replace(reference, "").trim().to_owned(),
        kind: SkillSelectionKind::SkillUri,
    })
}

fn parse_skill_mention(prompt: &str) -> Option<ParsedSelection> {
    prompt.split_whitespace().find_map(|token| {
        let request = token
            .strip_prefix("@skill:")
            .or_else(|| token.strip_prefix("@skill/"))?
            .trim_matches([',', '.', ')', ']']);
        (!request.is_empty()).then(|| ParsedSelection {
            request: request.to_owned(),
            remainder: prompt.replace(token, "").trim().to_owned(),
            kind: SkillSelectionKind::ExplicitMention,
        })
    })
}

fn find_requested_skill(outcome: &SkillLoadOutcome, request: &str) -> Option<Skill> {
    outcome
        .skills
        .iter()
        .find(|skill| skill.metadata.name == request)
        .cloned()
}

fn load_warnings(outcome: &SkillLoadOutcome) -> Vec<String> {
    outcome
        .errors
        .iter()
        .map(|error| format!("{}: {}", error.path.display(), error.message))
        .collect()
}
