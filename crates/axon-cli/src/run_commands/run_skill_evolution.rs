use std::path::{Path, PathBuf};

use axon_agent_runtime::{
    SkillDraftProposalInput, SkillEvolutionDecision, SkillEvolutionRisk, SkillEvolutionRuntime,
    SkillImprovementProposalInput, agent_run_tool_receipt_completed_from_facts,
};
use serde::Serialize;

use super::{
    RunRequest,
    run_execution::RunResponse,
    run_skill_evolution_materialize::{
        draft_skill_name, update_existing_skill, write_validated_draft,
    },
    run_skill_evolution_store::{SkillEvolutionRecord, SkillEvolutionStage, SkillEvolutionStore},
    run_turn_outcome::build_turn_outcome,
};

const SKILL_EVOLUTION_LOW_MAX_FAILED_RECEIPTS: usize = 0;
const SKILL_EVOLUTION_SUMMARY_MAX_CHARS: usize = 1_000;
const SKILL_EVOLUTION_SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkillEvolutionEvent {
    pub(super) status: &'static str,
    pub(super) risk: &'static str,
    pub(super) skill_name: String,
    pub(super) material_ref: String,
    pub(super) source_ref: String,
    pub(super) summary: String,
    pub(super) auto_enabled: bool,
    pub(super) remote_publish_allowed: bool,
    pub(super) reasons: Vec<String>,
}

pub(super) fn skill_evolution_for_run(
    request: &RunRequest,
    response: &RunResponse,
    reflection_summary: &str,
) -> Option<SkillEvolutionEvent> {
    let skill_name = request.selected_skill_name.as_deref()?;
    let outcome = build_turn_outcome(request, response);
    let selected = outcome
        .selected_skill
        .as_ref()
        .filter(|selected| selected.execution_verified)?;
    improve_selected_skill_for_run(
        request,
        response,
        skill_name,
        reflection_summary,
        &selected.evidence_refs,
    )
}

pub(super) fn draft_skill_for_run(
    request: &RunRequest,
    response: &RunResponse,
    reflection_summary: &str,
    evidence_refs: &[String],
) -> Option<SkillEvolutionEvent> {
    let summary = bounded_evolution_summary(reflection_summary)?;
    let outcome = build_turn_outcome(request, response);
    if !outcome
        .execution_evidence
        .has_verified_evidence_ref(evidence_refs)
    {
        return None;
    }
    let skill_name = draft_skill_name(&summary);
    let risk = draft_skill_evolution_risk(response);
    let store = SkillEvolutionStore::open(&request.config_path).ok()?;
    update_current_state(
        &store,
        request,
        "draft",
        &skill_name,
        risk,
        evidence_refs,
        SkillEvolutionStage::Candidate,
        None,
        None,
        "已验证 Procedure 反思进入 Skill candidate",
    )
    .ok()?;
    let evidence_ref = evidence_refs.join(",");
    match SkillEvolutionRuntime.draft_proposal(SkillDraftProposalInput {
        skill_name: skill_name.clone(),
        evidence_ref: evidence_ref.clone(),
        problem_summary: summary.clone(),
        suggested_action: "生成可验证的新 Skill 草案".to_owned(),
        risk,
    }) {
        SkillEvolutionDecision::DraftProposal(_) => {
            let material = write_validated_draft(&store, &skill_name, &summary).ok()?;
            update_current_state(
                &store,
                request,
                "draft",
                &skill_name,
                risk,
                evidence_refs,
                SkillEvolutionStage::Draft,
                Some(&material.material_ref),
                Some(&material.revision),
                "Skill draft 已生成",
            )
            .ok()?;
            update_current_state(
                &store,
                request,
                "draft",
                &skill_name,
                risk,
                evidence_refs,
                SkillEvolutionStage::Validated,
                Some(&material.material_ref),
                Some(&material.revision),
                "schema、正向触发与负向不触发 eval 已通过",
            )
            .ok()?;
            Some(SkillEvolutionEvent {
                status: "validatedDraft",
                risk: risk_label(risk),
                skill_name,
                material_ref: material.material_ref,
                source_ref: evidence_ref,
                summary,
                auto_enabled: false,
                remote_publish_allowed: false,
                reasons: vec![
                    "skill_evolution:verified_procedure".to_owned(),
                    "skill_evolution:draft_validated".to_owned(),
                    "skill_evolution:promotion_requires_gate".to_owned(),
                ],
            })
        }
        SkillEvolutionDecision::SilentExperience(_) | SkillEvolutionDecision::Rejected(_) => {
            let _ = update_current_state(
                &store,
                request,
                "draft",
                &skill_name,
                risk,
                evidence_refs,
                SkillEvolutionStage::Rejected,
                None,
                None,
                "风险门禁拒绝生成 Skill draft",
            );
            None
        }
        SkillEvolutionDecision::ImprovementProposal(_) => None,
    }
}

