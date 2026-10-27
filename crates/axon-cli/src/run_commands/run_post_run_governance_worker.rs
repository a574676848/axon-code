//! Post-run governance 持久化 worker。

use std::{path::Path, sync::Arc, thread, time::Duration};

use axon_agent_runtime::{AgentRunReasoningMessage, AgentRunResponse};
use axon_config::{ConfigLoader, provider_credential};
use axon_memory::{MemoryRuntimeQualityObservation, MemoryRuntimeQualityStore};
use axon_memory_read::{MemoryTurnHit, init_memory_orchestrator};
use axon_session_runtime::TurnPerformanceRecorder;
use serde::{Deserialize, Serialize};

use super::{
    RunAgentIdentity, RunCommand, RunRequest, RunRequestBuildOptions, RunRequestSession,
    RunStartupSnapshot,
    run_execution::RunResponse,
    run_memory_autonomy::apply_memory_autonomy_feedback,
    run_memory_citation::record_cited_memory_usage,
    run_memory_recall::{RunMemoryRecallEvent, RunMemoryRecallTraceEvent, RunMemoryScopeEvent},
    run_memory_usefulness::RunMemoryUsefulnessEvent,
    run_post_run_governance::PostRunGovernanceJob,
    run_post_run_governance_performance::{
        log_governance_performance, record_governance_config_ready,
        record_governance_startup_ready, start_governance_performance,
    },
    run_post_run_governance_store::{GOVERNANCE_MAX_ATTEMPTS, GovernanceQueue},
    run_request_subagents::{RunSubagentExecutionWiringInput, run_subagent_execution_wiring},
    run_self_iteration_subagent::run_post_run_self_iteration_subagents,
    run_session_store::{RunReasoningMessageRecord, RunSessionRecord},
};
use crate::setup_commands::env_path_for_config;

