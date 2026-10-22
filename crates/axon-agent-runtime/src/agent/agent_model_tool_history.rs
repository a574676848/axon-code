//! Provider 工具历史归一化。
//!
//! 对齐 Codex ContextManager：发送 Provider 前保证每个 call 都有唯一 output，
//! 删除 orphan/重复 output，并为未结算 call 插入稳定的 aborted output。

use std::{borrow::Cow, collections::HashSet};

use crate::{
    AgentModelMessage, AgentModelMessageKind, AgentModelToolResultMetadata, RetentionClass,
    agent_model_message::{
        AgentModelHistorySnapshot, TOOL_RESULT_SOURCE_REF_PREFIX, tool_result_source_ref,
    },
};

const ABORTED_TOOL_OUTPUT: &str = "aborted";
const ROLE_SYSTEM: &str = "system";
const ROLE_DEVELOPER: &str = "developer";
const ROLE_USER: &str = "user";
const ROLE_ASSISTANT: &str = "assistant";
const ROLE_TOOL: &str = "tool";
const ROLE_FUNCTION: &str = "function";

#[path = "provider_history_reasons.rs"]
mod provider_history_reasons;
pub(crate) use provider_history_reasons::*;

/// Provider 前历史规范化结果。
#[derive(Debug)]
pub(crate) struct ProviderHistoryNormalization<'a> {
    /// 规范化后的消息。
    pub(crate) messages: Cow<'a, [AgentModelMessage]>,
    /// 修复或删除投影的结构化原因。
    pub(crate) reasons: Vec<String>,
}

/// 返回满足 Provider 历史合同的唯一最终规范化结果。
pub(crate) fn normalize_provider_history(
    messages: &[AgentModelMessage],
    supports_vision: bool,
) -> ProviderHistoryNormalization<'_> {
    let paired = normalize_tool_history_pairs(messages);
    let paired_rewritten = matches!(paired, Cow::Owned(_));
    let source = paired.as_ref();
    let latest_checkpoint = source
        .iter()
        .rposition(|message| message.kind == AgentModelMessageKind::ConversationCheckpoint);
    let mut output: Option<Vec<AgentModelMessage>> = None;
    let mut reasons = Vec::new();
    if paired_rewritten {
        push_reason(&mut reasons, PROVIDER_HISTORY_REASON_TOOL_PAIR_REWRITTEN);
    }

    let mut conversation_started = false;
    for (index, original) in source.iter().enumerate() {
        if original.kind == AgentModelMessageKind::ConversationCheckpoint
            && Some(index) != latest_checkpoint
        {
            output.get_or_insert_with(|| source[..index].to_vec());
            push_reason(
                &mut reasons,
                PROVIDER_HISTORY_REASON_CHECKPOINT_DEDUPLICATED,
            );
            continue;
        }

        if has_tool_calls(original) && has_tool_result(original) {
            let mut call_message = original.clone();
            clear_tool_result(&mut call_message);
            clear_tool_result_source_ref(&mut call_message);
            let mut result_message = original.clone();
            clear_tool_calls(&mut result_message);
            result_message.retention_class = RetentionClass::P2Evidence;
            normalize_provider_message(
                &mut call_message,
                &mut conversation_started,
                supports_vision,
                &mut reasons,
            );
            normalize_provider_message(
                &mut result_message,
                &mut conversation_started,
                supports_vision,
                &mut reasons,
            );
            let normalized = output.get_or_insert_with(|| source[..index].to_vec());
            normalized.push(call_message);
            normalized.push(result_message);
            push_reason(&mut reasons, PROVIDER_HISTORY_REASON_TOOL_METADATA_SPLIT);
            continue;
        }

        let (role, _) = provider_role(original, conversation_started);
        if provider_message_requires_normalization(original, role, supports_vision) {
            let mut message = original.clone();
            normalize_provider_message(
                &mut message,
                &mut conversation_started,
                supports_vision,
                &mut reasons,
            );
            output
                .get_or_insert_with(|| source[..index].to_vec())
                .push(message);
        } else {
            if matches!(original.kind, AgentModelMessageKind::Regular)
                && original.source != crate::AgentModelMessageSource::Context
                && !matches!(role, ROLE_SYSTEM | ROLE_DEVELOPER)
            {
                conversation_started = true;
            }
            if let Some(normalized) = output.as_mut() {
                normalized.push(original.clone());
            }
        }
    }

    let messages = output.map_or(paired, Cow::Owned);
    ProviderHistoryNormalization { messages, reasons }
}

