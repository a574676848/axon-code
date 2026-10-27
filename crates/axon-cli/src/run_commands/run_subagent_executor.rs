//! 子代理执行器边界。
//!
//! 默认执行器会把 `spawn_agent` 转成真实 provider child turn，
//! 测试或特殊宿主仍可注入自定义执行器覆盖。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use axon_agent_runtime::{
    AGENT_RUN_ERROR_TYPE_INTERRUPTED, AgentControlRuntime, AgentGraphStore, AgentModelMessage,
    AgentPath, AgentRuntimeNoteKind, ChildExecutionInput, ChildExecutionOutput,
    ChildExecutionRunner, InterAgentCommunication, JobTerminalKind, SubagentSessionRuntime,
    ToolRegistry,
};
#[cfg(test)]
use axon_agent_runtime::{
    AgentStatus, ProviderStructuredOutputContract, ProviderStructuredOutputMode,
    TokenUsageTelemetry,
};
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::PermissionSnapshotId;
use axon_session_runtime::SubagentSessionSinkFactory;

pub(crate) use super::run_subagent_completion::{
    RunSubagentCompletionSink, RunSubagentContinuationSink,
};
use super::{
    RunCancelToken, RunCommand, RunRequest,
    run_execution::ProviderStreamingRunner,
    run_model_routing::{
        FAST_MODEL_SLOT_NAME, resolve_model_profile_id,
        select_channel_balanced_model_for_agent_slot,
    },
    run_progress::RunProgressEventBus,
    run_request::{
        CLI_RUN_DEFAULT_MODEL_PROFILE_ID, hash_session_selector, prepare_model_input_and_tools,
    },
    run_request_builder::{RunRequestAssemblyInput, assemble_run_request},
    run_request_payload::{
        BuildRunModelPayloadInput, RunWorkspaceSnapshotPrewarm, build_run_model_payload,
    },
    run_rollout_store::commit_session_to_rollout,
    run_session_store::{RunSessionPartialInput, RunSessionRecord},
    run_subagent_completion::log_completion_failure,
    run_subagent_executor_support::{ChildModelSelection, child_session_id},
    run_subagent_lifecycle::{
        SubagentLifecyclePublisher, SubagentLifecycleRegistry,
        subagent_explicit_terminal_lifecycle, subagent_started_lifecycle,
        subagent_terminal_lifecycle,
    },
    run_subagent_performance::{
        log_subagent_performance, record_subagent_history_ready, start_subagent_performance,
    },
    run_subagent_progress::{SubagentProviderTurnRunner, run_subagent_provider_turn},
    run_thread_store::{
        mirror_session_started, mirror_session_terminal, mirror_session_to_thread,
        prepare_run_session,
    },
    run_thread_title::derive_thread_title,
};
#[path = "run_subagent_persistence.rs"]
mod child_persistence;
#[path = "run_subagent_executor_helpers.rs"]
mod helpers;
/// 子代理执行输入、输出和状态由 transport-neutral runtime 统一定义。
pub(crate) use axon_agent_runtime::{
    ChildExecutionInput as RunSubagentExecutionInput,
    ChildExecutionModelIntent as RunSubagentModelIntent,
    ChildExecutionOutput as RunSubagentExecutionOutput,
    ChildExecutionStatus as RunSubagentExecutionStatus,
};
pub(super) use child_persistence::{
    SUBAGENT_RESULT_MARKER_TEXT, append_result_path, extract_result_path, persist_subagent_result,
    should_persist_subagent_result, strip_result_path, subagent_archive_root,
    subagent_result_instruction,
};
use child_persistence::{
    persist_child_turn_completed, persist_child_turn_started, persist_child_turn_terminal,
};
use helpers::{child_model_history_json, persisted_memory_write_count, start_child_stop_control};

