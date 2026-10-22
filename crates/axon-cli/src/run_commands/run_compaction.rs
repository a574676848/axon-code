//! 上下文压缩与代际窗口轮转管线（Context Window Rollover）。
//!
//! 达到统一预算边界后归档旧窗口，以结构化 checkpoint 和按需历史检索延续任务。
//! 稳定前缀由 Context Assembler 维护，Workspace 感知内容在 history 后重新装配。

use std::path::Path;

use axon_agent_runtime::{
    AgentModelMessage, CanonicalCheckpoint, CompressionPolicy, PromptCacheProviderWireFormat,
    ProviderStreamProtocol, SessionNotesRuntime, WindowRolloverEngine, WindowRolloverPolicyRuntime,
    WindowRolloverValidationMode,
};
use axon_config::AxonConfig;
use axon_session::{ThreadCompaction, ThreadDetail};

use super::{
    RunCancelToken,
    run_compaction_projection::CompactionTrigger,
    run_execution::now_ms,
    run_session_store::resolved_axon_root,
    run_thread_store::{
        install_compaction_projection, thread_event_sequence_snapshot, thread_model_history_json,
    },
};

pub(super) const COMPACTION_CANCELLED_ERROR: &str = "压缩已取消";

/// 内联轮转的 replacement history 必然携带 checkpoint 交接投影，否则 publish 会被拒绝。
const CHECKPOINT_INJECTED_INTO_NEW_WINDOW: bool = true;