/// 返回满足 call/result 配对和 Provider 消息合同的历史。
pub(crate) fn normalize_tool_history(
    messages: &[AgentModelMessage],
) -> Cow<'_, [AgentModelMessage]> {
    normalize_provider_history(messages, true).messages
}

/// 为 provider 可见历史渲染 Codex 风格的稳定 item id 标记。
///
/// 只作用于本次 provider 请求的瞬时投影：与窗口归档使用同一套历史归一化，
/// 因此条目序号与 `history.read_item` 使用的 `item_id` 完全一致；非 assistant
/// 条目在正文尾部追加 `[id: <window_id>:<ordinal>]`。未绑定窗口 ID 时原样返回。
///
/// 缓存安全：`system` 与 `developer` 消息参与 `stable_prefix_hash`（见
/// `prompt_cache_plan`），因此绝不添加标记；只标记 user / tool 等会话历史条目。
#[must_use]
pub fn annotate_history_item_ids(
    snapshot: &AgentModelHistorySnapshot,
    supports_vision: bool,
) -> AgentModelHistorySnapshot {
    let Some(window_id) = snapshot.history_window_id() else {
        return snapshot.clone();
    };
    let normalized = normalize_provider_history(snapshot.as_slice(), supports_vision);
    let mut messages = normalized.messages.into_owned();
    for (ordinal, message) in messages.iter_mut().enumerate() {
        if is_stable_prefix_role(&message.role) || message.content.trim().is_empty() {
            continue;
        }
        message.content.push('\n');
        message
            .content
            .push_str(&crate::history_item_marker(&crate::history_item_id(
                window_id,
                u64::try_from(ordinal).unwrap_or(u64::MAX),
            )));
    }
    AgentModelHistorySnapshot::from(messages).with_history_window_id(Some(window_id.to_owned()))
}

/// 判断角色是否属于稳定前缀；这些消息的内容必须保持逐字节稳定。
fn is_stable_prefix_role(role: &str) -> bool {
    matches!(role, ROLE_ASSISTANT | ROLE_SYSTEM | ROLE_DEVELOPER)
}

/// 仅按模型视觉能力投影单条消息，不执行历史配对或角色归一化。
pub(crate) fn normalize_provider_message_modalities(
    message: &AgentModelMessage,
    supports_vision: bool,
) -> Cow<'_, AgentModelMessage> {
    if message.local_images.is_empty() {
        return Cow::Borrowed(message);
    }
    let (role, _) = provider_role(message, false);
    if supports_vision {
        if message
            .local_images
            .iter()
            .all(|image| provider_image_is_valid(image, role))
        {
            return Cow::Borrowed(message);
        }
        let mut normalized = message.clone();
        normalized
            .local_images
            .retain(|image| provider_image_is_valid(image, role));
        return Cow::Owned(normalized);
    }
    let mut normalized = message.clone();
    omit_unsupported_images(&mut normalized);
    Cow::Owned(normalized)
}