/// 子代理执行器。
pub(crate) trait RunSubagentExecutor: Send + Sync {
    /// 执行子代理任务。返回 `None` 表示当前运行环境暂不接管执行。
    fn execute(&self, input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput>;

    /// 启动子代理任务。返回 `true` 表示任务已被接管并会异步回写 completion。
    fn start(
        &self,
        input: RunSubagentExecutionInput,
        completion: RunSubagentCompletionSink,
    ) -> bool {
        let Some(output) = self.execute(input.clone()) else {
            return false;
        };
        if let Err(error) = completion.complete(&input, &output) {
            log_completion_failure(&input.task_path, &error);
        }
        true
    }

    /// 记录子代理的显式终态（`job_kill` / 墙钟超时 / 空闲超时统一走这里）。
    fn notify_explicit_terminal(&self, _task_paths: &[AgentPath], _terminal: JobTerminalKind) {}
}

/// provider child turn 执行所需的父运行环境快照。
#[derive(Clone)]
pub(crate) struct ProviderRunSubagentEnvironment {
    pub(crate) config: axon_config::AxonConfig,
    pub(crate) config_path: std::path::PathBuf,
    pub(crate) parent_session_id: String,
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) provider: ProviderConfig,
    pub(crate) model: ModelConfig,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) channel_chain: Vec<(ModelConfig, ProviderConfig)>,
    pub(crate) routing_key: String,
    pub(crate) balancer: ChannelBalancerConfig,
    pub(crate) timeout: std::time::Duration,
    pub(crate) permission_snapshot_id: PermissionSnapshotId,
    pub(crate) startup: Arc<super::RunStartupSnapshot>,
    pub(crate) cancel_token: Option<super::RunCancelToken>,
    pub(crate) workspace_files: Arc<axon_agent_runtime::WorkspaceFileSession>,
    pub(crate) agent_control: Arc<Mutex<AgentControlRuntime>>,
    pub(crate) agent_graph_store: Arc<dyn AgentGraphStore>,
    pub(crate) subagent_sessions: Arc<Mutex<SubagentSessionRuntime>>,
    pub(crate) subagent_event_bus: Option<RunProgressEventBus>,
    /// 宿主提供的子代理会话 sink 工厂；子代理逐字稿据此落进它自己的会话事件流。
    pub(crate) subagent_session_sinks: Option<Arc<dyn SubagentSessionSinkFactory>>,
    pub(crate) subagent_cancel_tokens: Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>,
}

/// 只属于主 turn 的工具：计划与结构化决定是主 turn 的决策权。
///
/// 子代理只执行被委托的工作，其 Provider payload 必须过滤这些工具，
/// 与不装配 `<tool_orchestration_protocol>` 片段保持同一语义。
const MAIN_TURN_ONLY_TOOL_QUALIFIED_NAMES: &[&str] = &[
    axon_agent_runtime::UPDATE_PLAN_TOOL_QUALIFIED_NAME,
    axon_agent_runtime::ASK_USER_QUESTION_TOOL_QUALIFIED_NAME,
];

/// 默认 provider 子代理执行器。
#[derive(Clone)]
pub(crate) struct ProviderRunSubagentExecutor {
    environment: ProviderRunSubagentEnvironment,
    lifecycle_registry: SubagentLifecycleRegistry,
    provider_turn_runner: SubagentProviderTurnRunner,
}

impl ProviderRunSubagentExecutor {
    pub(crate) fn new(environment: ProviderRunSubagentEnvironment) -> Self {
        let lifecycle_registry = SubagentLifecycleRegistry::new(
            environment.subagent_event_bus.clone(),
            environment.parent_session_id.clone(),
        );
        Self {
            environment,
            lifecycle_registry,
            provider_turn_runner: ProviderStreamingRunner::run,
        }
    }

    #[cfg(test)]
    fn with_provider_turn_runner(
        environment: ProviderRunSubagentEnvironment,
        provider_turn_runner: SubagentProviderTurnRunner,
    ) -> Self {
        let mut executor = Self::new(environment);
        executor.provider_turn_runner = provider_turn_runner;
        executor
    }

