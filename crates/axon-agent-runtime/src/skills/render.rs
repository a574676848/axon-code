//! Skill rendering helpers.

use super::model::{Skill, SkillLoadOutcome};
use std::collections::BTreeSet;

const DEFAULT_SKILL_METADATA_CHAR_BUDGET: usize = 8_000;
const SKILL_METADATA_CONTEXT_WINDOW_PERCENT: u64 = 2;
const APPROX_CHARS_PER_TOKEN: u64 = 4;
const SKILL_NAME_WEIGHT: usize = 8;
const SKILL_DESCRIPTION_WEIGHT: usize = 4;
const SKILL_CONTENT_WEIGHT: usize = 1;

/// Budget for model-visible skill listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillRenderBudget {
    /// Skill metadata total character budget.
    pub max_chars: usize,
}

impl SkillRenderBudget {
    /// 按 Codex 同类策略使用模型上下文窗口的 2% 作为 Skill metadata 预算。
    #[must_use]
    pub fn for_context_window(context_window: u64) -> Self {
        if context_window == 0 {
            return Self::default();
        }
        let token_budget = context_window
            .saturating_mul(SKILL_METADATA_CONTEXT_WINDOW_PERCENT)
            .saturating_div(100)
            .max(1);
        Self {
            max_chars: usize::try_from(token_budget.saturating_mul(APPROX_CHARS_PER_TOKEN))
                .unwrap_or(usize::MAX),
        }
    }
}

impl Default for SkillRenderBudget {
    fn default() -> Self {
        Self {
            max_chars: DEFAULT_SKILL_METADATA_CHAR_BUDGET,
        }
    }
}

/// Render result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRenderReport {
    /// Model-visible text.
    pub text: String,
    /// Listed skill count.
    pub listed: usize,
    /// Dropped skill count.
    pub dropped: usize,
    /// Load error count.
    pub errors: usize,
}

/// Renderer for available and selected skill content.
#[derive(Debug, Default, Clone, Copy)]
pub struct SkillsRenderer;

impl SkillsRenderer {
    /// Render available skills as a compact instruction block.
    #[must_use]
    pub fn render_available(
        outcome: &SkillLoadOutcome,
        budget: SkillRenderBudget,
    ) -> SkillRenderReport {
        let visible = outcome.skills.iter().collect::<Vec<_>>();
        render_skill_listing(
            outcome,
            "[Available Skills]\nUse builtin.invoke_skill({\"skill_name\":\"<name>\"}) to load the best matching skill instructions for each distinct stage. invoke_skill accepts no task arguments and does not execute the task. After loading, follow the returned instructions with the actual tools. Do not compose skills that execute the same workflow.\n",
            &visible,
            budget,
            render_available_skill,
        )
    }

    /// 按当前请求排序并渲染最相关的可调用 Skill。
    #[must_use]
    pub fn render_relevant(
        outcome: &SkillLoadOutcome,
        query: &str,
        budget: SkillRenderBudget,
    ) -> SkillRenderReport {
        let query_terms = relevance_terms(query);
        if query_terms.is_empty() {
            return empty_report(outcome.errors.len());
        }
        let mut ranked = outcome
            .skills
            .iter()
            .filter_map(|skill| {
                let score = skill_relevance_score(skill, query, &query_terms);
                (score > 0).then_some((score, skill))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.metadata.name.cmp(&right.1.metadata.name))
        });
        if ranked.is_empty() {
            return empty_report(outcome.errors.len());
        }

        let skills = ranked.iter().map(|(_, skill)| *skill).collect::<Vec<_>>();
        render_skill_listing(
            outcome,
            "[Relevant Skills]\n",
            &skills,
            budget,
            render_relevant_skill,
        )
    }

    /// Render a selected skill as model-visible instructions.
    #[must_use]
    pub fn render_selected(skill: &Skill, user_prompt: Option<&str>) -> String {
        let mut text = String::new();
        text.push_str("[Selected Skill]\nstatus=instructions_loaded_task_not_executed\nsource=");
        text.push_str(&skill.metadata.source_ref());
        text.push_str("\nname=");
        text.push_str(&skill.metadata.name);
        text.push_str("\ndescription=");
        text.push_str(&skill.metadata.description);
        text.push_str("\ndigest=");
        text.push_str(&skill.metadata.content_digest);
        text.push('\n');
        text.push_str(skill.content.trim());
        if let Some(prompt) = user_prompt.filter(|prompt| !prompt.trim().is_empty()) {
            text.push_str("\n\n[User Request]\n");
            text.push_str(prompt.trim());
        }
        text
    }
}