fn normalize_provider_message(
    message: &mut AgentModelMessage,
    conversation_started: &mut bool,
    supports_vision: bool,
    reasons: &mut Vec<String>,
) {
    let (role, phase_normalized) = provider_role(message, *conversation_started);
    if phase_normalized {
        push_reason(reasons, PROVIDER_HISTORY_REASON_PHASE_NORMALIZED);
    }
    if message.role != role {
        role.clone_into(&mut message.role);
        push_reason(reasons, PROVIDER_HISTORY_REASON_ROLE_NORMALIZED);
    }
    normalize_tool_result_ledger_metadata(message, reasons);
    if !matches!(message.kind, AgentModelMessageKind::Regular) && !message.cache_excluded {
        message.cache_excluded = true;
        push_reason(
            reasons,
            PROVIDER_HISTORY_REASON_CONTROL_CACHE_SEGMENT_NORMALIZED,
        );
    }
    if !message.local_images.is_empty() {
        if supports_vision {
            let before = message.local_images.len();
            message
                .local_images
                .retain(|image| provider_image_is_valid(image, role));
            if message.local_images.len() != before {
                push_reason(reasons, PROVIDER_HISTORY_REASON_IMAGE_PROJECTION_REMOVED);
            }
        } else {
            omit_unsupported_images(message);
            push_reason(reasons, PROVIDER_HISTORY_REASON_UNSUPPORTED_IMAGE_OMITTED);
        }
    }

    if let Some(metadata) = message.provider_metadata.as_mut() {
        if metadata.tool_calls.as_ref().is_some_and(Vec::is_empty) {
            metadata.tool_calls = None;
            push_reason(reasons, PROVIDER_HISTORY_REASON_METADATA_NORMALIZED);
        }
        if metadata
            .anthropic_thinking_blocks
            .as_ref()
            .is_some_and(Vec::is_empty)
        {
            metadata.anthropic_thinking_blocks = None;
            push_reason(reasons, PROVIDER_HISTORY_REASON_METADATA_NORMALIZED);
        }
        if metadata
            .openai_reasoning_content
            .as_ref()
            .is_some_and(|content| content.trim().is_empty())
        {
            metadata.openai_reasoning_content = None;
            push_reason(reasons, PROVIDER_HISTORY_REASON_METADATA_NORMALIZED);
        }
        if let Some(blocks) = metadata.anthropic_thinking_blocks.as_mut() {
            for block in blocks {
                let block_type = block.block_type.trim().to_ascii_lowercase();
                if block.block_type != block_type {
                    block.block_type = block_type.clone();
                    push_reason(reasons, PROVIDER_HISTORY_REASON_METADATA_NORMALIZED);
                }
            }
        }
    }
    clear_empty_metadata(message);

    let is_context_prefix = message.source == crate::AgentModelMessageSource::Context;
    let is_control = !matches!(message.kind, AgentModelMessageKind::Regular);
    if !is_context_prefix && !is_control && !matches!(role, ROLE_SYSTEM | ROLE_DEVELOPER) {
        *conversation_started = true;
    }
}

fn canonical_role(role: &str) -> Option<&'static str> {
    match role.trim().to_ascii_lowercase().as_str() {
        ROLE_SYSTEM => Some(ROLE_SYSTEM),
        ROLE_DEVELOPER => Some(ROLE_DEVELOPER),
        ROLE_USER => Some(ROLE_USER),
        ROLE_ASSISTANT => Some(ROLE_ASSISTANT),
        ROLE_TOOL | ROLE_FUNCTION => Some(ROLE_TOOL),
        _ => None,
    }
}

fn provider_role(message: &AgentModelMessage, conversation_started: bool) -> (&'static str, bool) {
    let has_calls = has_tool_calls(message);
    let has_result = has_tool_result(message);
    let has_thinking = message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.anthropic_thinking_blocks.as_ref())
        .is_some_and(|blocks| !blocks.is_empty());
    let mut role = canonical_role(&message.role).unwrap_or(ROLE_USER);
    if has_result {
        role = ROLE_TOOL;
    } else if has_calls || has_thinking {
        role = ROLE_ASSISTANT;
    } else if role == ROLE_TOOL {
        role = ROLE_USER;
    }
    let phase_normalized = conversation_started
        && matches!(role, ROLE_SYSTEM | ROLE_DEVELOPER)
        && message.source != crate::AgentModelMessageSource::Context;
    (
        if phase_normalized { ROLE_USER } else { role },
        phase_normalized,
    )
}