/// 治理载荷版本；v3 起携带执行身份与轮前场景帧，v2 及更早一律拒收。
const GOVERNANCE_PAYLOAD_VERSION: u32 = 3;
const GOVERNANCE_WORKER_BATCH_SIZE: usize = 8;
const GOVERNANCE_HTTP_STATUS_COMPLETED: u16 = 200;
const GOVERNANCE_RECALL_STATUS: &str = "durable";
const GOVERNANCE_RECALL_EMBEDDING_STATUS: &str = "persisted";
const GOVERNANCE_SCOPE_MATCH: &str = "scope_match";
const GOVERNANCE_SCOPE_MISMATCH: &str = "scope_mismatch";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GovernanceSessionReference {
    session_id: String,
    assistant_part_id: String,
    parent_session_id: Option<String>,
    cwd: Option<String>,
    provider: String,
    model: String,
    selected_skill_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DurableGovernancePayload {
    version: u32,
    config_path: std::path::PathBuf,
    session_record: GovernanceSessionReference,
    selected_skill_source_ref: Option<String>,
    memory_recall: Option<DurableMemoryRecall>,
    memory_usefulness: Option<RunMemoryUsefulnessEvent>,
    /// 本轮受众信任域；决定人格状态是否可写。
    #[serde(default)]
    persona_disclosure: Option<axon_domain::DisclosurePolicy>,
    /// 本轮执行身份记号，与受众互为印证。
    #[serde(default)]
    agent_identity: Option<String>,
    /// 轮前场景帧快照，保证结算与轮前判断使用同一份事实。
    #[serde(default)]
    persona_scene: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DurableMemoryRecall {
    hit_memory_ids: Vec<u64>,
    memory_turn_hits: Vec<DurableMemoryTurnHit>,
    hit_refs: Vec<String>,
    namespace: String,
    repo: Option<String>,
    branch: Option<String>,
    scope_mismatch_refs: Vec<String>,
    debugging_experience_hit_count: usize,
    skill_reflection_hit_count: usize,
    procedural_review_hit_count: usize,
    tool_guidance: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DurableMemoryTurnHit {
    memory_id: u64,
    content_ref: String,
}

impl DurableGovernancePayload {
    fn from_job(job: &PostRunGovernanceJob) -> Self {
        let record = &job.session_record;
        Self {
            version: GOVERNANCE_PAYLOAD_VERSION,
            config_path: job.config_path.clone(),
            session_record: GovernanceSessionReference {
                session_id: record.session_id.clone(),
                assistant_part_id: record.assistant_part_id.clone(),
                parent_session_id: record.parent_session_id.clone(),
                cwd: record.cwd.clone(),
                provider: record.provider.clone(),
                model: record.model.clone(),
                selected_skill_name: record.selected_skill_name.clone(),
            },
            selected_skill_source_ref: job.request.selected_skill_source_ref.clone(),
            memory_recall: job
                .request
                .memory_recall
                .as_ref()
                .map(DurableMemoryRecall::from_event),
            memory_usefulness: job.memory_usefulness.clone(),
            persona_disclosure: Some(job.request.goal_disclosure_policy),
            agent_identity: Some(agent_identity_token(job.request.goal_disclosure_policy)),
            persona_scene: Some(persona_scene_snapshot(&job.request)),
        }
    }
}

/// 由载荷里的身份记号还原执行身份；缺失时保守按主代理私有处理。
fn payload_agent_identity(payload: &DurableGovernancePayload) -> RunAgentIdentity {
    match payload.agent_identity.as_deref() {
        Some("primary_workspace") => RunAgentIdentity::PrimaryWorkspace,
        Some("goal_execution_subagent") => RunAgentIdentity::GoalExecutionSubagent,
        _ => RunAgentIdentity::PrimaryPrivate,
    }
}

/// 执行身份记号：私聊即主代理私有身份，其余按主代理受限身份处理。
fn agent_identity_token(disclosure: axon_domain::DisclosurePolicy) -> String {
    if disclosure == axon_domain::DisclosurePolicy::PrivateOnly {
        "primary_private".to_owned()
    } else {
        "primary_workspace".to_owned()
    }
}

/// 渠道记号，与人格场景帧的取值保持一致。
fn client_channel_token(channel: axon_client_identity::ClientChannel) -> &'static str {
    match channel {
        axon_client_identity::ClientChannel::Tui => "tui",
        axon_client_identity::ClientChannel::Cli => "cli",
        axon_client_identity::ClientChannel::Desktop => "desktop",
        axon_client_identity::ClientChannel::Feishu => "feishu",
        axon_client_identity::ClientChannel::Memory => "memory",
    }
}

/// 轮前场景帧快照：只记录轮前已经确定的事实，不引入模型推断。
fn persona_scene_snapshot(request: &super::RunRequest) -> serde_json::Value {
    use chrono::Timelike as _;
    let hour = u8::try_from(chrono::Local::now().hour()).unwrap_or(0);
    let daypart = match hour % 24 {
        0..=5 => "late_night",
        6..=11 => "morning",
        12..=17 => "afternoon",
        _ => "evening",
    };
    serde_json::json!({
        "channel": client_channel_token(request.client_channel),
        "audience": if request.goal_disclosure_policy == axon_domain::DisclosurePolicy::PrivateOnly {
            "private"
        } else {
            "group"
        },
        "daypart": daypart,
        "hasWorkspace": request.startup.workspace_root.is_some(),
    })
}

impl DurableMemoryRecall {
    fn from_event(event: &RunMemoryRecallEvent) -> Self {
        Self {
            hit_memory_ids: event.hit_memory_ids.clone(),
            memory_turn_hits: event
                .memory_turn_hits
                .iter()
                .map(|hit| DurableMemoryTurnHit {
                    memory_id: hit.memory_id,
                    content_ref: hit.content_ref.clone(),
                })
                .collect(),
            hit_refs: event.hit_refs.clone(),
            namespace: event.scope.namespace.clone(),
            repo: event.scope.repo.clone(),
            branch: event.scope.branch.clone(),
            scope_mismatch_refs: event
                .trace
                .iter()
                .filter(|trace| trace.scope_match == GOVERNANCE_SCOPE_MISMATCH)
                .map(|trace| trace.content_ref.clone())
                .collect(),
            debugging_experience_hit_count: event.debugging_experience_hit_count,
            skill_reflection_hit_count: event.skill_reflection_hit_count,
            procedural_review_hit_count: event.procedural_review_hit_count,
            tool_guidance: event.tool_guidance.clone(),
        }
    }

    fn into_event(self) -> RunMemoryRecallEvent {
        let mismatch = self.scope_mismatch_refs;
        let trace = self
            .hit_refs
            .iter()
            .map(|content_ref| RunMemoryRecallTraceEvent {
                content_ref: content_ref.clone(),
                score_basis_points: 0,
                scope_match: if mismatch.contains(content_ref) {
                    GOVERNANCE_SCOPE_MISMATCH
                } else {
                    GOVERNANCE_SCOPE_MATCH
                },
                prompt_injected: true,
                tool_guidance_applied: self.tool_guidance.contains(content_ref),
            })
            .collect();
        let memory_turn_hits = self
            .memory_turn_hits
            .iter()
            .map(|hit| MemoryTurnHit {
                memory_id: hit.memory_id,
                content_ref: hit.content_ref.clone(),
            })
            .collect::<Vec<_>>();
        RunMemoryRecallEvent {
            status: GOVERNANCE_RECALL_STATUS,
            hit_count: self.hit_refs.len(),
            hit_memory_ids: self.hit_memory_ids,
            hit_ref_pairs: memory_turn_hits
                .iter()
                .map(|hit| (hit.memory_id, hit.content_ref.clone()))
                .collect(),
            memory_turn_hits,
            hit_refs: self.hit_refs,
            scope: RunMemoryScopeEvent {
                namespace: self.namespace,
                repo: self.repo,
                branch: self.branch,
            },
            trace,
            debugging_experience_hit_count: self.debugging_experience_hit_count,
            skill_reflection_hit_count: self.skill_reflection_hit_count,
            procedural_review_hit_count: self.procedural_review_hit_count,
            embedding_status: GOVERNANCE_RECALL_EMBEDDING_STATUS,
            reason: None,
            tool_guidance: self.tool_guidance,
            remote_payload_allowed: false,
            prompt_text_persisted: false,
            raw_payload_persisted: false,
        }
    }
}

pub(super) fn serialize_governance_job(job: &PostRunGovernanceJob) -> Result<String, String> {
    serde_json::to_string(&DurableGovernancePayload::from_job(job))
        .map_err(|_| "governance payload 序列化失败".to_owned())
}

pub(crate) fn run_governance_worker(config_path: &Path) -> Result<usize, String> {
    run_governance_queue(config_path, execute_claimed_payload, true)
}

fn run_governance_queue(
    config_path: &Path,
    mut execute: impl FnMut(&str) -> Result<(), String>,
    wait_for_retry: bool,
) -> Result<usize, String> {
    let mut queue = GovernanceQueue::open(config_path)?;
    let mut completed = 0;
    let mut claims = 0;
    while claims < GOVERNANCE_WORKER_BATCH_SIZE {
        let now_ms = axon_time::unix_now_ms();
        queue.compact_expired_payloads(now_ms)?;
        let Some(job) = queue.claim_next(now_ms)? else {
            let Some(delay_ms) = queue.next_queued_delay_ms(now_ms)? else {
                break;
            };
            if !wait_for_retry {
                break;
            }
            thread::sleep(Duration::from_millis(delay_ms));
            continue;
        };
        claims += 1;
        match execute(&job.payload_json) {
            Ok(()) => {
                queue.complete(&job.job_id, axon_time::unix_now_ms())?;
                completed += 1;
            }
            Err(error) => {
                let attempts = if governance_error_is_retryable(&error) {
                    job.attempts.min(GOVERNANCE_MAX_ATTEMPTS)
                } else {
                    GOVERNANCE_MAX_ATTEMPTS
                };
                queue.fail(&job.job_id, attempts, &error, axon_time::unix_now_ms())?;
                axon_logger::error(&format!(
                    "post-run governance failed: job_id={}, reason={error}",
                    job.job_id
                ));
            }
        }
    }
    Ok(completed)
}

fn execute_claimed_payload(payload_json: &str) -> Result<(), String> {
    let performance = start_governance_performance();
    let payload = parse_governance_payload(payload_json)?;
    super::load_run_env_file(&env_path_for_config(&payload.config_path))?;
    let session_record = load_governance_session_record(&payload)?;
    let loaded = ConfigLoader::load_from_path(&payload.config_path)
        .map_err(|_| "governance 配置加载失败".to_owned())?;
    record_governance_config_ready(performance.as_deref());
    let request = rebuild_request(
        &payload,
        &session_record.prompt,
        &loaded.config,
        performance.clone(),
    )?;
    ensure_governance_provider_credential(&request)?;
    log_governance_performance(performance.as_deref());
    execute_governance_payload(&payload, &session_record, &loaded.config, &request)
}

fn load_governance_session_record(
    payload: &DurableGovernancePayload,
) -> Result<RunSessionRecord, String> {
    super::run_rollout_store::load_session_turn(
        &payload.config_path,
        &payload.session_record.session_id,
        &payload.session_record.assistant_part_id,
    )?
    .ok_or_else(|| {
        format!(
            "governance 会话记录不可用：session_id={}, assistant_part_id={}",
            payload.session_record.session_id, payload.session_record.assistant_part_id
        )
    })
}

fn ensure_governance_provider_credential(request: &RunRequest) -> Result<(), String> {
    if request
        .provider
        .api_key
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    let Some(env_name) = request.provider.api_key_env.as_deref() else {
        return Ok(());
    };
    if provider_credential(env_name).is_some_and(|value| !value.trim().is_empty()) {
        return Ok(());
    }
    Err(format!("API key 环境变量未设置：{env_name}"))
}

fn governance_error_is_retryable(error: &str) -> bool {
    ![
        "API key 环境变量未设置",
        "Axon .env",
        "governance payload",
        "governance 配置加载失败",
        "governance 会话缺少 cwd",
        "governance 会话记录不可用",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

fn parse_governance_payload(payload_json: &str) -> Result<DurableGovernancePayload, String> {
    let payload: DurableGovernancePayload = serde_json::from_str(payload_json)
        .map_err(|_| "governance payload JSON 无效".to_owned())?;
    if payload.version != GOVERNANCE_PAYLOAD_VERSION {
        return Err("governance payload 版本不匹配".to_owned());
    }
    Ok(payload)
}

fn execute_governance_payload(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
    config: &axon_config::AxonConfig,
    request: &RunRequest,
) -> Result<(), String> {
    let response = response_from_payload(&payload.session_record, session_record);
    record_cited_memory_usage(&payload.config_path, config, request, &response.content);
    // 先跑自我迭代拿到语义评估，再据其写人格状态：情绪与漂移是结算的输入。
    let self_iteration = run_post_run_self_iteration_subagents(request, &response);
    let assessment = self_iteration.semantic_assessment.as_ref();
    let disclosure = payload
        .persona_disclosure
        .unwrap_or(axon_domain::DisclosurePolicy::Workspace);
    super::run_persona_settlement::capture_style_candidate(
        &payload.config_path,
        disclosure,
        assessment,
        self_iteration.user_message_ref.as_deref(),
    );
    super::run_persona_settlement::settle_persona_turn(
        &super::run_persona_settlement::PersonaSettlementInput {
            config_path: &payload.config_path,
            session_id: &payload.session_record.session_id,
            assistant_part_id: &payload.session_record.assistant_part_id,
            disclosure,
            final_status: axon_persona::TurnFinalStatus::Completed,
            user_affect: assessment.and_then(|item| item.user_affect.as_deref()),
            user_fatigue: assessment.and_then(|item| item.user_fatigue.as_deref()),
            persona_drift_bp: assessment.and_then(|item| item.persona_drift_bp),
            drift_reason: assessment.and_then(|item| item.drift_reason.as_deref()),
            scene_audience: payload
                .persona_scene
                .as_ref()
                .and_then(|scene| scene.get("audience"))
                .and_then(serde_json::Value::as_str),
        },
    );
    if let Err(error) = record_runtime_quality(
        payload,
        session_record,
        config,
        &response,
        &self_iteration.quality,
    ) {
        axon_logger::error(&format!("post-run quality metrics failed: {error}"));
    }
    if self_iteration.failed_entry_count > 0 {
        return Err(format!(
            "governance 自我迭代有 {} 个子任务未完成",
            self_iteration.failed_entry_count
        ));
    }
    if self_iteration.skill_evolution.is_some() {
        axon_logger::debug("post-run governance applied skill evolution");
    }
    let _ = apply_memory_autonomy_feedback(
        &payload.config_path,
        config,
        request,
        payload.memory_usefulness.as_ref(),
        true,
    );
    Ok(())
}

fn record_runtime_quality(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
    config: &axon_config::AxonConfig,
    response: &RunResponse,
    quality: &super::run_self_iteration_subagent::PostRunSelfIterationQuality,
) -> Result<(), String> {
    let orchestrator = init_memory_orchestrator(&payload.config_path, config)?;
    let usage = response.token_usage.unwrap_or_default();
    MemoryRuntimeQualityStore::open(orchestrator.config())
        .map_err(|error| format!("{error:?}"))?
        .record_observation(&MemoryRuntimeQualityObservation {
            observation_id: format!(
                "post-run-governance:{}:{}",
                session_record.session_id, session_record.assistant_part_id
            ),
            cache_read_input_tokens: usage.cache_read_input_tokens,
            complete_input_tokens: usage.complete_input_tokens(),
            reflection_attempts: quality.reflection_attempts,
            reflection_yields: quality.reflection_yields,
            semantic_assessments: quality.semantic_assessments,
            false_memory_rejections: quality.false_memory_rejections,
            recorded_at_ms: axon_time::unix_now_ms(),
        })
        .map_err(|error| format!("{error:?}"))
}

#[cfg(test)]
pub(super) fn run_governance_worker_with_request(
    config_path: &Path,
    config: &axon_config::AxonConfig,
    request: &RunRequest,
) -> Result<usize, String> {
    run_governance_queue(
        config_path,
        |payload_json| {
            let payload = parse_governance_payload(payload_json)?;
            let session_record = load_governance_session_record(&payload)?;
            execute_governance_payload(&payload, &session_record, config, request)
        },
        false,
    )
}

fn rebuild_request(
    payload: &DurableGovernancePayload,
    prompt: &str,
    config: &axon_config::AxonConfig,
    performance: Option<Arc<dyn TurnPerformanceRecorder>>,
) -> Result<RunRequest, String> {
    let cwd = payload
        .session_record
        .cwd
        .as_deref()
        .map(Path::new)
        .ok_or_else(|| "governance 会话缺少 cwd".to_owned())?;
    let startup = Arc::new(RunStartupSnapshot::build(&payload.config_path, cwd)?);
    record_governance_startup_ready(performance.as_deref());
    let command = governance_run_command(payload, prompt);
    let mut request = RunRequest::from_command(
        &command,
        config,
        &payload.config_path,
        RunRequestSession::explicit(payload.session_record.session_id.clone()),
        Some(cwd),
        RunRequestBuildOptions {
            client_channel: axon_client_identity::ClientChannel::Cli,
            startup: Arc::clone(&startup),
            cancel_token: None,
            subagent_event_bus: None,
            subagent_session_sinks: None,
            agent_identity: payload_agent_identity(payload),
            performance,
            workspace_snapshot_prewarm: None,
        },
    )?;
    request
        .parent_session_id
        .clone_from(&payload.session_record.parent_session_id);
    request
        .selected_skill_name
        .clone_from(&payload.session_record.selected_skill_name);
    request
        .selected_skill_source_ref
        .clone_from(&payload.selected_skill_source_ref);
    request.memory_recall = payload
        .memory_recall
        .clone()
        .map(DurableMemoryRecall::into_event);
    rewire_subagents(&mut request, config, &command, &startup)?;
    Ok(request)
}

fn rewire_subagents(
    request: &mut RunRequest,
    config: &axon_config::AxonConfig,
    command: &RunCommand,
    startup: &Arc<RunStartupSnapshot>,
) -> Result<(), String> {
    let wiring = run_subagent_execution_wiring(RunSubagentExecutionWiringInput {
        config,
        config_path: &request.config_path,
        session_id: &request.session_id,
        cwd: &request.cwd,
        provider: &request.provider,
        model: &request.model,
        reasoning_effort: request.reasoning_effort.clone(),
        channel_chain: &request.channel_chain,
        routing_key: &request.routing_key,
        command,
        startup,
        cancel_token: None,
        subagent_event_bus: None,
        subagent_session_sinks: None,
        workspace_files: Arc::clone(&request.workspace_files),
    })?;
    request.agent_control = wiring.agent_control;
    request.agent_graph_store = wiring.agent_graph_store;
    request.subagent_sessions = wiring.subagent_sessions;
    request.subagent_executor = wiring.subagent_executor;
    request.subagent_cancel_tokens = wiring.subagent_cancel_tokens;
    Ok(())
}

fn governance_run_command(payload: &DurableGovernancePayload, prompt: &str) -> RunCommand {
    RunCommand {
        prompt: prompt.to_owned(),
        model: Some(payload.session_record.model.clone()),
        reasoning_effort: None,
        format: None,
        continue_last: false,
        session: None,
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    }
}

fn response_from_payload(
    session: &GovernanceSessionReference,
    record: &RunSessionRecord,
) -> RunResponse {
    AgentRunResponse {
        content: record.response.clone(),
        assistant_part_id: session.assistant_part_id.clone(),
        http_status: GOVERNANCE_HTTP_STATUS_COMPLETED,
        token_usage: record.usage,
        tool_receipts: record.tool_receipts.clone(),
        tool_calls: record.tool_calls.clone(),
        provider: session.provider.clone(),
        model: session.model.clone(),
        channel_attempts: Vec::new(),
        reasoning_messages: record
            .reasoning_messages
            .iter()
            .map(reasoning_message_from_record)
            .collect(),
        turn_events: record.turn_events.clone(),
        max_tokens_truncated: false,
    }
}

fn reasoning_message_from_record(record: &RunReasoningMessageRecord) -> AgentRunReasoningMessage {
    AgentRunReasoningMessage {
        part_id: record.part_id.clone(),
        reasoning_id: record.reasoning_id.clone(),
        content: record.content.clone(),
    }
}

#[cfg(test)]
mod failure_classification_tests {
    use super::governance_error_is_retryable;

    #[test]
    fn authentication_and_payload_errors_are_not_retried() {
        assert!(!governance_error_is_retryable(
            "API key 环境变量未设置：ANTHROPIC_AUTH_TOKEN"
        ));
        assert!(!governance_error_is_retryable(
            "governance payload JSON 无效"
        ));
        assert!(!governance_error_is_retryable(
            "governance 会话记录不可用：session_id=s1, assistant_part_id=a1"
        ));
    }

    #[test]
    fn transient_self_iteration_failure_remains_retryable() {
        assert!(governance_error_is_retryable(
            "governance 自我迭代有 1 个子任务未完成"
        ));
    }
}