fn render_skill_listing(
    outcome: &SkillLoadOutcome,
    header: &str,
    skills: &[&Skill],
    budget: SkillRenderBudget,
    render_entry: fn(&Skill, Option<usize>) -> String,
) -> SkillRenderReport {
    if skills.is_empty() || budget.max_chars == 0 {
        return empty_report(outcome.errors.len());
    }
    let full_entries = skills
        .iter()
        .map(|skill| render_entry(skill, Some(usize::MAX)))
        .collect::<Vec<_>>();
    let full_chars = header
        .chars()
        .count()
        .saturating_add(full_entries.iter().map(|entry| entry.chars().count()).sum());
    if full_chars <= budget.max_chars {
        return listing_report(header, full_entries, skills.len(), outcome.errors.len());
    }

    let minimum_entries = skills
        .iter()
        .map(|skill| render_entry(skill, None))
        .collect::<Vec<_>>();
    let header_chars = header.chars().count();
    let minimum_chars = header_chars.saturating_add(
        minimum_entries
            .iter()
            .map(|entry| entry.chars().count())
            .sum(),
    );
    if minimum_chars <= budget.max_chars {
        let description_chars = budget
            .max_chars
            .saturating_sub(minimum_chars)
            .saturating_div(skills.len());
        let entries = skills
            .iter()
            .map(|skill| render_entry(skill, Some(description_chars)))
            .collect();
        return listing_report(header, entries, skills.len(), outcome.errors.len());
    }

    let mut used = header_chars;
    let mut entries = Vec::new();
    for entry in minimum_entries {
        let entry_chars = entry.chars().count();
        if used.saturating_add(entry_chars) > budget.max_chars {
            continue;
        }
        used = used.saturating_add(entry_chars);
        entries.push(entry);
    }
    listing_report(header, entries, skills.len(), outcome.errors.len())
}

fn listing_report(
    header: &str,
    entries: Vec<String>,
    total: usize,
    errors: usize,
) -> SkillRenderReport {
    let listed = entries.len();
    SkillRenderReport {
        text: format!("{header}{}", entries.concat()),
        listed,
        dropped: total.saturating_sub(listed),
        errors,
    }
}

fn render_available_skill(skill: &Skill, description_chars: Option<usize>) -> String {
    let mut text = format!(
        "- name: {}\n  source: {}\n  context_scope: {}\n  source_scope: {}\n  digest: {}",
        skill.metadata.name,
        skill.metadata.source_ref(),
        skill.metadata.scope.context_scope(),
        skill.metadata.scope,
        skill.metadata.content_digest,
    );
    append_skill_description(&mut text, &skill.metadata.description, description_chars);
    text.push('\n');
    text
}

fn render_relevant_skill(skill: &Skill, description_chars: Option<usize>) -> String {
    let mut text = format!(
        "- name: {}\n  source: {}\n  context_scope: {}\n  source_scope: {}",
        skill.metadata.name,
        skill.metadata.source_ref(),
        skill.metadata.scope.context_scope(),
        skill.metadata.scope,
    );
    append_skill_description(&mut text, &skill.metadata.description, description_chars);
    text.push('\n');
    text
}

fn append_skill_description(text: &mut String, description: &str, max_chars: Option<usize>) {
    let Some(max_chars) = max_chars.filter(|max_chars| *max_chars > 0) else {
        return;
    };
    text.push_str("\n  description: ");
    text.push_str(&bound(description, max_chars));
}

fn empty_report(errors: usize) -> SkillRenderReport {
    SkillRenderReport {
        text: String::new(),
        listed: 0,
        dropped: 0,
        errors,
    }
}

fn skill_relevance_score(skill: &Skill, query: &str, query_terms: &BTreeSet<String>) -> usize {
    let normalized_query = query.to_lowercase();
    let mut score = usize::from(normalized_query.contains(&skill.metadata.name.to_lowercase()))
        * SKILL_NAME_WEIGHT;
    score += overlap_score(
        query_terms,
        &relevance_terms(&skill.metadata.name),
        SKILL_NAME_WEIGHT,
    );
    score += overlap_score(
        query_terms,
        &relevance_terms(&skill.metadata.description),
        SKILL_DESCRIPTION_WEIGHT,
    );
    score
        + overlap_score(
            query_terms,
            &relevance_terms(&skill.content),
            SKILL_CONTENT_WEIGHT,
        )
}

fn overlap_score(
    query_terms: &BTreeSet<String>,
    candidate_terms: &BTreeSet<String>,
    weight: usize,
) -> usize {
    query_terms.intersection(candidate_terms).count() * weight
}

fn relevance_terms(value: &str) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    let mut ascii = String::new();
    let mut non_ascii = Vec::new();
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            flush_non_ascii_terms(&mut terms, &mut non_ascii);
            ascii.push(character);
        } else {
            if !ascii.is_empty() {
                terms.insert(std::mem::take(&mut ascii));
            }
            if character.is_alphanumeric() {
                non_ascii.push(character);
            } else {
                flush_non_ascii_terms(&mut terms, &mut non_ascii);
            }
        }
    }
    if !ascii.is_empty() {
        terms.insert(ascii);
    }
    flush_non_ascii_terms(&mut terms, &mut non_ascii);
    terms
}

fn flush_non_ascii_terms(terms: &mut BTreeSet<String>, buffer: &mut Vec<char>) {
    if buffer.len() == 1 {
        terms.insert(buffer[0].to_string());
    } else {
        for pair in buffer.windows(2) {
            terms.insert(pair.iter().collect());
        }
    }
    buffer.clear();
}

fn bound(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    if limit <= 3 {
        return ".".repeat(limit);
    }
    value.chars().take(limit - 3).collect::<String>() + "..."
}