#[path = "run_compaction_idle.rs"]
mod idle;
pub(super) use idle::{cancel_idle_compaction, schedule_idle_compaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CompactionDecision {
    Skipped(&'static str),
    Installed(CompactionInstallReport),
}

/// 一次成功压缩的完整结果；同时是跨 turn 通知账本的投影输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompactionInstallReport {
    pub(super) session_id: String,
    pub(super) before_tokens: u64,
    pub(super) after_tokens: u64,
    pub(super) before_messages: usize,
    pub(super) after_messages: usize,
    pub(super) removed_items: usize,
    pub(super) duration_ms: u64,
    pub(super) window_number: u64,
    pub(super) window_id: String,
    pub(super) previous_window_id: Option<String>,
    pub(super) archived_messages_count: usize,
    pub(super) model_history_json: String,
}

impl CompactionInstallReport {
    /// 窗口轮转的权威协议事实。
    pub(super) fn context_rollover(&self) -> axon_protocol::wire::WireContextRollover {
        super::run_compaction_projection::context_rollover_fact(
            self.window_number,
            self.window_id.clone(),
            self.previous_window_id.clone(),
            self.archived_messages_count,
            CHECKPOINT_INJECTED_INTO_NEW_WINDOW,
        )
    }
}

pub(super) fn compact_loaded_thread(
    config_path: &Path,
    config: &AxonConfig,
    detail: &ThreadDetail,
    trigger: CompactionTrigger,
    cancel_token: Option<&RunCancelToken>,
    selected: super::run_model_routing::ChannelBalancedModel,
) -> Result<CompactionDecision, String> {
    let session_id = detail.thread.thread_id.as_str();
    let root = resolved_axon_root(config_path)?;
    if detail.turns.is_empty() {
        return Ok(CompactionDecision::Skipped("empty_history"));
    }
    let expected_thread_event_sequence = thread_event_sequence_snapshot(&root, session_id)?;
    let history_json = thread_model_history_json(detail)?;
    let history = serde_json::from_str::<Vec<AgentModelMessage>>(&history_json)
        .map_err(|error| format!("待压缩历史 JSON 无效：{error}"))?;

    let policy = CompressionPolicy::for_context_window(
        selected.model.context_window,
        config.agent.auto_compaction_threshold,
    )
    .map_err(|error| format!("构造压缩策略失败：{error:?}"))?
    .ok_or_else(|| "模型未声明有效上下文窗口".to_owned())?;
    let provider_format = prompt_cache_wire_format(&selected.provider.protocol)?;
    let model_tools = crate::runtime_tools::build_execution_tool_registry_for_session(
        Some(config_path),
        Some(Path::new(&detail.thread.settings.cwd)),
        session_id,
    )
    .model_tools_for_provider();
    let runtime = WindowRolloverPolicyRuntime::new(policy).with_provider_wire_capabilities(
        provider_format,
        model_tools.clone(),
        selected.model.supports_vision,
    );
    let provider_window = runtime.provider_visible_window(&history);
    let provider_history = provider_window.messages();
    let session_dir = root.join("sessions").join(session_id);
    let persisted_at_ms = i64::try_from(now_ms()).unwrap_or(i64::MAX);
    if let Some(user_objective) = provider_history
        .iter()
        .find(|message| message.is_real_user_turn())
        .map(|message| message.content.as_str())
    {
        SessionNotesRuntime::initialize_goal_anchor(&session_dir, user_objective, persisted_at_ms)
            .map_err(|error| format!("初始化会话目标锚点失败：{error:?}"))?;
    }
    let persisted_notes = axon_agent_runtime::SessionNotes::load_from_session(&session_dir)
        .map_err(|error| format!("读取会话战术便签失败：{error:?}"))?;
    let notes = persisted_notes.clone().unwrap_or_else(|| {
        axon_agent_runtime::SessionNotes::new(Default::default(), now_ms() as i64)
    });
    let current_window_number = notes.window_number;
    let previous_archive =
        WindowRolloverEngine::read_archive_snapshot(&session_dir, current_window_number)
            .map_err(|error| format!("读取归档回滚快照失败：{error:?}"))?;
    let lifecycle_id = format!(
        "context-rollover:{}:{}:{}",
        trigger.code(),
        session_id,
        current_window_number
    );
    let checkpoint_text = notes
        .try_render_canonical_summary(&session_dir, provider_history)
        .map_err(|error| format!("构造压缩 checkpoint 失败：{error:?}"))?;
    let checkpoint = CanonicalCheckpoint::parse(&checkpoint_text, provider_history)
        .map_err(|error| format!("压缩 checkpoint 未通过完整性门禁：{error:?}"))?;
    let validation_mode = WindowRolloverValidationMode::Automatic;
    SessionNotesRuntime::ensure_state_saved_for_window(&session_dir, current_window_number)
        .map_err(|error| format!("换窗前便签未写入: {error:?}"))?;
    let compacted = runtime
        .build_validated_replacement(&provider_window, &checkpoint, validation_mode)
        .map_err(|error| format!("replacement history 未通过完整性门禁：{error:?}"))?;
    let before_tokens = latest_provider_context_tokens(detail).unwrap_or(provider_window.tokens());
    let after_tokens = compacted.after_tokens;
    let replacement_history_json = serde_json::to_string(&compacted.messages)
        .map_err(|error| format!("压缩历史序列化失败：{error}"))?;
    let through_sequence = detail
        .turns
        .last()
        .map(|turn| turn.sequence)
        .ok_or_else(|| "压缩缺少待覆盖 Turn".to_owned())?;
    let recorded_at_ms = now_ms();
    let compaction = ThreadCompaction {
        history_json: replacement_history_json.clone(),
        through_sequence,
        checkpoint_text,
        before_tokens,
        after_tokens,
        recorded_at_ms,
    };
    let previous_compaction = detail.compacted_history.as_ref();

    ensure_compaction_commit_allowed(cancel_token)?;

    let rollover = WindowRolloverEngine::archive_active_window(
        &session_dir,
        current_window_number,
        notes.current_window_id.clone(),
        notes.revision,
        &lifecycle_id,
        provider_history,
        now_ms() as i64,
    )
    .map_err(|e| format!("执行窗口轮转失败: {e:?}"))?;

    let projection_result = install_compaction_projection(
        config_path,
        &root,
        session_id,
        expected_thread_event_sequence,
        compaction,
        previous_compaction,
    );
    if let Err(error) = projection_result {
        let rollback_result = WindowRolloverEngine::rollback_active_window(
            &session_dir,
            current_window_number,
            &lifecycle_id,
            rollover.notes_revision,
            persisted_notes.as_ref(),
            previous_archive.as_deref(),
        );
        return Err(match rollback_result {
            Ok(()) => format!("Thread 压缩 projection 失败，窗口归档与 notes 已回滚：{error}"),
            Err(rollback_error) => format!(
                "Thread 压缩 projection 失败且窗口回滚失败：projection={error}，rollback={rollback_error:?}"
            ),
        });
    }

    Ok(CompactionDecision::Installed(CompactionInstallReport {
        session_id: session_id.to_owned(),
        before_tokens,
        after_tokens,
        before_messages: provider_history.len(),
        after_messages: compacted.messages.len(),
        removed_items: provider_history
            .len()
            .saturating_sub(compacted.messages.len()),
        duration_ms: rollover.duration_ms,
        window_number: rollover.window_number,
        window_id: rollover.window_id.clone(),
        previous_window_id: rollover.previous_window_id.clone(),
        archived_messages_count: rollover.archived_messages_count,
        model_history_json: replacement_history_json,
    }))
}

fn prompt_cache_wire_format(protocol: &str) -> Result<PromptCacheProviderWireFormat, String> {
    match ProviderStreamProtocol::from_code(protocol)
        .map_err(|error| format!("Provider 协议无效：{error:?}"))?
    {
        ProviderStreamProtocol::OpenAiResponses => {
            Ok(PromptCacheProviderWireFormat::OpenAiResponses)
        }
        ProviderStreamProtocol::OpenAiCompatible => {
            Ok(PromptCacheProviderWireFormat::OpenAiCompatibleChat)
        }
        ProviderStreamProtocol::AnthropicMessages => {
            Ok(PromptCacheProviderWireFormat::AnthropicMessages)
        }
    }
}

pub(super) fn compacted_after_latest_turn(detail: &ThreadDetail) -> bool {
    let Some(compacted) = detail.compacted_history.as_ref() else {
        return false;
    };
    let Some(last_turn) = detail.turns.last() else {
        return true;
    };
    compacted.through_sequence >= last_turn.sequence
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedTurnUsage {
    usage: Option<axon_agent_runtime::TokenUsageTelemetry>,
}

pub(super) fn latest_provider_context_tokens(detail: &ThreadDetail) -> Option<u64> {
    detail
        .turns
        .last()?
        .token_usage_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<PersistedTurnUsage>(json).ok())
        .and_then(|metadata| metadata.usage)
        .map(axon_agent_runtime::TokenUsageTelemetry::current_context_tokens)
        .filter(|tokens| *tokens > 0)
}

pub(super) fn current_context_tokens_for_selection(
    config_path: &Path,
    detail: &ThreadDetail,
    selected: &super::run_model_routing::ChannelBalancedModel,
) -> Result<u64, String> {
    if let Some(tokens) = latest_provider_context_tokens(detail) {
        return Ok(tokens);
    }
    let history_json = thread_model_history_json(detail)?;
    let history = serde_json::from_str::<Vec<AgentModelMessage>>(&history_json)
        .map_err(|error| format!("待估算历史 JSON 无效：{error}"))?;
    let provider_format = prompt_cache_wire_format(&selected.provider.protocol)?;
    let model_tools = crate::runtime_tools::build_execution_tool_registry_for_session(
        Some(config_path),
        Some(Path::new(&detail.thread.settings.cwd)),
        detail.thread.thread_id.as_str(),
    )
    .model_tools_for_provider();
    Ok(
        axon_agent_runtime::estimated_history_tokens_for_provider_with_capabilities(
            &history,
            provider_format,
            &model_tools,
            selected.model.supports_vision,
        ),
    )
}

fn ensure_compaction_commit_allowed(cancel_token: Option<&RunCancelToken>) -> Result<(), String> {
    if cancel_token.is_some_and(|token| token.is_cancelled()) {
        return Err("压缩已取消".to_string());
    }
    Ok(())
}

#[cfg(test)]
#[path = "run_compaction_tests.rs"]
mod tests;