fn improve_selected_skill_for_run(
    request: &RunRequest,
    response: &RunResponse,
    skill_name: &str,
    reflection_summary: &str,
    evidence_refs: &[String],
) -> Option<SkillEvolutionEvent> {
    let summary = bounded_evolution_summary(reflection_summary)?;
    let risk = selected_skill_evolution_risk(request, response);
    let evidence_ref = evidence_refs.join(",");
    let store = SkillEvolutionStore::open(&request.config_path).ok()?;
    update_current_state(
        &store,
        request,
        "improvement",
        skill_name,
        risk,
        evidence_refs,
        SkillEvolutionStage::Candidate,
        None,
        None,
        "已实际执行的 Skill 进入改进 candidate",
    )
    .ok()?;
    match SkillEvolutionRuntime.improvement_proposal(SkillImprovementProposalInput {
        skill_name: skill_name.to_owned(),
        evidence_ref: evidence_ref.clone(),
        problem_summary: summary.clone(),
        suggested_action: "追加本次已验证的 project/user skill 说明增强".to_owned(),
        risk,
    }) {
        SkillEvolutionDecision::ImprovementProposal(_) => {
            let skill_file = selected_skill_file(request, skill_name)?;
            let material = update_existing_skill(&skill_file, skill_name, &summary)
                .ok()
                .flatten()?;
            update_current_state(
                &store,
                request,
                "improvement",
                skill_name,
                risk,
                evidence_refs,
                SkillEvolutionStage::Validated,
                Some(&material.material_ref),
                Some(&material.revision),
                "frontmatter、大小、正向触发与负向不触发 eval 已通过",
            )
            .ok()?;
            update_current_state(
                &store,
                request,
                "improvement",
                skill_name,
                risk,
                evidence_refs,
                SkillEvolutionStage::Promoted,
                Some(&material.material_ref),
                Some(&material.revision),
                "低风险既有 Skill 已原子更新",
            )
            .ok()?;
            Some(SkillEvolutionEvent {
                status: "autoApplied",
                risk: "low",
                material_ref: material.material_ref,
                source_ref: evidence_ref,
                summary,
                skill_name: skill_name.to_owned(),
                auto_enabled: true,
                remote_publish_allowed: false,
                reasons: vec![
                    "skill_evolution:self_iteration_subagent".to_owned(),
                    "skill_evolution:verified_evidence".to_owned(),
                    "skill_evolution:validated_atomic_update".to_owned(),
                ],
            })
        }
        SkillEvolutionDecision::SilentExperience(_) | SkillEvolutionDecision::Rejected(_) => {
            let _ = update_current_state(
                &store,
                request,
                "improvement",
                skill_name,
                risk,
                evidence_refs,
                SkillEvolutionStage::Rejected,
                None,
                None,
                "风险或内容门禁拒绝 Skill 更新",
            );
            None
        }
        SkillEvolutionDecision::DraftProposal(_) => None,
    }
}

fn selected_skill_file(request: &RunRequest, skill_name: &str) -> Option<PathBuf> {
    let source_ref = request.selected_skill_source_ref.as_deref().unwrap_or("");
    let candidates = [
        request
            .cwd
            .join(".axon")
            .join("skills")
            .join(skill_name)
            .join(SKILL_EVOLUTION_SKILL_FILE),
        home_dir()?
            .join(".axon")
            .join("skills")
            .join(skill_name)
            .join(SKILL_EVOLUTION_SKILL_FILE),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file() && source_allows_skill_write(source_ref, path, &request.cwd))
}