    fn child_request(
        &self,
        input: &RunSubagentExecutionInput,
        cancel_token: Option<RunCancelToken>,
    ) -> Result<RunRequest, String> {
        let performance = start_subagent_performance();
        let workspace_snapshot_prewarm = RunWorkspaceSnapshotPrewarm::spawn(
            self.environment.config_path.clone(),
            self.environment.cwd.clone(),
            performance.clone(),
        );
        let session_id = child_session_id(&self.environment.parent_session_id, &input.task_path);
        let model_selection = self.child_model_selection(input, &session_id)?;
        let command = RunCommand {
            prompt: input.message.clone(),
            model: Some(model_selection.model.name.clone()),
            reasoning_effort: model_selection.reasoning_effort.clone(),
            format: None,
            continue_last: false,
            session: Some(session_id.clone()),
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let previous_session = prepare_run_session(&self.environment.config_path, &command, None)?;
        record_subagent_history_ready(performance.as_deref());
        let fork_history = child_model_history_json(input)?;
        let model_history_json = previous_session
            .as_ref()
            .map(|session| session.model_history_json.clone())
            .or(fork_history);
        let subagent_stable_prefix = &self
            .environment
            .startup
            .prompt_context
            .subagent_stable_prefix;
        let runtime_input = BuildRunModelPayloadInput {
            config_path: &self.environment.config_path,
            client_channel: axon_client_identity::ClientChannel::Cli,
            config: &self.environment.config,
            cwd: &self.environment.cwd,
            command: &command,
            startup: &self.environment.startup,
            stable_prefix: subagent_stable_prefix,
            prompt_for_model: &input.message,
            session_id: &session_id,
            model_history_json: model_history_json.as_deref(),
            attached_files: &[],
            supports_vision: model_selection.model.supports_vision,
            context_window: model_selection.model.context_window,
            include_user_soul: false,
            include_personal_memory: false,
            memory_disclosure_policy: input.memory_disclosure_policy(),
            performance: performance.as_deref(),
            workspace_snapshot_prewarm: Some(workspace_snapshot_prewarm),
        };
        let (mut model_input, execution_tools) = if input.tools_enabled {
            prepare_model_input_and_tools(&runtime_input)?
        } else {
            let registry = ToolRegistry::new();
            let model_input = build_run_model_payload(&runtime_input, &registry)?;
            (model_input, Arc::new(RwLock::new(registry)))
        };
        // 计划、结构化决定与 Goal 账本都是主 turn 的决策权：子代理只执行被委托的工作，
        // 绝不能提交 update_plan 或 builtin.goal_*。
        model_input.model_payload.model_tools.retain(|tool| {
            !MAIN_TURN_ONLY_TOOL_QUALIFIED_NAMES.contains(&tool.qualified_name.as_str())
                && !axon_agent_runtime::GoalToolRuntime::is_goal_tool(&tool.qualified_name)
        });
        if !input.task_path.is_root() {
            model_input.model_payload.model_tools.retain(|tool| {
                tool.qualified_name
                    != format!(
                        "{}.{}",
                        axon_agent_runtime::AGENT_TOOL_NAMESPACE,
                        axon_agent_runtime::SPAWN_AGENT_TOOL_NAME
                    )
            });
        }
        let result_instruction = input.structured_output.is_none().then(|| {
            // 子代理的父代理就是它路径的上一级；root 之下没有父路径时退回自身，
            // 保证指引里始终有一个可寻址的引用。
            let parent = input
                .task_path
                .parent()
                .unwrap_or_else(|| input.task_path.clone());
            subagent_result_instruction(parent)
        });
        let system_message = model_input
            .model_payload
            .messages
            .iter_mut()
            .find(|message| message.role == "system")
            .ok_or_else(|| "普通 coding payload 缺少 system prompt".to_owned())?;
        if let Some(system_prompt) = input.system_prompt_override.as_deref() {
            let content = result_instruction.as_deref().map_or_else(
                || system_prompt.to_owned(),
                |instruction| format!("{system_prompt}\n\n{instruction}"),
            );
            *system_message = AgentModelMessage::text("system", content);
        } else if let Some(instruction) = result_instruction.as_deref() {
            system_message
                .content
                .push_str(&format!("\n\n{instruction}"));
        }
        // 协作消息消费点（合同 §5.3.5 / M3 设计 §7.5）：在构造 provider 请求之前，
        // 把该子代理 mailbox 里等待阅读的消息取走并注入为 runtime note。
        //
        // 位置必须在 assemble_run_request 之前：runtime note 是 cache-excluded 消息，
        // 请求组装（含 prompt cache 计划）必须看到它，否则注入会游离在计划之外。
        model_input
            .model_payload
            .messages
            .extend(self.drain_mailbox_notes(&input.task_path)?);
        let model_profile_id = resolve_model_profile_id(
            &self.environment.config,
            &model_selection.provider.name,
            &model_selection.model.name,
        )
        .unwrap_or(CLI_RUN_DEFAULT_MODEL_PROFILE_ID);

        Ok(assemble_run_request(RunRequestAssemblyInput {
            client_channel: axon_client_identity::ClientChannel::Cli,
            config_path: self.environment.config_path.clone(),
            session_id,
            resumed_from: previous_session.map(|session| session.record.session_id),
            parent_session_id: Some(self.environment.parent_session_id.clone()),
            cwd: self.environment.cwd.clone(),
            prompt: input.message.clone(),
            structured_output: input.structured_output.clone(),
            model_input,
            startup: Arc::clone(&self.environment.startup),
            execution_tools,
            workspace_files: Arc::clone(&self.environment.workspace_files),
            provider: model_selection.provider,
            model: model_selection.model,
            reasoning_effort: model_selection.reasoning_effort,
            channel_chain: model_selection.channel_chain,
            routing_key: model_selection.routing_key,
            balancer: self.environment.balancer,
            timeout: input
                .max_runtime_ms
                .map(std::time::Duration::from_millis)
                .map_or(self.environment.timeout, |limit| {
                    limit.min(self.environment.timeout)
                }),
            cancel_token,
            model_profile_id,
            permission_snapshot_id: self.environment.permission_snapshot_id,
            current_agent_path: input.task_path.clone(),
            agent_control: Arc::clone(&self.environment.agent_control),
            agent_graph_store: self.environment.agent_graph_store.clone(),
            subagent_sessions: self.environment.subagent_sessions.clone(),
            subagent_executor: Arc::new(self.clone()),
            subagent_cancel_tokens: self.environment.subagent_cancel_tokens.clone(),
            steer_inbox: None,
            auto_compaction_threshold: self.environment.config.agent.auto_compaction_threshold,
            performance,
        }))
    }

    fn child_model_selection(
        &self,
        input: &RunSubagentExecutionInput,
        session_id: &str,
    ) -> Result<ChildModelSelection, String> {
        match input.model_intent {
            RunSubagentModelIntent::Inherit => Ok(ChildModelSelection {
                provider: self.environment.provider.clone(),
                model: self.environment.model.clone(),
                channel_chain: self.environment.channel_chain.clone(),
                routing_key: self.environment.routing_key.clone(),
                reasoning_effort: self.environment.reasoning_effort.clone(),
            }),
            RunSubagentModelIntent::FastDelegate => self.agent_slot_child_model_selection(
                &self.environment.config.agent.fast_model,
                FAST_MODEL_SLOT_NAME,
                session_id,
            ),
        }
    }

    fn agent_slot_child_model_selection(
        &self,
        slot: &str,
        slot_name: &str,
        session_id: &str,
    ) -> Result<ChildModelSelection, String> {
        let balanced = select_channel_balanced_model_for_agent_slot(
            &self.environment.config,
            slot,
            slot_name,
            session_id,
            hash_session_selector(session_id),
            crate::tool_orchestrator::now_ms(),
        )?;
        Ok(ChildModelSelection {
            provider: balanced.provider,
            model: balanced.model,
            channel_chain: balanced.chain,
            routing_key: balanced.routing_key,
            reasoning_effort: None,
        })
    }

    fn execute_child_request(
        &self,
        child_request: &RunRequest,
        input: &RunSubagentExecutionInput,
        publisher: &SubagentLifecyclePublisher,
    ) -> RunSubagentExecutionOutput {
        persist_child_turn_started(child_request);
        let _stop_control = match start_child_stop_control(child_request) {
            Ok(control) => Some(control),
            Err(error) => {
                axon_logger::warning(&format!(
                    "子代理 run-control 不可用，继续使用运行时取消令牌: session_id={}, error={error}",
                    child_request.session_id
                ));
                None
            }
        };
        let model = model_display_name(&child_request.model);
        let output = match run_subagent_provider_turn(
            child_request,
            &input.task_path,
            input.started_at_ms,
            &model,
            publisher,
            self.provider_turn_runner,
            self.environment.subagent_session_sinks.clone(),
        ) {
            Ok(response) => {
                // 空结果守卫：完成却既无文本也无任何工具回执（且非 max-tokens 截断），
                // 视为失败而非静默放行：父代理不得把空结果误读为任务完成。
                let empty_result = response.content.trim().is_empty()
                    && response.tool_receipts.is_empty()
                    && !response.max_tokens_truncated;
                if empty_result {
                    axon_logger::warning(&format!(
                        "子代理完成但未产生任何结果（空内容且无工具调用），标记为失败: session_id={}, task_path={}",
                        child_request.session_id, input.task_path
                    ));
                    persist_child_turn_terminal(
                        child_request,
                        "errored",
                        "子代理完成但未产生任何结果（空内容且无工具调用）",
                        None,
                    );
                    RunSubagentExecutionOutput {
                        status: RunSubagentExecutionStatus::Errored,
                        summary:
                            "子代理完成但未产生任何结果（空内容且无工具调用）。常见原因：通道回退后模型返回空响应。"
                                .to_owned(),
                        memory_write_count: 0,
                        token_usage: response.token_usage,
                        model: Some(model),
                        budget_exhausted: false,
                    }
                } else {
                    persist_child_turn_completed(child_request, &response);
                    let memory_write_count = persisted_memory_write_count(&response);
                    RunSubagentExecutionOutput {
                        status: RunSubagentExecutionStatus::Completed,
                        summary: response.content,
                        memory_write_count,
                        token_usage: response.token_usage,
                        model: Some(model),
                        budget_exhausted: response.max_tokens_truncated,
                    }
                }
            }
            Err(error) if error.error_type == AGENT_RUN_ERROR_TYPE_INTERRUPTED => {
                persist_child_turn_terminal(child_request, "interrupted", &error.message, None);
                RunSubagentExecutionOutput {
                    status: RunSubagentExecutionStatus::Interrupted,
                    summary: error.message,
                    memory_write_count: 0,
                    token_usage: error.token_usage,
                    model: Some(model),
                    budget_exhausted: false,
                }
            }
            Err(error) => {
                let failure = super::run_session_store::runtime_failure_from_execution_error(
                    &child_request.session_id,
                    error.as_ref(),
                    "",
                    super::run_execution::now_ms(),
                );
                persist_child_turn_terminal(
                    child_request,
                    "errored",
                    &error.message,
                    Some(&failure),
                );
                RunSubagentExecutionOutput {
                    status: RunSubagentExecutionStatus::Errored,
                    summary: error.message,
                    memory_write_count: 0,
                    token_usage: error.token_usage,
                    model: Some(model),
                    budget_exhausted: false,
                }
            }
        };
        log_subagent_performance(child_request.performance.as_deref());
        output
    }

    fn execute_sync(&self, input: &RunSubagentExecutionInput) -> RunSubagentExecutionOutput {
        let publisher = self.lifecycle_registry.begin_execution(&input.task_path);
        let model = self.expected_child_model_display_name(input);
        let _ = publisher.publish(subagent_started_lifecycle(input, model.as_deref()));
        let cancel_token = self
            .environment
            .cancel_token
            .as_ref()
            .map(RunCancelToken::child_token)
            .unwrap_or_default();
        let child_request = match self.child_request(input, Some(cancel_token)) {
            Ok(child_request) => child_request,
            Err(error) => {
                let output = self.attach_result_artifact(
                    input,
                    RunSubagentExecutionOutput {
                        status: RunSubagentExecutionStatus::Errored,
                        summary: error,
                        memory_write_count: 0,
                        token_usage: None,
                        model: None,
                        budget_exhausted: false,
                    },
                );
                let _ = publisher.publish_terminal(subagent_terminal_lifecycle(
                    input,
                    &output,
                    &output.agent_status(),
                ));
                self.lifecycle_registry.remove(&input.task_path, &publisher);
                return output;
            }
        };
        let output = self.attach_result_artifact(
            input,
            self.execute_child_request(&child_request, input, &publisher),
        );
        let _ = publisher.publish_terminal(subagent_terminal_lifecycle(
            input,
            &output,
            &output.agent_status(),
        ));
        self.lifecycle_registry.remove(&input.task_path, &publisher);
        output
    }

    fn expected_child_model_display_name(
        &self,
        input: &RunSubagentExecutionInput,
    ) -> Option<String> {
        let session_id = child_session_id(&self.environment.parent_session_id, &input.task_path);
        self.child_model_selection(input, &session_id)
            .ok()
            .map(|selection| model_display_name(&selection.model))
    }

    /// 取走子代理 mailbox 的待读消息并渲染为 runtime note。
    ///
    /// 取走即读确认（`take_mailbox_messages`），因此同一批消息不会被第二次注入；
    /// 没有消息时返回空向量，不产生任何注入。
    fn drain_mailbox_notes(&self, task_path: &AgentPath) -> Result<Vec<AgentModelMessage>, String> {
        let messages = self
            .environment
            .agent_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take_mailbox_messages(task_path, crate::tool_orchestrator::now_ms())
            .map_err(|error| format!("子代理 mailbox 消费失败：{error:?}"))?;
        Ok(messages
            .into_iter()
            .map(|message| {
                AgentModelMessage::runtime_note(
                    AgentRuntimeNoteKind::AgentMailbox,
                    agent_mailbox_note(&message),
                )
            })
            .collect())
    }

    fn attach_result_artifact(
        &self,
        input: &RunSubagentExecutionInput,
        mut output: RunSubagentExecutionOutput,
    ) -> RunSubagentExecutionOutput {
        if should_persist_subagent_result(&output.summary) {
            let archived = subagent_archive_root(&self.environment.config_path).and_then(|root| {
                persist_subagent_result(&root, &input.task_path, input.started_at_ms, &output)
            });
            match archived {
                Ok(result) => output.summary = append_result_path(&result.preview, &result.path),
                Err(error) => axon_logger::warning(&format!(
                    "subagent result persistence skipped: task_path={}, error={error}",
                    input.task_path
                )),
            }
        }
        output
    }
}

impl RunSubagentExecutor for ProviderRunSubagentExecutor {
    fn execute(&self, input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        self.run(input)
    }

