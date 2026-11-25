//! FreshWindow replacement history 的稳定上下文重装与 token 计量。

use super::WINDOW_ROLLOVER_CHECKPOINT_PREFIX;
#[cfg(test)]
use crate::agent_model_tool_history::normalize_provider_history;
use crate::{
    AgentModelMessage, AgentModelMessageKind, AgentRuntimeNoteKind, CanonicalCheckpoint,
    agent_session_stable_context::is_cross_workspace_stable_message,
};

/// FreshWindow replacement history。
///
/// 只承载消息本身：换窗前后都不再产出 token 读数，任何需要体量的场合一律读
/// Provider 实测（见 `TokenUsageTelemetry::current_context_tokens`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedHistory {
    /// replacement history 消息。
    pub messages: Vec<AgentModelMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FreshWindowContext {
    stable_messages: Vec<AgentModelMessage>,
    active_turn_tail: Vec<AgentModelMessage>,
}

impl FreshWindowContext {
    /// 从已规范化的 provider-visible 历史捕获稳定消息与活动 turn 尾部。
    ///
    /// 调用方必须传入与归档、token 计量同源的规范化历史，本函数不再重复规范化。
    pub(super) fn capture_from_normalized(provider_messages: &[AgentModelMessage]) -> Self {
        let stable_messages = dedup_stable_messages(
            provider_messages
                .iter()
                .filter(|message| is_cross_workspace_stable_message(message)),
        );
        let active_turn_tail = active_turn_tail(provider_messages);
        Self {
            stable_messages,
            active_turn_tail,
        }
    }

    /// 仅供测试：从任意历史捕获上下文。
    #[cfg(test)]
    pub(super) fn capture(messages: &[AgentModelMessage], supports_vision: bool) -> Self {
        let normalized = normalize_provider_history(messages, supports_vision);
        Self::capture_from_normalized(normalized.messages.as_ref())
    }
}

/// 换窗后那条固定运行态说明的正文。
///
/// 两段各答一件事，且都不与 checkpoint 正文重叠：路线说明「怎么把上下文接回来」，
/// 动作段说明「换窗前刚做了什么」。动作段为空时路线照常下发。
fn rollover_note_text(digest: Option<&str>) -> String {
    let mut text = String::from(
        "上一个窗口已归档。回读路线（按需要选一条，不要凭印象补全）：\n\
         - 交接摘要就是 checkpoint.md 原文时，正文已完整带入，不必再整份重读；\n\
         - 摘要只是运行时投影时，正文未内联，先用 builtin.notes_read_file 读 checkpoint.md；\n\
         - 便签不止一份时用 builtin.notes_list_files_by_prefix 列举；\n\
         - 更早的细节用 history 四件套：builtin.history_list_windows 看窗口、builtin.history_list_items 列条目、builtin.history_read_item 读单条、builtin.history_search_contents 搜内容。",
    );
    if let Some(digest) = digest.map(str::trim).filter(|value| !value.is_empty()) {
        text.push_str("\n换窗前最近的动作（运行时回执，按时间先后）：\n");
        text.push_str(digest);
    }
    text
}

/// 稳定前缀按内容去重，保留首次出现。
///
/// 每开一个新窗口都会重新注入一组稳定前缀，而换窗时 `is_cross_workspace_stable_message`
/// 会把此前累积的每一组都判成稳定消息。不去重的话，第 N 个窗口的 replacement 会带上 N 份
/// 几乎相同的 workspace 规则与工具协议：真机窗口 10 实测 84 条里 72 条是 9 组重复，单这一
/// 项就约 114K tokens，新窗口的 baseline 随窗口数线性膨胀，估算水位越往后越早越过换窗线。
///
/// 只折叠逐字段完全相同的消息：内容真的变过（例如协议版本更新）时两组都会留下，宁可多带
/// 一份，也不静默丢掉新的事实。
fn dedup_stable_messages<'a>(
    messages: impl Iterator<Item = &'a AgentModelMessage>,
) -> Vec<AgentModelMessage> {
    let mut unique: Vec<AgentModelMessage> = Vec::new();
    for message in messages {
        let duplicate = unique
            .iter()
            .any(|kept| kept.role == message.role && kept.content == message.content);
        if !duplicate {
            unique.push(message.clone());
        }
    }
    unique
}

/// 换窗后从旧窗口带入的消息范围。
///
/// 只保留本轮内容：紧邻用户请求之前的连续动态注入 + 当前用户请求 + 其后的全部交互。
/// 边界以 `kind` 为准，不再用 `source != Context` 判断，动态注入与稳定前缀现在同为
/// `Context`，沿用 source 边界会把整段旧对话一路扫进来，换窗等于没换。
fn active_turn_tail(messages: &[AgentModelMessage]) -> Vec<AgentModelMessage> {
    let Some(user_index) = messages
        .iter()
        .rposition(AgentModelMessage::is_real_user_turn)
    else {
        return Vec::new();
    };
    let mut start = user_index;
    while start > 0 {
        let candidate = &messages[start - 1];
        // 遇到任何非注入上下文（assistant 回复、工具结果、稳定前缀、交接摘要）立即停：
        // 中间夹 assistant 时也必须在这里截断，不能越过它继续向前捡内容。
        if candidate.kind != AgentModelMessageKind::InjectedContext
            || is_cross_workspace_stable_message(candidate)
        {
            break;
        }
        start -= 1;
    }
    // 右边界停在当前用户请求：换窗发生在 turn 之间，本轮之后的 assistant 回复
    // 已归档进历史，不需要再整段搬进新窗口。
    messages[start..=user_index]
        .iter()
        .filter(|message| !is_cross_workspace_stable_message(message))
        .cloned()
        .collect()
}

pub(super) fn build_fresh_window_history_from_normalized(
    provider_messages: &[AgentModelMessage],
    checkpoint: &CanonicalCheckpoint,
    captured_context: Option<&FreshWindowContext>,
    rollover_digest: Option<&str>,
) -> CompactedHistory {
    let mut messages = captured_context
        .map(|context| context.stable_messages.clone())
        .unwrap_or_else(|| {
            dedup_stable_messages(
                provider_messages
                    .iter()
                    .filter(|message| is_cross_workspace_stable_message(message)),
            )
        });
    messages.push(AgentModelMessage::conversation_checkpoint(format!(
        "{WINDOW_ROLLOVER_CHECKPOINT_PREFIX}\n{}",
        checkpoint.as_str()
    )));
    /*
     * 换窗后的运行态说明恒定下发，并且是「怎么把上下文接回来」这类提示的唯一落点：
     *
     * 1. 交接来源：模型便签原样采用时正文已经在上一条里，重读一遍只是把同一份内容
     *    再花一次工具调用；投影生成时正文没有内联，必须回读。判据写在这里，模型不必试。
     * 2. 最近动作：便签常常只写结论，换窗前跑过什么由运行时按真实回执兜底。
     * 3. 回读路线：便签与历史两套工具名只出现在这一条，投影摘要因此只留事实。
     *
     * 恒定下发而不是「有动作才发」：最后几步恰好都是便签写入与换窗请求时动作段为空，
     * 但路线仍是新窗口唯一拿得到的交接说明，不能跟着一起消失。
     */
    messages.push(AgentModelMessage::runtime_note(
        AgentRuntimeNoteKind::RolloverDigest,
        rollover_note_text(rollover_digest),
    ));
    if let Some(context) = captured_context {
        messages.extend(context.active_turn_tail.clone());
    }
    CompactedHistory { messages }
}