fn provider_message_requires_normalization(
    message: &AgentModelMessage,
    role: &str,
    supports_vision: bool,
) -> bool {
    message.role != role
        || tool_result_ledger_requires_normalization(message)
        || (!matches!(message.kind, AgentModelMessageKind::Regular) && !message.cache_excluded)
        || (!supports_vision && !message.local_images.is_empty())
        || message
            .local_images
            .iter()
            .any(|image| !provider_image_is_valid(image, role))
        || message
            .provider_metadata
            .as_ref()
            .is_some_and(metadata_needs_normalization)
}

fn tool_result_ledger_requires_normalization(message: &AgentModelMessage) -> bool {
    has_tool_result(message)
        && (message.retention_class != RetentionClass::P2Evidence
            || canonical_tool_result_source_ref(message).is_some())
}

fn normalize_tool_result_ledger_metadata(
    message: &mut AgentModelMessage,
    reasons: &mut Vec<String>,
) {
    if !has_tool_result(message) {
        return;
    }
    let source_ref = canonical_tool_result_source_ref(message);
    let mut normalized = false;
    if message.retention_class != RetentionClass::P2Evidence {
        message.retention_class = RetentionClass::P2Evidence;
        normalized = true;
    }
    if let Some(source_ref) = source_ref {
        message.source_ref = Some(source_ref);
        normalized = true;
    }
    if normalized {
        push_reason(
            reasons,
            PROVIDER_HISTORY_REASON_TOOL_RESULT_LEDGER_NORMALIZED,
        );
    }
}

fn canonical_tool_result_source_ref(message: &AgentModelMessage) -> Option<String> {
    let result = tool_result(message)?;
    let canonical = tool_result_source_ref(result.call_id.trim());
    match message.source_ref.as_deref() {
        Some(source_ref)
            if !source_ref.trim().is_empty()
                && (!source_ref.starts_with(TOOL_RESULT_SOURCE_REF_PREFIX)
                    || source_ref == canonical) =>
        {
            None
        }
        _ => Some(canonical),
    }
}

fn clear_tool_result_source_ref(message: &mut AgentModelMessage) {
    if message
        .source_ref
        .as_deref()
        .is_some_and(|source_ref| source_ref.starts_with(TOOL_RESULT_SOURCE_REF_PREFIX))
    {
        message.source_ref = None;
    }
}

fn append_image_omitted_placeholders(content: &mut String, count: usize) {
    for _ in 0..count {
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(IMAGE_CONTENT_OMITTED_PLACEHOLDER);
    }
}

fn omit_unsupported_images(message: &mut AgentModelMessage) {
    let omitted = message.local_images.len();
    message.local_images.clear();
    if let Some(tool_result) = message
        .provider_metadata
        .as_mut()
        .and_then(|metadata| metadata.tool_result.as_mut())
    {
        append_image_omitted_placeholders(&mut tool_result.output, omitted);
    } else {
        append_image_omitted_placeholders(&mut message.content, omitted);
    }
}

fn provider_image_is_valid(image: &crate::AgentModelLocalImageRef, role: &str) -> bool {
    role == ROLE_USER
        && !image.mime_type.trim().is_empty()
        && (!image.path.trim().is_empty()
            || image
                .archive_ref
                .as_deref()
                .is_some_and(|archive_ref| !archive_ref.trim().is_empty()))
}

fn metadata_needs_normalization(m: &crate::AgentModelMessageProviderMetadata) -> bool {
    m.tool_calls.as_ref().is_some_and(Vec::is_empty)
        || m.anthropic_thinking_blocks
            .as_ref()
            .is_some_and(Vec::is_empty)
        || m.openai_reasoning_content
            .as_ref()
            .is_some_and(|content| content.trim().is_empty())
        || m.anthropic_thinking_blocks.as_ref().is_some_and(|blocks| {
            blocks
                .iter()
                .any(|block| block.block_type != block.block_type.trim().to_ascii_lowercase())
        })
        || (m.openai_reasoning_content.is_none()
            && m.tool_calls.is_none()
            && m.tool_result.is_none()
            && m.anthropic_thinking_blocks.is_none())
}