    fn start(
        &self,
        input: RunSubagentExecutionInput,
        completion: RunSubagentCompletionSink,
    ) -> bool {
        let executor = self.clone();
        let publisher = self.lifecycle_registry.begin_execution(&input.task_path);
        let cancel_token = self
            .environment
            .cancel_token
            .as_ref()
            .map(RunCancelToken::child_token)
            .unwrap_or_default();
        let completion = completion
            .with_lifecycle(self.lifecycle_registry.clone(), publisher.clone())
            .with_cancel_token(
                self.environment.subagent_cancel_tokens.clone(),
                cancel_token.clone(),
            );
        self.environment
            .subagent_cancel_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(input.task_path.clone(), cancel_token.clone());
        let model = self.expected_child_model_display_name(&input);
        let _ = publisher.publish(subagent_started_lifecycle(&input, model.as_deref()));
        std::thread::spawn(move || {
            let output = match executor.child_request(&input, Some(cancel_token)) {
                Ok(child_request) => {
                    executor.execute_child_request(&child_request, &input, &publisher)
                }
                Err(error) => RunSubagentExecutionOutput {
                    status: RunSubagentExecutionStatus::Errored,
                    summary: error,
                    memory_write_count: 0,
                    token_usage: None,
                    model: None,
                    budget_exhausted: false,
                },
            };
            let output = executor.attach_result_artifact(&input, output);
            if let Err(error) = completion.complete(&input, &output) {
                log_completion_failure(&input.task_path, &error);
            }
        });
        true
    }

