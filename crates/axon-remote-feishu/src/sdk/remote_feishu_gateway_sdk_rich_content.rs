//! Feishu SDK 富消息文本抽取。
//!
//! 只抽取 Agent 可见的标题、摘要、卡片正文与动作标签，不保留完整富消息结构。

use axon_domain::{AxonError, AxonResult};
use serde_json::{Map, Value};

const TEXT_KEY: &str = "text";
const CONTENT_KEY: &str = "content";
const TITLE_KEY: &str = "title";
const SUMMARY_KEY: &str = "summary";
const PREVIEW_KEY: &str = "preview";
const DESCRIPTION_KEY: &str = "description";
const CHAT_ID_KEY: &str = "chat_id";
const OPEN_CHAT_ID_KEY: &str = "open_chat_id";
const CHAT_NAME_KEY: &str = "chat_name";
const NAME_KEY: &str = "name";
const VALUE_KEY: &str = "value";
const CARD_KEY: &str = "card";
const HEADER_KEY: &str = "header";

/// 抽取合并转发消息的可见文本。
pub(crate) fn parse_merge_forward_content(content: &str) -> AxonResult<String> {
    let value = parse_optional_json_object(content)?;
    let mut parts = Vec::new();
    if let Some(title) = first_text_by_keys(
        &value,
        &[TITLE_KEY, SUMMARY_KEY, PREVIEW_KEY, DESCRIPTION_KEY],
    ) {
        parts.push(title);
    }
    parts.extend(collect_forward_entries(&value).into_iter().take(8));
    Ok(parts.join("\n").if_empty("[Feishu merge_forward]"))
}

/// 抽取群分享消息的可见文本。
pub(crate) fn parse_share_chat_content(content: &str) -> AxonResult<String> {
    let value = parse_optional_json_object(content)?;
    let chat_name = first_text_by_keys(&value, &[CHAT_NAME_KEY, NAME_KEY, TITLE_KEY]);
    let mut parts = vec![match chat_name {
        Some(name) => format!("Shared chat: {name}"),
        None => "[Feishu share_chat]".to_owned(),
    }];
    if let Some(chat_id) = first_text_by_keys(&value, &[CHAT_ID_KEY, OPEN_CHAT_ID_KEY]) {
        parts.push(format!("Chat ID: {chat_id}"));
    }
    Ok(parts.join("\n"))
}

/// 抽取 interactive/card 消息的可见文本。
pub(crate) fn parse_interactive_content(content: &str) -> AxonResult<String> {
    let value = parse_optional_json_object(content)?;
    let card = value.get(CARD_KEY).unwrap_or(&value);
    let mut parts = Vec::new();
    if let Some(title) = header_title(card)
        .or_else(|| first_text_by_keys(card, &[TITLE_KEY, SUMMARY_KEY, "subtitle"]))
    {
        parts.push(title);
    }
    collect_visible_text(card, &mut parts);
    Ok(unique_lines(parts)
        .into_iter()
        .take(12)
        .collect::<Vec<_>>()
        .join("\n")
        .if_empty("[Feishu interactive]"))
}

fn parse_optional_json_object(content: &str) -> AxonResult<Value> {
    if content.trim().is_empty() {
        return Ok(Value::Object(Map::default()));
    }
    serde_json::from_str::<Value>(content)
        .map_err(|_| AxonError::runtime_failure("Feishu rich content JSON 无效"))
}

fn collect_forward_entries(value: &Value) -> Vec<String> {
    let mut entries = Vec::new();
    for key in ["messages", "items", "message_list", "records", "content"] {
        let Some(items) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let sender =
                first_text_by_keys(item, &["sender_name", "user_name", "sender", NAME_KEY]);
            let body = first_text_by_keys(
                item,
                &[TEXT_KEY, SUMMARY_KEY, PREVIEW_KEY, CONTENT_KEY, TITLE_KEY],
            );
            match (sender, body) {
                (Some(sender), Some(body)) => entries.push(format!("- {sender}: {body}")),
                (_, Some(body)) => entries.push(format!("- {body}")),
                _ => {}
            }
        }
    }
    unique_lines(entries)
}

fn header_title(value: &Value) -> Option<String> {
    let title = value.get(HEADER_KEY)?.get(TITLE_KEY)?;
    if let Some(text) = title.as_str() {
        return clean_text(text);
    }
    first_text_by_keys(title, &[CONTENT_KEY, TEXT_KEY, NAME_KEY])
}

fn first_text_by_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(text) = map.get(*key).and_then(Value::as_str).and_then(clean_text) {
                    return Some(text);
                }
            }
            map.values()
                .find_map(|child| first_text_by_keys(child, keys))
        }
        Value::Array(items) => items.iter().find_map(|item| first_text_by_keys(item, keys)),
        Value::String(text) => clean_text(text),
        _ => None,
    }
}

fn collect_visible_text(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for key in [TEXT_KEY, CONTENT_KEY, TITLE_KEY, NAME_KEY, VALUE_KEY] {
                if let Some(text) = map.get(key).and_then(Value::as_str).and_then(clean_text) {
                    parts.push(text);
                }
            }
            for child in map.values() {
                collect_visible_text(child, parts);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_visible_text(item, parts);
            }
        }
        _ => {}
    }
}

fn clean_text(value: &str) -> Option<String> {
    let text = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn unique_lines(lines: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    for line in lines {
        if !unique.contains(&line) {
            unique.push(line);
        }
    }
    unique
}

trait EmptyTextFallback {
    fn if_empty(self, fallback: &str) -> String;
}

impl EmptyTextFallback for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}