fn has_tool_calls(message: &AgentModelMessage) -> bool {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_ref())
        .is_some_and(|calls| !calls.is_empty())
}

fn has_tool_result(message: &AgentModelMessage) -> bool {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_result.as_ref())
        .is_some()
}

fn clear_tool_calls(message: &mut AgentModelMessage) {
    if let Some(metadata) = message.provider_metadata.as_mut() {
        metadata.tool_calls = None;
    }
    clear_empty_metadata(message);
}

fn clear_tool_result(message: &mut AgentModelMessage) {
    if let Some(metadata) = message.provider_metadata.as_mut() {
        metadata.tool_result = None;
    }
    clear_empty_metadata(message);
}

fn push_reason(reasons: &mut Vec<String>, reason: &'static str) {
    if !reasons.iter().any(|item| item == reason) {
        reasons.push(reason.to_owned());
    }
}

/// 删除最老的非前缀历史消息及其对应工具项，并恢复 Provider 历史配对不变量。
///
/// 与 Codex `ContextManager` 一致，稳定的 system/developer 前缀不会参与裁剪；
/// 工具 call 与 output 按对应项删除，不会连带移除二者之间的独立消息。
pub fn remove_oldest_model_history_unit(messages: &mut Vec<AgentModelMessage>) -> bool {
    let Some(index) = messages
        .iter()
        .position(|message| !matches!(message.role.as_str(), "system" | "developer"))
    else {
        return false;
    };
    let removed = messages.remove(index);
    let call_ids = tool_calls(&removed)
        .iter()
        .map(|call| call.id.trim().to_owned())
        .filter(|call_id| !call_id.is_empty())
        .collect::<Vec<_>>();
    for call_id in call_ids {
        remove_first_tool_result(messages, &call_id);
    }
    if let Some(result) = tool_result(&removed) {
        remove_first_tool_call(messages, result.call_id.trim());
    }
    if let Cow::Owned(normalized) = normalize_tool_history(messages) {
        *messages = normalized;
    }
    true
}

fn normalize_tool_history_pairs(messages: &[AgentModelMessage]) -> Cow<'_, [AgentModelMessage]> {
    let mut call_ids = HashSet::new();
    let mut valid_result_ids = HashSet::new();
    let mut requires_rewrite = false;

    for message in messages {
        for call in tool_calls(message) {
            let call_id = call.id.trim();
            requires_rewrite |= call_id.is_empty()
                || call_id != call.id
                || call.name.trim().is_empty()
                || call.name.trim() != call.name
                || !call_ids.insert(call_id.to_owned());
        }
        if let Some(result) = tool_result(message) {
            let call_id = result.call_id.trim();
            requires_rewrite |= call_id.is_empty()
                || call_id != result.call_id
                || !call_ids.contains(call_id)
                || !valid_result_ids.insert(call_id.to_owned());
        }
    }

    requires_rewrite |= call_ids
        .iter()
        .any(|call_id| !valid_result_ids.contains(call_id));
    if !requires_rewrite {
        return Cow::Borrowed(messages);
    }

    Cow::Owned(rewrite_tool_history(messages, &valid_result_ids))
}

fn remove_first_tool_result(messages: &mut Vec<AgentModelMessage>, call_id: &str) {
    if let Some(index) = messages.iter().position(|message| {
        tool_result(message).is_some_and(|result| result.call_id.trim() == call_id)
    }) {
        messages.remove(index);
    }
}

fn remove_first_tool_call(messages: &mut Vec<AgentModelMessage>, call_id: &str) {
    let Some((message_index, call_index)) =
        messages.iter().enumerate().find_map(|(index, message)| {
            tool_calls(message)
                .iter()
                .position(|call| call.id.trim() == call_id)
                .map(|call_index| (index, call_index))
        })
    else {
        return;
    };
    let mut retained_calls = tool_calls(&messages[message_index]).to_vec();
    retained_calls.remove(call_index);
    set_tool_calls(&mut messages[message_index], retained_calls);
    if empty_message_shell(&messages[message_index]) {
        messages.remove(message_index);
    }
}

