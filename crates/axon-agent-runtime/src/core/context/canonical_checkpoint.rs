//! 压缩 checkpoint 的固定合同与提交前完整性门禁。

use std::collections::BTreeSet;

use crate::AgentModelMessage;

/// checkpoint 的 P0 分组标题。
pub const CANONICAL_CHECKPOINT_P0_HEADER: &str = "P0:";
/// checkpoint 的任务字段标题。
pub const CANONICAL_CHECKPOINT_TASK_HEADER: &str = "TASK:";
/// checkpoint 的约束字段标题。
pub const CANONICAL_CHECKPOINT_CONSTRAINTS_HEADER: &str = "CONSTRAINTS:";
/// checkpoint 的已验证事实字段标题。
pub const CANONICAL_CHECKPOINT_VERIFIED_HEADER: &str = "VERIFIED:";
/// checkpoint 的待办字段标题。
pub const CANONICAL_CHECKPOINT_OPEN_HEADER: &str = "OPEN:";
/// checkpoint 的活动工具字段标题。
pub const CANONICAL_CHECKPOINT_TOOLS_HEADER: &str = "TOOLS:";

const CANONICAL_CHECKPOINT_HEADERS: [&str; 6] = [
    CANONICAL_CHECKPOINT_P0_HEADER,
    CANONICAL_CHECKPOINT_TASK_HEADER,
    CANONICAL_CHECKPOINT_CONSTRAINTS_HEADER,
    CANONICAL_CHECKPOINT_VERIFIED_HEADER,
    CANONICAL_CHECKPOINT_OPEN_HEADER,
    CANONICAL_CHECKPOINT_TOOLS_HEADER,
];
const CHECKPOINT_EMPTY_LIST_ITEM: &str = "-";
const CHECKPOINT_NO_ACTIVE_TOOLS: &str = "- none";
const CHECKPOINT_TOOL_ID_PREFIX: &str = "- ID: ";

/// 已通过完整性门禁的 canonical checkpoint。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalCheckpoint {
    summary: String,
}

impl CanonicalCheckpoint {
    /// 解析并校验压缩摘要，确保 replacement history 只接受可恢复的 checkpoint。
    pub fn parse(summary: &str, history: &[AgentModelMessage]) -> Result<Self, String> {
        let summary = summary.trim();
        let fields = CheckpointFields::parse(summary)?;
        for (header, value) in [
            (CANONICAL_CHECKPOINT_TASK_HEADER, fields.task),
            (CANONICAL_CHECKPOINT_CONSTRAINTS_HEADER, fields.constraints),
            (CANONICAL_CHECKPOINT_VERIFIED_HEADER, fields.verified),
            (CANONICAL_CHECKPOINT_OPEN_HEADER, fields.open),
            (CANONICAL_CHECKPOINT_TOOLS_HEADER, fields.tools),
        ] {
            validate_required_field(header, value)?;
        }
        validate_tool_checkpoint(fields.tools, history)?;
        Ok(Self {
            summary: summary.to_owned(),
        })
    }

    /// 返回可写入历史 checkpoint 的原始摘要。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.summary
    }

    /// 取得已校验的摘要所有权。
    #[must_use]
    pub fn into_inner(self) -> String {
        self.summary
    }
}

struct CheckpointFields<'a> {
    task: &'a str,
    constraints: &'a str,
    verified: &'a str,
    open: &'a str,
    tools: &'a str,
}

impl<'a> CheckpointFields<'a> {
    fn parse(summary: &'a str) -> Result<Self, String> {
        let positions = CANONICAL_CHECKPOINT_HEADERS
            .iter()
            .map(|header| unique_header_position(summary, header))
            .collect::<Result<Vec<_>, _>>()?;
        if positions
            .windows(2)
            .any(|pair| pair[0].start >= pair[1].start)
        {
            return Err(
                "checkpoint 固定字段顺序必须为 P0/TASK/CONSTRAINTS/VERIFIED/OPEN/TOOLS".to_owned(),
            );
        }
        Ok(Self {
            task: section_body(summary, positions[1], positions[2]),
            constraints: section_body(summary, positions[2], positions[3]),
            verified: section_body(summary, positions[3], positions[4]),
            open: section_body(summary, positions[4], positions[5]),
            tools: summary[positions[5].content_start..].trim(),
        })
    }
}