fn source_allows_skill_write(source_ref: &str, path: &Path, cwd: &Path) -> bool {
    let normalized_source = source_ref.to_ascii_lowercase();
    let normalized_path = path_ref(path).to_ascii_lowercase();
    if normalized_source.contains("claude") || normalized_source.contains("codex") {
        return false;
    }
    normalized_source.contains("axon")
        || normalized_path.contains("/.axon/skills/")
        || path.starts_with(cwd.join(".axon").join("skills"))
}

fn bounded_evolution_summary(value: &str) -> Option<String> {
    let summary = value.trim();
    if summary.is_empty() || text_has_sensitive_content(summary) {
        return None;
    }
    Some(bounded_text(summary, SKILL_EVOLUTION_SUMMARY_MAX_CHARS))
}

fn update_current_state(
    store: &SkillEvolutionStore,
    request: &RunRequest,
    kind: &str,
    skill_name: &str,
    risk: SkillEvolutionRisk,
    evidence_refs: &[String],
    stage: SkillEvolutionStage,
    material_ref: Option<&str>,
    revision: Option<&str>,
    reason: &str,
) -> Result<(), String> {
    store.record_current(&SkillEvolutionRecord {
        session_id: request.session_id.clone(),
        kind: kind.to_owned(),
        skill_name: skill_name.to_owned(),
        risk: risk_label(risk).to_owned(),
        evidence_refs: evidence_refs.to_vec(),
        stage,
        material_ref: material_ref.map(str::to_owned),
        revision: revision.map(str::to_owned),
        reason: reason.to_owned(),
        recorded_at_ms: axon_time::unix_now_ms(),
    })
}

fn draft_skill_evolution_risk(response: &RunResponse) -> SkillEvolutionRisk {
    let failed = response
        .tool_receipts
        .iter()
        .filter(|receipt| !agent_run_tool_receipt_completed_from_facts(*receipt))
        .count();
    if response_summary_has_sensitive_content(response) {
        return SkillEvolutionRisk::High;
    }
    if failed > SKILL_EVOLUTION_LOW_MAX_FAILED_RECEIPTS {
        return SkillEvolutionRisk::Medium;
    }
    SkillEvolutionRisk::Low
}

fn selected_skill_evolution_risk(
    request: &RunRequest,
    response: &RunResponse,
) -> SkillEvolutionRisk {
    let source_ref = request.selected_skill_source_ref.as_deref().unwrap_or("");
    if response_summary_has_sensitive_content(response) {
        return SkillEvolutionRisk::High;
    }
    if !source_ref.to_ascii_lowercase().contains("axon")
        && !source_ref.to_ascii_lowercase().contains(".axon")
    {
        return SkillEvolutionRisk::High;
    }
    draft_skill_evolution_risk(response)
}

fn response_summary_has_sensitive_content(response: &RunResponse) -> bool {
    response
        .tool_receipts
        .iter()
        .any(|receipt| text_has_sensitive_content(&receipt.summary))
}