fn rewrite_tool_history(
    messages: &[AgentModelMessage],
    valid_result_ids: &HashSet<String>,
) -> Vec<AgentModelMessage> {
    let mut normalized = Vec::with_capacity(messages.len());
    let mut seen_call_ids = HashSet::new();
    let mut seen_result_ids = HashSet::new();

    for message in messages {
        if tool_result(message).is_some() && tool_calls(message).is_empty() {
            append_normalized_result(
                &mut normalized,
                message,
                &seen_call_ids,
                &mut seen_result_ids,
            );
            continue;
        }

        let mut message = message.clone();
        let original_call_count = tool_calls(&message).len();
        let retained_calls = tool_calls(&message)
            .iter()
            .filter_map(|call| {
                let call_id = call.id.trim();
                (!call_id.is_empty()
                    && !call.name.trim().is_empty()
                    && seen_call_ids.insert(call_id.to_owned()))
                .then(|| {
                    let mut retained = call.clone();
                    call_id.clone_into(&mut retained.id);
                    call.name.trim().clone_into(&mut retained.name);
                    retained
                })
            })
            .collect::<Vec<_>>();
        set_tool_calls(&mut message, retained_calls.clone());
        if original_call_count == 0 || !retained_calls.is_empty() || !empty_message_shell(&message)
        {
            normalized.push(message);
        }

        normalized.extend(
            retained_calls
                .into_iter()
                .filter(|call| !valid_result_ids.contains(&call.id))
                .map(|call| AgentModelMessage::tool_result(call.id, ABORTED_TOOL_OUTPUT, false)),
        );
    }
    normalized
}

fn append_normalized_result(
    normalized: &mut Vec<AgentModelMessage>,
    message: &AgentModelMessage,
    seen_call_ids: &HashSet<String>,
    seen_result_ids: &mut HashSet<String>,
) {
    let Some(result) = tool_result(message) else {
        return;
    };
    let call_id = result.call_id.trim();
    if call_id.is_empty()
        || !seen_call_ids.contains(call_id)
        || !seen_result_ids.insert(call_id.to_owned())
    {
        return;
    }
    let mut message = message.clone();
    if let Some(result) = message
        .provider_metadata
        .as_mut()
        .and_then(|metadata| metadata.tool_result.as_mut())
    {
        call_id.clone_into(&mut result.call_id);
    }
    normalized.push(message);
}

fn set_tool_calls(
    message: &mut AgentModelMessage,
    tool_calls: Vec<crate::AgentModelToolCallMetadata>,
) {
    let Some(metadata) = message.provider_metadata.as_mut() else {
        return;
    };
    metadata.tool_calls = (!tool_calls.is_empty()).then_some(tool_calls);
    clear_empty_metadata(message);
}

fn clear_empty_metadata(message: &mut AgentModelMessage) {
    let clear = message.provider_metadata.as_ref().is_some_and(|metadata| {
        metadata.openai_reasoning_content.is_none()
            && metadata.tool_calls.is_none()
            && metadata.tool_result.is_none()
            && metadata.anthropic_thinking_blocks.is_none()
    });
    if clear {
        message.provider_metadata = None;
    }
}

fn empty_message_shell(message: &AgentModelMessage) -> bool {
    message.content.trim().is_empty()
        && message.provider_metadata.is_none()
        && message.local_images.is_empty()
}

fn tool_calls(message: &AgentModelMessage) -> &[crate::AgentModelToolCallMetadata] {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_deref())
        .unwrap_or_default()
}

fn tool_result(message: &AgentModelMessage) -> Option<&AgentModelToolResultMetadata> {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_result.as_ref())
}

#[cfg(test)]
#[path = "agent_model_tool_history_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_model_tool_history_image_tests.rs"]
mod image_tests;
