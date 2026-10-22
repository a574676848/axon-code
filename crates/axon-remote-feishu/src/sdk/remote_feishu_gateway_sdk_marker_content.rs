//! Feishu SDK 轻结构消息文本抽取。
//!
//! 这些消息没有必要进入媒体下载链，但必须保留可见语义，避免 SDK loop 因未知类型丢事件。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

const FILE_KEY: &str = "file_key";
const FILE_NAME_KEY: &str = "file_name";
const LATITUDE_KEY: &str = "latitude";
const LONGITUDE_KEY: &str = "longitude";
const NAME_KEY: &str = "name";
const OPTIONS_KEY: &str = "options";
const SUMMARY_KEY: &str = "summary";
const TEMPLATE_KEY: &str = "template";
const TEXT_KEY: &str = "text";
const TITLE_KEY: &str = "title";
const TOPIC_KEY: &str = "topic";
const USER_ID_KEY: &str = "user_id";

/// 抽取文件夹卡片的可见文本。
pub(crate) fn parse_folder_content(content: &str) -> AxonResult<String> {
    parse_marker_content(content, "folder", &[FILE_KEY, FILE_NAME_KEY])
}

/// 抽取红包消息的可见文本。
pub(crate) fn parse_hongbao_content(content: &str) -> AxonResult<String> {
    parse_marker_content(content, "hongbao", &[TEXT_KEY])
}

/// 抽取位置消息的可见文本。
pub(crate) fn parse_location_content(content: &str) -> AxonResult<String> {
    parse_marker_content(
        content,
        "location",
        &[NAME_KEY, LATITUDE_KEY, LONGITUDE_KEY],
    )
}

/// 抽取联系人分享消息的可见文本。
pub(crate) fn parse_share_user_content(content: &str) -> AxonResult<String> {
    parse_marker_content(content, "contact", &[USER_ID_KEY])
}

/// 抽取系统消息的可见文本。
pub(crate) fn parse_system_content(content: &str) -> AxonResult<String> {
    let value = parse_json(content, "Feishu system content JSON 无效")?;
    let Some(mut text) = optional_string(&value, TEMPLATE_KEY) else {
        return Ok("[Feishu system]".to_owned());
    };
    if let Value::Object(map) = &value {
        for (key, value) in map {
            if key == TEMPLATE_KEY {
                continue;
            }
            text = text.replace(&format!("{{{key}}}"), &scalar_text(value));
        }
    }
    Ok(text)
}

/// 抽取投票消息的可见文本。
pub(crate) fn parse_vote_content(content: &str) -> AxonResult<String> {
    parse_list_content(content, "vote", TOPIC_KEY, OPTIONS_KEY)
}

/// 抽取视频会议消息的可见文本。
pub(crate) fn parse_video_chat_content(content: &str) -> AxonResult<String> {
    parse_marker_content(content, "meeting", &[TOPIC_KEY])
}

/// 抽取日历消息的可见文本。
pub(crate) fn parse_calendar_content(content: &str) -> AxonResult<String> {
    parse_marker_content(content, "calendar", &[SUMMARY_KEY])
}

/// 抽取待办消息的可见文本。
pub(crate) fn parse_task_content(content: &str) -> AxonResult<String> {
    let value = parse_json(content, "Feishu task content JSON 无效")?;
    let title = value
        .get(SUMMARY_KEY)
        .and_then(|summary| optional_string(summary, TITLE_KEY))
        .or_else(|| optional_string(&value, TITLE_KEY))
        .unwrap_or_else(|| "task".to_owned());
    Ok(format!("[Feishu task]\n{title}"))
}

fn parse_marker_content(content: &str, marker: &str, fields: &[&str]) -> AxonResult<String> {
    let value = parse_json(content, "Feishu marker content JSON 无效")?;
    let attrs = fields
        .iter()
        .filter_map(|field| optional_string(&value, field).map(|text| format!("{field}={text}")))
        .collect::<Vec<_>>();
    if attrs.is_empty() {
        Ok(format!("[Feishu {marker}]"))
    } else {
        Ok(format!("[Feishu {marker}: {}]", attrs.join(", ")))
    }
}

fn parse_list_content(
    content: &str,
    marker: &str,
    title_key: &str,
    items_key: &str,
) -> AxonResult<String> {
    let value = parse_json(content, "Feishu list content JSON 无效")?;
    let mut parts = Vec::new();
    if let Some(title) = optional_string(&value, title_key) {
        parts.push(title);
    }
    if let Some(items) = value.get(items_key).and_then(Value::as_array) {
        parts.extend(
            items
                .iter()
                .map(scalar_text)
                .filter(|text| !text.is_empty()),
        );
    }
    let body = if parts.is_empty() {
        marker.to_owned()
    } else {
        parts.join("\n")
    };
    Ok(format!("[Feishu {marker}]\n{body}"))
}

fn parse_json(content: &str, error: &'static str) -> AxonResult<Value> {
    serde_json::from_str::<Value>(content).map_err(|_| AxonError::runtime_failure(error))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Array(items) => items
            .iter()
            .map(scalar_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}