    fn notify_explicit_terminal(&self, task_paths: &[AgentPath], terminal: JobTerminalKind) {
        for task_path in task_paths {
            let Some(publisher) = self.lifecycle_registry.take_current(task_path) else {
                continue;
            };
            let _ = publisher
                .publish_terminal(subagent_explicit_terminal_lifecycle(task_path, terminal));
        }
    }
}

impl ChildExecutionRunner for ProviderRunSubagentExecutor {
    fn run(&self, input: ChildExecutionInput) -> Option<ChildExecutionOutput> {
        Some(self.execute_sync(&input))
    }
}

fn model_display_name(model: &ModelConfig) -> String {
    if model.name.trim().is_empty() {
        model.id.clone()
    } else {
        model.name.clone()
    }
}

/// 把一条协作消息渲染为低权威 runtime note 正文。
///
/// 渲染收敛在 `axon-agent-runtime` 的同一处：mailbox 的每次消费都用同一段措辞，
/// 避免同一批消息在不同消费点得到两种语义。
fn agent_mailbox_note(message: &InterAgentCommunication) -> String {
    axon_agent_runtime::agent_mailbox_note(message.author.as_str(), &message.content)
}

/// 拒绝执行器：用于不承载子代理执行的宿主，调用方会把 start=false 投影为 Blocked。
#[derive(Debug, Default)]
pub(crate) struct NoopRunSubagentExecutor;

impl RunSubagentExecutor for NoopRunSubagentExecutor {
    fn execute(&self, _input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        None
    }
}

impl ChildExecutionRunner for NoopRunSubagentExecutor {
    fn run(&self, _input: ChildExecutionInput) -> Option<ChildExecutionOutput> {
        None
    }
}

#[cfg(test)]
#[path = "run_subagent_executor_tests.rs"]
mod tests;