#[derive(Clone, Copy)]
struct HeaderPosition {
    start: usize,
    content_start: usize,
}

fn unique_header_position(summary: &str, header: &str) -> Result<HeaderPosition, String> {
    let positions = header_positions(summary, header);
    match positions.as_slice() {
        [position] => Ok(*position),
        [] => Err(format!("checkpoint 缺少固定字段 {header}")),
        _ => Err(format!("checkpoint 固定字段 {header} 重复")),
    }
}

fn header_positions(summary: &str, header: &str) -> Vec<HeaderPosition> {
    let mut positions = Vec::new();
    let mut offset = 0;
    for line in summary.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.trim() == header {
            positions.push(HeaderPosition {
                start: offset,
                content_start: offset.saturating_add(line.len()),
            });
        }
        offset = offset.saturating_add(line.len());
    }
    positions
}

fn section_body(summary: &str, start: HeaderPosition, end: HeaderPosition) -> &str {
    summary[start.content_start..end.start].trim()
}

fn validate_required_field(header: &str, value: &str) -> Result<(), String> {
    let has_content = value
        .lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && line != CHECKPOINT_EMPTY_LIST_ITEM);
    has_content
        .then_some(())
        .ok_or_else(|| format!("checkpoint 固定字段 {header} 不能为空"))
}

fn validate_tool_checkpoint(tools: &str, history: &[AgentModelMessage]) -> Result<(), String> {
    let active_ids = active_tool_call_ids(history)?;
    if active_ids.is_empty() {
        return (tools == CHECKPOINT_NO_ACTIVE_TOOLS)
            .then_some(())
            .ok_or_else(|| "没有活动 tool call 时，checkpoint TOOLS 必须为 - none".to_owned());
    }
    let checkpoint_ids = if tools == CHECKPOINT_NO_ACTIVE_TOOLS {
        BTreeSet::new()
    } else {
        checkpoint_tool_ids(tools)?
    };
    let missing_ids = active_ids
        .difference(&checkpoint_ids)
        .cloned()
        .collect::<Vec<_>>();
    missing_ids.is_empty().then_some(()).ok_or_else(|| {
        format!(
            "checkpoint TOOLS 缺少活动 tool call ID：{}",
            missing_ids.join(", ")
        )
    })
}

fn active_tool_call_ids(history: &[AgentModelMessage]) -> Result<BTreeSet<String>, String> {
    let mut call_ids = BTreeSet::new();
    let mut result_ids = BTreeSet::new();
    for message in history {
        let Some(metadata) = message.provider_metadata.as_ref() else {
            continue;
        };
        for call in metadata.tool_calls.as_deref().unwrap_or_default() {
            let call_id = call.id.trim();
            if call_id.is_empty() {
                return Err("压缩历史含无 ID 的活动 tool call，无法构建 checkpoint".to_owned());
            }
            call_ids.insert(call_id.to_owned());
        }
        if let Some(result) = metadata.tool_result.as_ref() {
            let call_id = result.call_id.trim();
            if !call_id.is_empty() {
                result_ids.insert(call_id.to_owned());
            }
        }
    }
    Ok(call_ids.difference(&result_ids).cloned().collect())
}

fn checkpoint_tool_ids(tools: &str) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::new();
    for line in tools.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(call_id) = line.strip_prefix(CHECKPOINT_TOOL_ID_PREFIX) else {
            return Err("checkpoint TOOLS 的活动项必须使用 - ID: <provider-call-id>".to_owned());
        };
        let call_id = call_id.trim();
        if call_id.is_empty() {
            return Err("checkpoint TOOLS 包含空的活动 tool call ID".to_owned());
        }
        ids.insert(call_id.to_owned());
    }
    (!ids.is_empty())
        .then_some(ids)
        .ok_or_else(|| "checkpoint TOOLS 缺少活动 tool call ID".to_owned())
}
