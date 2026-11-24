//! Composer popup 候选项检索。

use super::candidate::{ComposerPopupItem, ComposerPopupKind, ComposerPopupMentionType};
use crate::r#loop::agent_tui::AgentTuiCatalog;

const EXACT_MATCH_SCORE: i32 = 1_000;
const PREFIX_MATCH_SCORE: i32 = 750;
const SUBSTRING_MATCH_SCORE: i32 = 500;
const FUZZY_MATCH_SCORE: i32 = 250;

pub(super) fn popup_items(
    kind: ComposerPopupKind,
    filter: &str,
    _cwd: &str,
    catalog: &AgentTuiCatalog,
) -> Vec<ComposerPopupItem> {
    match kind {
        ComposerPopupKind::File | ComposerPopupKind::Image | ComposerPopupKind::Path => Vec::new(),
        ComposerPopupKind::Shell => shell_items(filter),
        ComposerPopupKind::Skill => {
            let enabled_skills = catalog
                .skills
                .iter()
                .filter(|skill| !catalog.disabled_skills.contains(*skill))
                .cloned()
                .collect::<Vec<_>>();
            named_items(
                "$",
                ComposerPopupMentionType::Skill,
                filter,
                &enabled_skills,
            )
        }
        ComposerPopupKind::Connector => named_items(
            "%",
            ComposerPopupMentionType::Connector,
            filter,
            &catalog.connectors,
        ),
    }
}

fn shell_items(filter: &str) -> Vec<ComposerPopupItem> {
    named_item_score(&filter.to_ascii_lowercase(), "shell")
        .map(|(score, match_indices)| ComposerPopupItem {
            value: "!shell".to_owned(),
            description: "输入本机 shell 命令".to_owned(),
            insert_text: "!".to_owned(),
            target: "shell://command".to_owned(),
            mention_type: ComposerPopupMentionType::Shell,
            match_indices: match_indices.map(|indices| {
                indices
                    .into_iter()
                    .map(|index| index.saturating_add(1))
                    .collect()
            }),
            score,
        })
        .into_iter()
        .collect()
}

fn named_items(
    sigil: &str,
    mention_type: ComposerPopupMentionType,
    filter: &str,
    values: &[String],
) -> Vec<ComposerPopupItem> {
    let query = filter.trim().to_ascii_lowercase();
    let mut items = values
        .iter()
        .filter_map(|value| {
            named_item_score(&query, value).map(|(score, match_indices)| ComposerPopupItem {
                value: format!("{sigil}{value}"),
                description: mention_type.label().to_owned(),
                insert_text: format!("{sigil}{value} "),
                target: format!("{}://{value}", mention_protocol(sigil)),
                mention_type,
                match_indices,
                score,
            })
        })
        .collect::<Vec<_>>();
    sort_items(items.as_mut_slice());
    items
}

fn mention_protocol(sigil: &str) -> &'static str {
    match sigil {
        "@" => "file",
        "$" => "skill",
        "%" => "connector",
        _ => "mention",
    }
}

fn sort_items(items: &mut [ComposerPopupItem]) {
    items.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.value.cmp(&right.value))
    });
}

fn named_item_score(query: &str, value: &str) -> Option<(i32, Option<Vec<usize>>)> {
    if query.is_empty() {
        return Some((PREFIX_MATCH_SCORE, None));
    }
    let value = value.to_ascii_lowercase();
    if value == query {
        Some((
            EXACT_MATCH_SCORE,
            Some((0..value.chars().count()).collect()),
        ))
    } else if value.starts_with(query) {
        Some((
            PREFIX_MATCH_SCORE,
            Some((0..query.chars().count()).collect()),
        ))
    } else if value.contains(query) {
        Some((
            SUBSTRING_MATCH_SCORE,
            substring_match_indices(query, &value),
        ))
    } else if fuzzy_matches(query, &value) {
        Some((FUZZY_MATCH_SCORE, fuzzy_match_indices(query, &value)))
    } else {
        None
    }
}

fn fuzzy_matches(query: &str, value: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut query_chars = query.chars();
    let Some(mut expected) = query_chars.next() else {
        return true;
    };
    for value_char in value.chars() {
        if value_char == expected {
            match query_chars.next() {
                Some(next) => expected = next,
                None => return true,
            }
        }
    }
    false
}

fn substring_match_indices(query: &str, value: &str) -> Option<Vec<usize>> {
    let byte_start = value.find(query)?;
    let char_start = value[..byte_start].chars().count();
    Some((char_start..char_start + query.chars().count()).collect())
}

fn fuzzy_match_indices(query: &str, value: &str) -> Option<Vec<usize>> {
    if query.is_empty() {
        return None;
    }
    let mut query_chars = query.chars();
    let mut expected = query_chars.next()?;
    let mut indices = Vec::new();
    for (value_index, value_char) in value.chars().enumerate() {
        if value_char == expected {
            indices.push(value_index);
            match query_chars.next() {
                Some(next) => expected = next,
                None => return Some(indices),
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_items_insert_shell_prefix_without_submitting_literal_shell_word() {
        let items = shell_items("sh");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].value, "!shell");
        assert_eq!(items[0].insert_text, "!");
        assert_eq!(items[0].target, "shell://command");
        assert_eq!(items[0].mention_type, ComposerPopupMentionType::Shell);
    }

    #[test]
    fn named_items_support_fuzzy_skill_lookup_and_score_ordering() {
        let items = named_items(
            "$",
            ComposerPopupMentionType::Skill,
            "fd",
            &[
                "openai-docs".to_owned(),
                "frontend-design".to_owned(),
                "frontend-debug".to_owned(),
            ],
        );

        let values = items
            .iter()
            .map(|item| {
                (
                    item.value.as_str(),
                    item.target.as_str(),
                    item.mention_type,
                    item.match_indices.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                (
                    "$frontend-debug",
                    "skill://frontend-debug",
                    ComposerPopupMentionType::Skill,
                    Some(vec![0, 7])
                ),
                (
                    "$frontend-design",
                    "skill://frontend-design",
                    ComposerPopupMentionType::Skill,
                    Some(vec![0, 7])
                )
            ]
        );
    }

    #[test]
    fn named_items_keeps_all_matching_skills_available_for_navigation() {
        let skills = (0..20)
            .map(|index| format!("agent-skill-{index:02}"))
            .collect::<Vec<_>>();

        let items = named_items("$", ComposerPopupMentionType::Skill, "agent", &skills);

        assert_eq!(items.len(), skills.len());
        assert_eq!(
            items.first().map(|item| item.value.as_str()),
            Some("$agent-skill-00")
        );
        assert_eq!(
            items.last().map(|item| item.value.as_str()),
            Some("$agent-skill-19")
        );
        assert!(
            items
                .iter()
                .all(|item| item.match_indices.as_ref() == Some(&vec![0, 1, 2, 3, 4]))
        );
    }
}
