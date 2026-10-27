//! Agent runtime 的 M0 合同实现。

#[cfg(test)]
pub(crate) fn test_stream_outcome(body: impl AsRef<str>) -> ProviderStreamingTransportOutcome {
    let body = body.as_ref();
    let protocol = if body.contains("content_block_") || body.contains("message_stop") {
        ProviderStreamProtocol::AnthropicMessages
    } else if body.contains(r#""type":"response."#) {
        ProviderStreamProtocol::OpenAiResponses
    } else {
        ProviderStreamProtocol::OpenAiCompatible
    };
    let mut accumulator = ProviderStreamAccumulator::new(protocol, Vec::new());
    for line in body.lines().filter_map(|line| {
        let line = line.trim();
        line.strip_prefix("data: ")
            .filter(|event| *event != "[DONE]")
    }) {
        accumulator.consume(line);
    }
    ProviderStreamingTransportOutcome::AggregatedStream {
        http_status: 200,
        stream: Box::new(accumulator.finish().expect("测试 Provider 事件必须可聚合")),
    }
}

#[path = "agent/agent_apply_history.rs"]
mod agent_apply_history;
#[path = "agent/agent_chat_history.rs"]
mod agent_chat_history;
#[path = "agent/agent_communication.rs"]
mod agent_communication;
#[path = "core/context/agent_context.rs"]
mod agent_context;
#[path = "core/context/agent_context_memory.rs"]
mod agent_context_memory;
#[cfg(test)]
#[path = "core/context/agent_context_memory_layer_tests.rs"]
mod agent_context_memory_layer_tests;
#[path = "core/context/agent_context_memory_layers.rs"]
mod agent_context_memory_layers;
#[path = "core/context/agent_context_redaction.rs"]
mod agent_context_redaction;
#[cfg(test)]
#[path = "core/context/agent_context_tests.rs"]
mod agent_context_tests;
#[path = "agent/agent_control_completion.rs"]
mod agent_control_completion;
#[path = "agent/agent_control_execution.rs"]
mod agent_control_execution;
#[path = "agent/agent_control_graph_sync.rs"]
mod agent_control_graph_sync;
#[path = "agent/agent_control_mailbox.rs"]
mod agent_control_mailbox;
#[path = "agent/agent_control_reservation.rs"]
mod agent_control_reservation;
#[path = "agent/agent_control_restore.rs"]
mod agent_control_restore;
#[path = "agent/agent_control_runtime.rs"]
mod agent_control_runtime;
#[cfg(test)]
#[path = "agent/agent_control_runtime_tests.rs"]
mod agent_control_runtime_tests;
#[path = "agent/agent_graph_file_store.rs"]
mod agent_graph_file_store;
#[path = "agent/agent_graph_store.rs"]
mod agent_graph_store;
#[path = "agent/agent_mailbox_persistence.rs"]
mod agent_mailbox_persistence;
#[path = "agent/agent_model_message.rs"]
mod agent_model_message;
#[path = "agent/agent_model_payload.rs"]
mod agent_model_payload;
#[path = "agent/agent_model_tool_history.rs"]
mod agent_model_tool_history;
#[path = "agent/agent_pane.rs"]
mod agent_pane;
#[path = "agent/agent_prompt_engineering.rs"]
mod agent_prompt_engineering;
#[path = "agent/agent_reply_attachment.rs"]
mod agent_reply_attachment;
#[path = "agent/agent_response_stream.rs"]
mod agent_response_stream;
#[path = "agent/agent_run_hashline_metadata.rs"]
mod agent_run_hashline_metadata;
#[path = "agent/agent_run_hashline_receipt.rs"]
mod agent_run_hashline_receipt;
#[path = "agent/agent_run_model_tools.rs"]
mod agent_run_model_tools;
#[cfg(test)]
#[path = "agent/agent_run_model_tools_tests.rs"]
mod agent_run_model_tools_tests;
#[path = "agent/agent_run_read_file_receipt.rs"]
mod agent_run_read_file_receipt;
#[path = "core/run/agent_run_runtime.rs"]
mod agent_run_runtime;
#[cfg(test)]
#[path = "core/run/agent_run_runtime_tests.rs"]
mod agent_run_runtime_tests;
#[path = "agent/agent_runtime_note.rs"]
mod agent_runtime_note;
#[path = "core/context/environment_context.rs"]
mod environment_context;
#[cfg(test)]
pub(crate) use agent_run_runtime::OUTCOME_VISIBLE_OUTPUT;
#[path = "agent/agent_run_tool_args.rs"]
mod agent_run_tool_args;
#[path = "agent/agent_run_tool_result_wire.rs"]
mod agent_run_tool_result_wire;
#[path = "core/run/agent_runtime_task_lifecycle.rs"]
mod agent_runtime_task_lifecycle;
#[path = "agent/agent_session_stable_context.rs"]
mod agent_session_stable_context;
#[path = "agent/agent_subagent_fork_context.rs"]
mod agent_subagent_fork_context;
#[path = "core/tools/agent_tools.rs"]
mod agent_tools;
#[path = "core/run/agent_turn_runtime.rs"]
mod agent_turn_runtime;
#[path = "core/tools/apply_patch/apply_patch_invocation.rs"]
mod apply_patch_invocation;
#[path = "planning/atomic_commit_planner.rs"]
mod atomic_commit_planner;
#[path = "planning/atomic_commit_planner_support.rs"]
mod atomic_commit_planner_support;
#[path = "attachment_runtime.rs"]
mod attachment_runtime;
#[path = "planning/auto_changelog.rs"]
mod auto_changelog;
#[path = "runtime/background_scheduler.rs"]
mod background_scheduler;
#[path = "core/tools/builtin_ask_user_question_tool.rs"]
pub mod builtin_ask_user_question_tool;
#[path = "core/tools/builtin_edit_tool.rs"]
mod builtin_edit_tool;
#[path = "core/tools/builtin_fs_tools.rs"]
mod builtin_fs_tools;
#[path = "core/tools/builtin_glob_tool.rs"]
mod builtin_glob_tool;
#[path = "core/tools/builtin_goal_tools.rs"]
mod builtin_goal_tools;
#[path = "core/tools/builtin_history_tools.rs"]
mod builtin_history_tools;
#[path = "core/tools/builtin_job_tools.rs"]
mod builtin_job_tools;
#[path = "core/tools/builtin_memory_search_tool.rs"]
mod builtin_memory_search_tool;
#[path = "core/tools/builtin_memory_write_tool.rs"]
mod builtin_memory_write_tool;
#[path = "core/tools/builtin_notes_tools.rs"]
mod builtin_notes_tools;
#[path = "core/tools/builtin_search_engine.rs"]
mod builtin_search_engine;
#[path = "core/tools/builtin_search_tool.rs"]
mod builtin_search_tool;
#[path = "runtime/builtin_shell_script.rs"]
mod builtin_shell_script;
#[path = "core/tools/builtin_shell_tool.rs"]
mod builtin_shell_tool;
#[path = "core/tools/builtin_skill_invoke_tool.rs"]
mod builtin_skill_invoke_tool;
#[path = "core/tools/builtin_subagent_tool_spec.rs"]
mod builtin_subagent_tool_spec;
#[path = "core/tools/builtin_subagent_tools.rs"]
mod builtin_subagent_tools;
#[cfg(test)]
#[path = "core/tools/builtin_subagent_tools_tests.rs"]
mod builtin_subagent_tools_tests;
#[path = "core/tools/builtin_token_budget_tools.rs"]
mod builtin_token_budget_tools;
#[path = "core/tools/builtin_update_plan_tool.rs"]
mod builtin_update_plan_tool;
#[path = "core/tools/builtin_work_contract_tool.rs"]
mod builtin_work_contract_tool;
#[path = "core/context/canonical_checkpoint.rs"]
mod canonical_checkpoint;
#[cfg(test)]
#[path = "core/context/canonical_checkpoint_tests.rs"]
mod canonical_checkpoint_tests;
#[path = "core/routing/channel_balancer.rs"]
mod channel_balancer;
#[path = "core/routing/channel_health.rs"]
mod channel_health;
#[path = "runtime/child_execution_runner.rs"]
mod child_execution_runner;
#[path = "runtime/command_resolver.rs"]
pub mod command_resolver;
#[path = "planning/commit_history_analyzer.rs"]
mod commit_history_analyzer;
mod commit_message_lint;
#[path = "core/tools/content_archive.rs"]
mod content_archive;
pub mod context;
#[path = "core/context/context_assembler.rs"]
mod context_assembler;
#[path = "core/context/context_budget.rs"]
mod context_budget;
#[path = "core/context/context_budget_exports.rs"]
mod context_budget_exports;
#[path = "core/tools/edit_repair.rs"]
mod edit_repair;
mod exports;
#[path = "runtime/verification/fix_strategy.rs"]
mod fix_strategy;
#[path = "core/context/focus_resolver.rs"]
mod focus_resolver;
#[path = "git/git_diff_semantic_analyzer.rs"]
mod git_diff_semantic_analyzer;
#[path = "git/git_diff_semantic_types.rs"]
mod git_diff_semantic_types;
#[path = "core/tools/hashline_edit.rs"]
mod hashline_edit;
#[path = "core/tools/history_archive_query.rs"]
mod history_archive_query;
#[cfg(test)]
#[path = "core/tools/history_archive_query_tests.rs"]
mod history_archive_query_tests;
#[path = "core/context/insert_then_compress.rs"]
mod insert_then_compress;
#[cfg(test)]
#[path = "core/context/insert_then_compress_attachment_tests.rs"]
mod insert_then_compress_attachment_tests;
#[cfg(test)]
#[path = "core/context/insert_then_compress_budget_tests.rs"]
mod insert_then_compress_budget_tests;
#[path = "core/context/insert_then_compress_classification.rs"]
mod insert_then_compress_classification;
#[cfg(test)]
#[path = "core/context/insert_then_compress_tests.rs"]
mod insert_then_compress_tests;
#[path = "runtime/interrupt_validation.rs"]
mod interrupt_validation;
#[path = "runtime/job_runtime/mod.rs"]
pub mod job_runtime;
mod language_registry;
#[path = "runtime/long_running_cell.rs"]
mod long_running_cell;
#[path = "lsp/lsp_background.rs"]
mod lsp_background;
#[path = "lsp/lsp_client_pool.rs"]
mod lsp_client_pool;
#[path = "lsp/lsp_diagnostics.rs"]
mod lsp_diagnostics;
#[path = "lsp/lsp_diagnostics_cluster.rs"]
mod lsp_diagnostics_cluster;
#[path = "lsp/lsp_diagnostics_extended.rs"]
mod lsp_diagnostics_extended;
#[path = "lsp/lsp_diagnostics_projection.rs"]
mod lsp_diagnostics_projection;
#[path = "lsp/lsp_document.rs"]
mod lsp_document;
#[path = "lsp/lsp_document_location.rs"]
mod lsp_document_location;
#[path = "lsp/lsp_launch_plan.rs"]
mod lsp_launch_plan;
#[path = "lsp/lsp_stdio_client.rs"]
mod lsp_stdio_client;
#[path = "lsp/lsp_support.rs"]
mod lsp_support;
#[path = "lsp/lsp_workspace_scan.rs"]
mod lsp_workspace_scan;
#[path = "lsp/lsp_workspace_symbol.rs"]
mod lsp_workspace_symbol;
#[path = "lsp/lsp_write_diagnostics.rs"]
mod lsp_write_diagnostics;
#[path = "mcp/startup.rs"]
mod mcp;
#[path = "mcp/client_capabilities.rs"]
mod mcp_client_capabilities;
#[path = "mcp/completion.rs"]
mod mcp_completion;
#[cfg(test)]
#[path = "mcp/completion_tests.rs"]
mod mcp_completion_tests;
#[path = "mcp/metadata.rs"]
mod mcp_metadata;
#[path = "mcp/notifications.rs"]
mod mcp_notifications;
#[path = "mcp/persistent.rs"]
mod mcp_persistent;
#[path = "mcp/pool.rs"]
mod mcp_pool;
#[path = "mcp/pool_store.rs"]
mod mcp_pool_store;
#[path = "mcp/process.rs"]
mod mcp_process;
#[path = "mcp/prompts.rs"]
mod mcp_prompts;
#[cfg(test)]
#[path = "mcp/prompts_tests.rs"]
mod mcp_prompts_tests;
#[path = "mcp/resources.rs"]
mod mcp_resources;
#[path = "mcp/session.rs"]
mod mcp_session;
#[cfg(test)]
#[path = "mcp/session_tests.rs"]
mod mcp_session_tests;
#[path = "mcp/stdio_launcher.rs"]
mod mcp_stdio_launcher;
#[path = "mcp/tool_naming.rs"]
mod mcp_tool_naming;
#[path = "mcp/tools.rs"]
mod mcp_tools;
#[path = "mcp/transport.rs"]
mod mcp_transport;
#[path = "mcp/transport_auth.rs"]
mod mcp_transport_auth;
#[path = "mcp/transport_completion.rs"]
mod mcp_transport_completion;
#[path = "mcp/transport_http.rs"]
mod mcp_transport_http;
#[path = "mcp/transport_http_client.rs"]
mod mcp_transport_http_client;
#[path = "mcp/transport_oauth.rs"]
mod mcp_transport_oauth;
#[path = "mcp/transport_oauth_discovery.rs"]
mod mcp_transport_oauth_discovery;
#[path = "mcp/transport_oauth_flow.rs"]
mod mcp_transport_oauth_flow;
#[path = "mcp/transport_oauth_registration.rs"]
mod mcp_transport_oauth_registration;
#[path = "mcp/transport_oauth_store.rs"]
mod mcp_transport_oauth_store;
#[path = "mcp/transport_prompts.rs"]
mod mcp_transport_prompts;
#[path = "mcp/transport_resources.rs"]
mod mcp_transport_resources;
#[path = "memory/memory_adapter.rs"]
mod memory_adapter;
#[path = "memory/memory_auto_candidate.rs"]
mod memory_auto_candidate;
#[path = "memory/memory_auto_scheduler.rs"]
mod memory_auto_scheduler;
#[path = "memory/memory_autonomy.rs"]
mod memory_autonomy;
#[path = "memory/memory_code_symbol.rs"]
mod memory_code_symbol;
#[path = "memory/memory_code_symbol_support.rs"]
mod memory_code_symbol_support;
#[path = "memory/memory_code_symbol_types.rs"]
mod memory_code_symbol_types;
#[path = "memory/memory_consistency.rs"]
pub mod memory_consistency;
#[path = "memory/memory_consolidation.rs"]
mod memory_consolidation;
#[path = "memory/memory_consolidation_policy.rs"]
mod memory_consolidation_policy;
pub use memory_consolidation_policy::{MEMORY_DEFAULT_HALF_LIFE_DAYS, decayed_score_basis_points};
#[cfg(test)]
#[path = "memory/memory_consolidation_tests.rs"]
mod memory_consolidation_tests;
#[path = "memory/memory_core_exports.rs"]
mod memory_core_exports;
#[path = "memory/memory_embedding_provider.rs"]
mod memory_embedding_provider;
#[cfg(test)]
#[path = "memory/memory_embedding_provider_tests.rs"]
mod memory_embedding_provider_tests;
#[path = "memory/memory_experience.rs"]
mod memory_experience;
#[path = "memory/memory_governance.rs"]
mod memory_governance;
#[path = "memory/memory_governance_policy.rs"]
mod memory_governance_policy;
#[cfg(test)]
#[path = "memory/memory_projection_reconcile_tests.rs"]
mod memory_projection_reconcile_tests;
#[path = "memory/memory_quality.rs"]
mod memory_quality;
#[path = "memory/memory_receipt.rs"]
mod memory_receipt;
#[path = "memory/memory_recovery.rs"]
pub mod memory_recovery;
#[path = "memory/memory_runtime.rs"]
mod memory_runtime;
#[path = "memory/memory_skill_evolution.rs"]
mod memory_skill_evolution;
#[path = "memory/memory_tool_dispatcher.rs"]
mod memory_tool_dispatcher;
#[path = "memory/memory_write_trigger.rs"]
mod memory_write_trigger;
#[path = "core/routing/model_router.rs"]
mod model_router;
#[path = "core/routing/model_routing_metrics.rs"]
mod model_routing_metrics;
#[path = "core/tools/pinned_skill_executor.rs"]
mod pinned_skill_executor;
#[path = "runtime/prediction.rs"]
mod prediction;
#[path = "runtime/projection_runtime_budget.rs"]
mod projection_runtime_budget;
#[path = "core/context/prompt_cache/prompt_cache.rs"]
mod prompt_cache;
#[path = "core/context/prompt_cache/prompt_cache_plan.rs"]
mod prompt_cache_plan;
#[path = "core/context/prompt_cache/prompt_cache_tool_wire.rs"]
mod prompt_cache_tool_wire;
#[path = "core/context/prompt_cache/prompt_cache_wire.rs"]
mod prompt_cache_wire;
#[path = "provider/provider_stream.rs"]
mod provider_stream;
#[path = "provider/provider_stream_accumulator.rs"]
mod provider_stream_accumulator;
#[path = "provider/provider_stream_error.rs"]
mod provider_stream_error;
#[path = "provider/provider_stream_request.rs"]
mod provider_stream_request;
#[path = "provider/provider_stream_runner.rs"]
mod provider_stream_runner;
#[cfg(test)]
#[path = "provider/provider_stream_runner_tests.rs"]
mod provider_stream_runner_tests;
#[path = "provider/provider_stream_transport.rs"]
mod provider_stream_transport;
#[path = "provider/provider_structured_output.rs"]
mod provider_structured_output;
#[path = "provider/provider_text_tool_call.rs"]
mod provider_text_tool_call;
#[path = "core/tools/provider_tool_call.rs"]
mod provider_tool_call;
#[path = "core/tools/provider_tool_call_stream.rs"]
mod provider_tool_call_stream;
#[path = "core/tools/provider_tool_schedule.rs"]
mod provider_tool_schedule;
#[path = "provider/provider_visible_cache_shape.rs"]
mod provider_visible_cache_shape;
#[path = "core/context/quoted_message_projection.rs"]
mod quoted_message_projection;
#[path = "remote/remote_status_summary.rs"]
mod remote_status_summary;
#[path = "repo_structure/repo_structure_model.rs"]
mod repo_structure_model;
#[path = "repo_structure/repo_structure_model_scan.rs"]
mod repo_structure_model_scan;
#[path = "repo_structure/repo_structure_model_summary.rs"]
mod repo_structure_model_summary;
#[path = "repo_structure/repo_structure_model_types.rs"]
mod repo_structure_model_types;
#[path = "core/run/run_cancel_token.rs"]
mod run_cancel_token;
#[path = "runtime/runtime_cell_runtime.rs"]
mod runtime_cell_runtime;
mod runtime_limits;
#[path = "runtime/runtime_store_path.rs"]
mod runtime_store_path;
#[path = "core/tools/search_index/mod.rs"]
mod search_index;
#[path = "runtime/self_improvement/self_iteration.rs"]
mod self_iteration;
#[path = "runtime/self_improvement/self_iteration_receipt_store.rs"]
mod self_iteration_receipt_store;
#[path = "planning/semantic_commit.rs"]
mod semantic_commit;
#[path = "core/session/session_index.rs"]
mod session_index;
#[path = "core/context/session_notes.rs"]
mod session_notes;
#[path = "core/context/session_notes_runtime.rs"]
mod session_notes_runtime;
#[path = "core/context/session_notes_store.rs"]
mod session_notes_store;
#[cfg(test)]
#[path = "core/context/session_notes_tests.rs"]
mod session_notes_tests;
#[path = "core/tools/shell_command_policy_evaluator.rs"]
mod shell_command_policy_evaluator;
#[path = "runtime/shell_command_static_analyzer.rs"]
mod shell_command_static_analyzer;
#[path = "runtime/shell_command_wrapper_unwrapper.rs"]
mod shell_command_wrapper_unwrapper;
pub mod skills;
#[path = "runtime/snapshot.rs"]
mod snapshot;
#[path = "runtime/self_improvement/subagent_delegation.rs"]
mod subagent_delegation;
#[path = "runtime/self_improvement/subagent_delegation_execution.rs"]
mod subagent_delegation_execution;
#[path = "core/session/subagent_session_runtime.rs"]
mod subagent_session_runtime;
#[path = "core/context/token_budget_guard.rs"]
mod token_budget_guard;
#[path = "core/context/token_estimator.rs"]
mod token_estimator;
#[cfg(test)]
#[path = "core/context/token_estimator_media_tests.rs"]
mod token_estimator_media_tests;
#[path = "runtime/token_usage.rs"]
mod token_usage;
#[path = "runtime/tokenizer.rs"]
mod tokenizer;
#[path = "core/tools/tool_name_resolution.rs"]
mod tool_name_resolution;
#[path = "core/tools/tool_registry.rs"]
mod tool_registry;
#[path = "core/tools/tool_runtime_budget.rs"]
mod tool_runtime_budget;
mod turn_execution_ledger;
#[path = "runtime/turn_metadata.rs"]
mod turn_metadata;
#[path = "runtime/self_improvement/turn_outcome.rs"]
mod turn_outcome;
#[path = "core/run/user_question_limits.rs"]
pub mod user_question_limits;
#[path = "core/run/user_question_port.rs"]
pub mod user_question_port;
#[path = "runtime/vision.rs"]
mod vision;
#[path = "core/context/window_rollover.rs"]
mod window_rollover;
#[path = "core/context/window_rollover_storage.rs"]
mod window_rollover_storage;
#[path = "core/tools/workspace_file_session.rs"]
mod workspace_file_session;
#[path = "core/tools/workspace_walker.rs"]
mod workspace_walker;
pub use builtin_ask_user_question_tool::{
    ASK_USER_QUESTION_DESCRIPTION, ASK_USER_QUESTION_RECEIPT_KIND, ASK_USER_QUESTION_TOOL_ID,
    ASK_USER_QUESTION_TOOL_NAME, ASK_USER_QUESTION_TOOL_QUALIFIED_NAME,
    ASK_USER_QUESTION_TOOL_SCHEMA_VERSION, AskUserQuestionToolInput, AskUserQuestionToolReceipt,
    ask_user_question_tool_definition,
    parse_and_validate_input as parse_and_validate_ask_user_question_input,
};
pub use builtin_work_contract_tool::{
    WORK_CONTRACT_TOOL_ID, WORK_CONTRACT_TOOL_NAME, WORK_CONTRACT_TOOL_QUALIFIED_NAME,
    WORK_CONTRACT_TOOL_SCHEMA_VERSION, WorkContractRuntime, WorkContractToolError,
    apply_work_contract, work_contract_tool_definition,
};
pub use content_archive::{
    CONTENT_ARCHIVE_DEFAULT_FILE_SUFFIX, CONTENT_ARCHIVE_DEFAULT_PREVIEW_CHARS,
    CONTENT_ARCHIVE_DIR_NAME, CONTENT_ARCHIVE_RESULT_STREAM, ContentArchiveRuntime,
    ToolOutputReference, tool_output_source_trace, tool_output_source_trace_from_dispatch_name,
};
pub use exports::*;
pub use mcp_client_capabilities::{
    MCP_CLIENT_CAPABILITY_POLICY_STATUS, McpClientCapabilitiesProjection,
    McpClientCapabilitiesRuntime,
};
pub use mcp_completion::{
    McpCompletionReference, McpCompletionRequest, McpCompletionRuntime,
    McpCompletionRuntimeProjection, McpCompletionTransport,
};
pub use mcp_metadata::{
    McpServerCapabilitiesProjection, McpServerInfoProjection, McpServerMetadataRequest,
    McpServerMetadataRuntime,
};
pub use mcp_transport_completion::{
    McpCompletionProjection, McpCompletionValue, McpStreamableHttpCompletionReport,
};
pub use memory_core_exports::*;
pub use model_router::{
    RunModelSelection, agent_model_slot_candidates, default_run_model_candidates,
    resolve_run_model_profile_id, select_default_run_model_from_config,
};
pub use model_routing_metrics::{
    ChannelMetrics, MetricsDrivenScore, MetricsRouter, RequestSample, channel_metrics_key,
};
pub use projection_runtime_budget::ProjectionRuntimeBudget;
pub use repo_structure_model::{
    RepoEdge, RepoEdgeKind, RepoFile, RepoFileRole, RepoIndexSnapshot, RepoModule, RepoModuleKind,
    RepoSourceTag, RepoStructureModelRuntime, RepoStructureQuery, RepoStructureQueryReport,
    RepoStructureRefreshInput, RepoStructureRefreshPlan, RepoStructureSourceKind, RepoSymbol,
    RepoSymbolKind, RepoWorkspace,
};
pub use runtime_cell_runtime::RuntimeCellRuntime;
pub use runtime_limits::{
    CONTINUATION_ASSEMBLY_BATCH_LIMIT, CONTINUATION_ASSEMBLY_MAX_CHARS, CONTINUATION_RETENTION,
    CONTINUATION_SUMMARY_MAX_CHARS, JOB_SUMMARY_MAX_CHARS, MAILBOX_DELIVERY_BATCH_LIMIT,
    MAILBOX_DELIVERY_MAX_ATTEMPTS, MAILBOX_DELIVERY_RETRY_BACKOFF,
    MAILBOX_DELIVERY_RETRY_MAX_BACKOFF, MAILBOX_HYDRATE_BATCH_LIMIT, MAILBOX_MESSAGE_MAX_CHARS,
    MAILBOX_MESSAGE_RETENTION, SHELL_ARTIFACT_DIR_NAME, SHELL_ARTIFACT_FILE_SUFFIX,
};
pub use semantic_commit::{
    ChangePattern, CommitScope, SemanticCommit, SemanticCommitType, generate_semantic_commit,
    generate_short_commit,
};
pub use session_index::{SessionIndexEntry, SessionIndexRuntime, ThreadId};
pub use snapshot::{DiffEntry, PatchEntry, SnapshotId, SnapshotMetadata, SnapshotRuntime};
pub use tokenizer::{
    AnthropicTokenizer, HeuristicTokenizer, OpenAITokenizer, TokenCount, Tokenizer,
    TokenizerEncoding, TokenizerProvider, TokenizerRouter,
};
pub use tool_name_resolution::{
    ToolNameResolution, provider_custom_tool_name, resolve_provider_tool_name,
    resolve_provider_tool_name_candidates,
};
pub use tool_runtime_budget::{
    ToolBudgetKind, ToolRuntimeBudget, ToolRuntimeBudgetError, tool_runtime_budget,
};
pub use turn_execution_ledger::{
    EventLedgerDecision, ProjectionKey, ToolLedgerDecision, TurnExecutionLedger,
};
pub use turn_metadata::{TurnMetadataBag, TurnRequestKind, WorkspaceGitMetadata};
pub use user_question_limits::*;
pub use user_question_port::{QuestionWaitBudget, QuestionWaitResult, UserQuestionPort};
#[cfg(test)]
#[path = "tests/lib_test_modules.rs"]
mod lib_test_modules;