fn text_has_sensitive_content(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("token=")
        || normalized.contains("password=")
        || normalized.contains("authorization:")
        || normalized.contains("bearer ")
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn path_ref(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn risk_label(risk: SkillEvolutionRisk) -> &'static str {
    match risk {
        SkillEvolutionRisk::Low => "low",
        SkillEvolutionRisk::Medium => "medium",
        SkillEvolutionRisk::High => "high",
    }
}

fn home_dir() -> Option<PathBuf> {
    axon_config::user_home_dir().ok()
}

#[cfg(test)]
mod tests {
    use axon_agent_runtime::{ProviderToolCall, SkillEvolutionRisk};

    use super::{
        RunResponse, draft_skill_evolution_risk, draft_skill_for_run, skill_evolution_for_run,
    };
    use crate::run_commands::{RunRequest, run_tool_loop::RunToolReceipt};

    const VALID_RELEASE_SKILL: &str =
        "---\nname: release\ndescription: 验证发布流程\n---\n\n# Release\n";

    #[test]
    fn successful_run_is_low_risk_skill_evolution() {
        let response = response_with_receipt("completed", "patched files", true);

        assert_eq!(
            draft_skill_evolution_risk(&response),
            SkillEvolutionRisk::Low
        );
    }

    #[test]
    fn failed_or_sensitive_run_is_not_low_risk_skill_evolution() {
        let mut response = response_with_receipt("failed", "command failed", false);

        assert_eq!(
            draft_skill_evolution_risk(&response),
            SkillEvolutionRisk::Medium
        );

        response.tool_receipts[0].summary = "authorization: bearer secret".to_owned();
        assert_eq!(
            draft_skill_evolution_risk(&response),
            SkillEvolutionRisk::High
        );
    }

    #[test]
    fn selected_project_skill_is_enhanced_without_manual_review() {
        let root = test_root("selected-project");
        let skill_file = root
            .join(".axon")
            .join("skills")
            .join("release")
            .join("SKILL.md");
        std::fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        std::fs::write(&skill_file, VALID_RELEASE_SKILL).unwrap();
        let mut request = request_fixture(root.clone());
        request.selected_skill_name = Some("release".to_owned());
        request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
        let response = verified_skill_response("cargo check passed");

        let reflection = "该 Skill 应先确认发布目标，再执行并验证产物。";
        let event = skill_evolution_for_run(&request, &response, reflection)
            .expect("Project skill 低风险反思无需人工开关也应自动增强");

        assert_eq!(event.status, "autoApplied");
        assert_eq!(event.skill_name, "release");
        let content = std::fs::read_to_string(skill_file).unwrap();
        assert!(content.starts_with(VALID_RELEASE_SKILL));
        assert!(content.contains("## 已验证经验"));
        assert!(content.contains(reflection));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn high_risk_selected_skill_is_rejected_without_write() {
        let root = test_root("selected-high-risk");
        let skill_file = root
            .join(".axon")
            .join("skills")
            .join("release")
            .join("SKILL.md");
        std::fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        std::fs::write(&skill_file, VALID_RELEASE_SKILL).unwrap();
        let mut request = request_fixture(root.clone());
        request.selected_skill_name = Some("release".to_owned());
        request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
        let response = verified_skill_response("authorization: bearer secret");

        assert!(skill_evolution_for_run(&request, &response, "不得把敏感结果写入 Skill").is_none());
        let content = std::fs::read_to_string(&skill_file).unwrap();
        assert_eq!(content, VALID_RELEASE_SKILL);
        let state =
            std::fs::read_to_string(root.join("skill-evolution").join("state.json")).unwrap();
        assert!(state.contains("\"risk\": \"high\""));
        assert!(state.contains("\"kind\": \"improvement\""));
        assert!(state.contains("\"stage\": \"rejected\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn verified_procedure_generates_validated_draft_without_promotion() {
        let root = test_root("validated-draft");
        let request = request_fixture(root.clone());
        let response = procedure_response();

        let event = draft_skill_for_run(
            &request,
            &response,
            "每次发布前依次执行格式、编译和测试验证。",
            &["receipt:2".to_owned()],
        )
        .expect("已验证 Procedure 应生成 draft");

        assert_eq!(event.status, "validatedDraft");
        assert!(!event.auto_enabled);
        assert!(std::path::Path::new(&event.material_ref).is_file());
        assert!(!root.join("skills").join(&event.skill_name).exists());
        let state = std::fs::read_to_string(root.join("skill-evolution/state.json")).unwrap();
        assert!(state.contains("\"stage\": \"validated\""));
        let _ = std::fs::remove_dir_all(root);
    }

    fn response_with_receipt(status: &str, summary: &str, wrote: bool) -> RunResponse {
        RunResponse {
            content: "done".to_owned(),
            assistant_part_id: "assistant-part:skill-evolution".to_owned(),
            provider: "fixture".to_owned(),
            model: "fixture-chat".to_owned(),
            token_usage: None,
            http_status: 200,
            channel_attempts: Vec::new(),
            reasoning_messages: Vec::new(),
            tool_receipts: vec![RunToolReceipt {
                tool_call_id: 0,
                part_id: "tool-part:0".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                status: status.to_owned(),
                summary: summary.to_owned(),
                metadata: None,
                error_type: None,
                remote_trigger_allowed: false,
                filesystem_write_performed: wrote,
                output_ref: None,
                duration_ms: None,
            }],
            tool_calls: Vec::new(),
            turn_events: Vec::new(),
            max_tokens_truncated: false,
        }
    }

    fn verified_skill_response(summary: &str) -> RunResponse {
        let mut response = response_with_receipt("completed", summary, false);
        response.tool_receipts = vec![
            tool_receipt("call-skill", "builtin.invoke_skill", "loaded", false),
            tool_receipt("call-edit", "builtin.edit", summary, true),
            tool_receipt(
                "call-test",
                "builtin.shell_command",
                "validation completed",
                false,
            ),
        ];
        response.tool_calls = vec![ProviderToolCall::new(
            "call-skill",
            "builtin.invoke_skill",
            r#"{"skill_name":"release"}"#,
        )];
        response
    }

    fn procedure_response() -> RunResponse {
        let mut response = response_with_receipt("completed", "done", false);
        response.tool_receipts = vec![
            tool_receipt("call-edit", "builtin.edit", "patched files", true),
            tool_receipt(
                "call-test",
                "builtin.shell_command",
                "validation completed",
                false,
            ),
        ];
        response
    }

    fn tool_receipt(id: &str, tool_name: &str, summary: &str, wrote: bool) -> RunToolReceipt {
        RunToolReceipt {
            tool_call_id: 0,
            part_id: format!("tool-part:{id}"),
            provider_call_id: id.to_owned(),
            tool_name: tool_name.to_owned(),
            status: "completed".to_owned(),
            summary: summary.to_owned(),
            metadata: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: wrote,
            output_ref: None,
            duration_ms: None,
        }
    }

    fn request_fixture(root: std::path::PathBuf) -> RunRequest {
        let config_path = root.join("config.toml");
        let session_id = "session-1".to_owned();
        let (startup, execution_tools) =
            crate::run_commands::run_startup_snapshot::test_startup_runtime(
                &config_path,
                &root,
                &session_id,
            );
        RunRequest {
            user_question_port: None,
            client_channel: axon_client_identity::ClientChannel::Cli,
            config_path,
            session_id: session_id.clone(),
            invocation_id: format!("test-invocation:{session_id}"),
            startup,
            execution_tools,
            resumed_from: None,
            parent_session_id: None,
            cwd: root,
            prompt: "生成可复用 skill".to_owned(),
            quoted_message: None,
            structured_output: None,
            selected_skill_name: None,
            selected_skill_source_ref: None,
            model_payload: axon_agent_runtime::AgentModelPayload {
                messages: vec![axon_agent_runtime::AgentModelMessage::text(
                    "user",
                    "生成可复用 skill",
                )],
                model_tools: Vec::new(),
                context_section_count: 0,
                agent_pane_included: false,
                injected_context_messages: Vec::new(),
            },
            provider: axon_config::ProviderConfig::default(),
            model: axon_config::ModelConfig::default(),
            reasoning_effort: None,
            channel_chain: Vec::new(),
            routing_key: "fixture".to_owned(),
            balancer: axon_config::ChannelBalancerConfig::default(),
            timeout: std::time::Duration::from_secs(30),
            memory_recall: None,
            goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
            goal_transport_context: None,
            cancel_token: None,
            model_profile_id: axon_domain::ModelProfileId(0),
            permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
            current_agent_path: axon_agent_runtime::AgentPath::root(),
            agent_control: std::sync::Arc::new(std::sync::Mutex::new(
                axon_agent_runtime::AgentControlRuntime::default(),
            )),
            agent_graph_store: std::sync::Arc::new(
                axon_agent_runtime::LocalAgentGraphStore::default(),
            ),
            subagent_sessions: std::sync::Arc::new(std::sync::Mutex::new(
                axon_agent_runtime::SubagentSessionRuntime::default(),
            )),
            subagent_executor: std::sync::Arc::new(
                crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
            ),
            subagent_cancel_tokens: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            steer_inbox: None,
            workspace_files: std::sync::Arc::new(
                axon_agent_runtime::WorkspaceFileSession::default(),
            ),
            auto_compaction_threshold: 0.8,
            performance: None,
        }
    }

    fn test_root(label: &str) -> std::path::PathBuf {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-skill-evolution-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}
