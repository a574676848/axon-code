//! Feishu SDK 入站文本归一化。
//!
//! 本模块只处理 SDK message payload 中与文本归一化相关的 mention 信息，不写事件源、
//! 不执行治理链，也不依赖真实 SDK crate。

use serde_json::Value;

const ID_OBJECT_KEY: &str = "id";
const NAME_KEY: &str = "name";
const OPEN_ID_KEY: &str = "open_id";
const USER_ID_OBJECT_KEY: &str = "user_id";
const MENTION_HINT_PREFIX: &str = "[Mentioned: ";
const MENTION_HINT_SUFFIX: &str = "]";
const MENTION_BOUNDARY_CHARS: &str = " \t\n\r.,;:!?、，。；：！？()[]{}<>\"'`";
const TRAILING_TERMINAL_PUNCT: &str = " \t\n\r.!?。！？";

/// Feishu SDK payload 文本归一化上下文。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkPayloadTextContext {
    /// 当前 bot open_id。
    pub bot_open_id: Option<String>,
    /// 当前 bot 显示名。
    pub bot_name: Option<String>,
}

impl RemoteFeishuGatewaySdkPayloadTextContext {
    /// 根据 bot 身份创建文本归一化上下文。
    #[must_use]
    pub fn new(bot_open_id: Option<String>, bot_name: Option<String>) -> Self {
        Self {
            bot_open_id: clean_optional(bot_open_id),
            bot_name: clean_optional(bot_name),
        }
    }
}

/// Feishu mention 引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkMentionRef {
    /// 被 mention 用户 open_id。
    pub open_id: String,
    /// 是否为 @all。
    pub is_all: bool,
    name: String,
    is_self: bool,
}

/// 从 SDK mention payload 提取 mention 引用。
#[must_use]
pub fn mention_refs(
    mentions: Option<&Value>,
    text_context: Option<&RemoteFeishuGatewaySdkPayloadTextContext>,
) -> Vec<RemoteFeishuGatewaySdkMentionRef> {
    mentions
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|value| mention_ref(value, text_context))
                .collect::<Vec<RemoteFeishuGatewaySdkMentionRef>>()
        })
        .unwrap_or_default()
}

/// 按 Axon 规则归一化文本。
#[must_use]
pub fn normalize_message_text(
    text: &str,
    mentions: &[RemoteFeishuGatewaySdkMentionRef],
    inject_mention_hint: bool,
) -> String {
    let stripped = strip_edge_self_mentions(text, mentions);
    if stripped.starts_with('/') {
        return stripped;
    }
    if !inject_mention_hint {
        return stripped;
    }
    let Some(hint) = build_mention_hint(mentions) else {
        return stripped;
    };
    if stripped.is_empty() {
        return hint;
    }
    format!("{hint}\n\n{stripped}")
}

fn mention_ref(
    value: &Value,
    text_context: Option<&RemoteFeishuGatewaySdkPayloadTextContext>,
) -> Option<RemoteFeishuGatewaySdkMentionRef> {
    let open_id = mention_open_id(value).unwrap_or_default();
    let name = optional_string(value, NAME_KEY).unwrap_or_default();
    let is_all = mention_is_all(value);
    if open_id.is_empty() && name.is_empty() && !is_all {
        return None;
    }
    let is_self = text_context.is_some_and(|context| {
        context
            .bot_open_id
            .as_deref()
            .is_some_and(|bot_open_id| bot_open_id == open_id)
            || context
                .bot_name
                .as_deref()
                .is_some_and(|bot_name| bot_name == name)
    });
    Some(RemoteFeishuGatewaySdkMentionRef {
        open_id,
        is_all,
        name,
        is_self,
    })
}

fn mention_is_all(value: &Value) -> bool {
    optional_string(value, "key").as_deref() == Some("@_all")
        || optional_string(value, OPEN_ID_KEY).as_deref() == Some("@_all")
}

fn build_mention_hint(mentions: &[RemoteFeishuGatewaySdkMentionRef]) -> Option<String> {
    let mut parts = Vec::<String>::new();
    let mut seen = Vec::<(bool, String, String)>::new();
    for mention in mentions.iter().filter(|mention| !mention.is_self) {
        let signature = (
            mention.is_all,
            mention.open_id.clone(),
            mention.name.clone(),
        );
        if seen.contains(&signature) {
            continue;
        }
        seen.push(signature);
        if mention.is_all {
            parts.push("@all".to_owned());
        } else if !mention.open_id.is_empty() {
            let name = if mention.name.is_empty() {
                "unknown"
            } else {
                mention.name.as_str()
            };
            parts.push(format!("{name} (open_id={})", mention.open_id));
        } else {
            parts.push(if mention.name.is_empty() {
                "unknown".to_owned()
            } else {
                mention.name.clone()
            });
        }
    }
    (!parts.is_empty()).then(|| {
        format!(
            "{MENTION_HINT_PREFIX}{}{MENTION_HINT_SUFFIX}",
            parts.join(", ")
        )
    })
}

fn mention_open_id(value: &Value) -> Option<String> {
    value
        .get(OPEN_ID_KEY)
        .or_else(|| {
            value
                .get(USER_ID_OBJECT_KEY)
                .and_then(|user| user.get(OPEN_ID_KEY))
        })
        .or_else(|| value.get(ID_OBJECT_KEY).and_then(|id| id.get(OPEN_ID_KEY)))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn normalize_self_name(mention: &RemoteFeishuGatewaySdkMentionRef) -> String {
    let label = if mention.name.is_empty() {
        mention.open_id.as_str()
    } else {
        mention.name.as_str()
    };
    format!("@{label}")
}

fn strip_edge_self_mentions(text: &str, mentions: &[RemoteFeishuGatewaySdkMentionRef]) -> String {
    if text.is_empty() {
        return text.to_owned();
    }
    let self_names = mentions
        .iter()
        .filter(|mention| mention.is_self)
        .map(normalize_self_name)
        .collect::<Vec<String>>();
    if self_names.is_empty() {
        return text.to_owned();
    }
    let mut remaining = strip_leading_self_mentions(text.trim_start(), &self_names);
    loop {
        let trimmed_end =
            remaining.trim_end_matches(|value| TRAILING_TERMINAL_PUNCT.contains(value));
        let tail = &remaining[trimmed_end.len()..];
        let Some(name) = self_names
            .iter()
            .find(|name| trimmed_end.ends_with(name.as_str()))
        else {
            return remaining;
        };
        let body = trimmed_end[..trimmed_end.len() - name.len()].trim_end();
        remaining = format!("{body}{tail}");
    }
}

fn strip_leading_self_mentions(text: &str, self_names: &[String]) -> String {
    let mut remaining = text;
    loop {
        let Some(name) = self_names
            .iter()
            .find(|name| remaining.starts_with(name.as_str()))
        else {
            return remaining.to_owned();
        };
        let after = &remaining[name.len()..];
        if after
            .chars()
            .next()
            .is_some_and(|value| !MENTION_BOUNDARY_CHARS.contains(value))
        {
            return remaining.to_owned();
        }
        remaining = after.trim_start();
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
