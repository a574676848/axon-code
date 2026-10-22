use std::{collections::BTreeMap, path::Path};

use axon_agent_runtime::{
    MemoryHybridHit, MemoryRetrievalMode, MemorySqliteRecord,
    agent_model_memory_guidance_for_origin, agent_model_memory_guidance_projection,
};
use axon_config::AxonConfig;
use axon_crypto::sha256_tagged_ref;
use axon_memory_read::{
    MEMORY_GLOBAL_NAMESPACE_REF, MEMORY_QUERY_EMBEDDING_DISABLED, MEMORY_QUERY_EMBEDDING_GENERATED,
    MemoryHybridScope, MemoryTurnContext, MemoryTurnHit, PreparedMemoryQuery,
    hybrid_query_memory_with_embedding, init_memory_orchestrator, injected_usage_events,
    memory_query_embedding, recalled_usage_events, record_read_usage_async,
};
use axon_session::{WorkspaceSnapshot, WorkspaceSnapshotFieldState};
use serde::Serialize;

use axon_session_runtime::{TurnPerformanceRecorder, TurnPerformanceStage};

use super::{RunCommand, provider_stream_idle_timeout, run_execution::now_ms};

const MEMORY_RECALL_STATUS_HYBRID: &str = "hybrid";
const MEMORY_RECALL_STATUS_TEXT_FALLBACK: &str = "text_fallback";
const MEMORY_RECALL_STATUS_CORE_FALLBACK: &str = "core_fallback";
const MEMORY_RECALL_STATUS_SKIPPED: &str = "skipped";
/// 单次自动召回最多注入的 Memory 条目数；后续仍受 Context Token Budget 约束。
const MEMORY_HIT_LIMIT: usize = 32;
/// `axon run` 的 Memory recall 准备结果。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PreparedMemoryRecall {
    /// 可注入模型上下文的 ref-only Memory 命中。
    pub(super) hits: Vec<MemoryHybridHit>,
    /// JSON 输出中的可观测召回摘要；不包含 prompt 原文或 provider raw payload。
    pub(super) event: Option<RunMemoryRecallEvent>,
}

/// `axon run` 的 Memory recall 摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunMemoryRecallEvent {
    pub(super) status: &'static str,
    pub(super) hit_count: usize,
    #[serde(skip)]
    pub(super) hit_memory_ids: Vec<u64>,
    #[serde(skip)]
    pub(super) hit_ref_pairs: Vec<(u64, String)>,
    #[serde(skip)]
    pub(super) memory_turn_hits: Vec<MemoryTurnHit>,
    pub(super) hit_refs: Vec<String>,
    pub(super) scope: RunMemoryScopeEvent,
    pub(super) trace: Vec<RunMemoryRecallTraceEvent>,
    pub(super) debugging_experience_hit_count: usize,
    pub(super) skill_reflection_hit_count: usize,
    pub(super) procedural_review_hit_count: usize,
    pub(super) embedding_status: &'static str,
    pub(super) reason: Option<String>,
    pub(super) tool_guidance: Vec<String>,
    pub(super) remote_payload_allowed: bool,
    pub(super) prompt_text_persisted: bool,
    pub(super) raw_payload_persisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunMemoryScopeEvent {
    #[serde(rename = "namespaceRef")]
    pub(super) namespace: String,
    #[serde(rename = "repoRef")]
    pub(super) repo: Option<String>,
    #[serde(rename = "branchRef")]
    pub(super) branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunMemoryRecallTraceEvent {
    pub(super) content_ref: String,
    pub(super) score_basis_points: u16,
    pub(super) scope_match: &'static str,
    pub(super) prompt_injected: bool,
    pub(super) tool_guidance_applied: bool,
}

pub(super) fn prepare_memory_recall_for_run(
    config_path: &Path,
    config: &AxonConfig,
    prompt: &str,
    command: &RunCommand,
    scope: RunMemoryScopeEvent,
    performance: Option<&dyn TurnPerformanceRecorder>,
) -> PreparedMemoryRecall {
    let Ok(orchestrator) = init_memory_orchestrator(config_path, config) else {
        return skipped("memory orchestrator unavailable");
    };

    let recall_now_ms = now_ms();
    let query_embedding = memory_query_embedding(
        config,
        prompt,
        provider_stream_idle_timeout(command.timeout),
    );
    if let Some(performance) = performance {
        performance.record(TurnPerformanceStage::MemoryEmbeddingReady);
    }
    // Global 与 Workspace 分路查询，复用单次 query_embedding 消除重复远端 HTTP 请求，底层 scope 过滤保持错配 fail-closed。
    let global_query = hybrid_query_memory_with_embedding(
        &orchestrator,
        prompt,
        &query_embedding,
        MEMORY_HIT_LIMIT,
        recall_now_ms,
        Some(MemoryHybridScope {
            user_ref: None,
            namespace_ref: MEMORY_GLOBAL_NAMESPACE_REF.to_owned(),
        }),
    );
    let workspace_query = if scope.namespace == MEMORY_GLOBAL_NAMESPACE_REF {
        PreparedMemoryQuery {
            hits: Vec::new(),
            embedding_status: global_query.embedding_status,
            reason: None,
        }
    } else {
        hybrid_query_memory_with_embedding(
            &orchestrator,
            prompt,
            &query_embedding,
            MEMORY_HIT_LIMIT,
            recall_now_ms,
            Some(MemoryHybridScope {
                user_ref: None,
                namespace_ref: scope.namespace.clone(),
            }),
        )
    };
    let embedding_status = global_query.embedding_status;
    let reason = global_query.reason.or(workspace_query.reason);
    let hits = merge_scoped_hits(global_query.hits, workspace_query.hits);
    let scoped_records = hits
        .iter()
        .map(|hit| hit.record.clone())
        .collect::<Vec<_>>();
    let mut usage_events = recalled_usage_events(&hits, recall_now_ms);
    usage_events.extend(injected_usage_events(&hits, recall_now_ms));
    let _ = record_read_usage_async(orchestrator.clone(), usage_events);
    let status = memory_recall_status(orchestrator.retrieval_mode(), embedding_status);
    let event = recall_event(
        status,
        &hits,
        &scope,
        &scoped_records,
        embedding_status,
        reason,
    );

    PreparedMemoryRecall {
        hits,
        event: Some(event),
    }
}

fn memory_recall_status(mode: MemoryRetrievalMode, embedding_status: &'static str) -> &'static str {
    match mode {
        MemoryRetrievalMode::Hybrid if embedding_status == MEMORY_QUERY_EMBEDDING_GENERATED => {
            MEMORY_RECALL_STATUS_HYBRID
        }
        MemoryRetrievalMode::Hybrid if cfg!(feature = "memory-text-index") => {
            MEMORY_RECALL_STATUS_TEXT_FALLBACK
        }
        MemoryRetrievalMode::Hybrid | MemoryRetrievalMode::CoreFallback => {
            MEMORY_RECALL_STATUS_CORE_FALLBACK
        }
        MemoryRetrievalMode::TextFallback => MEMORY_RECALL_STATUS_TEXT_FALLBACK,
    }
}

fn skipped(reason: impl Into<String>) -> PreparedMemoryRecall {
    PreparedMemoryRecall {
        hits: Vec::new(),
        event: Some(recall_event(
            MEMORY_RECALL_STATUS_SKIPPED,
            &[],
            &RunMemoryScopeEvent {
                namespace: "memory-scope:unknown".to_owned(),
                repo: None,
                branch: None,
            },
            &[],
            MEMORY_QUERY_EMBEDDING_DISABLED,
            Some(reason.into()),
        )),
    }
}

fn recall_event(
    status: &'static str,
    hits: &[MemoryHybridHit],
    scope: &RunMemoryScopeEvent,
    persisted: &[MemorySqliteRecord],
    embedding_status: &'static str,
    reason: Option<String>,
) -> RunMemoryRecallEvent {
    let hit_memory_ids = hits
        .iter()
        .map(|hit| hit.record.record.id.0)
        .collect::<Vec<_>>();
    let hit_ref_pairs = hits
        .iter()
        .filter_map(|hit| hit_ref(hit).map(|value| (hit.record.record.id.0, value)))
        .collect::<Vec<_>>();
    let hit_refs = hit_ref_pairs
        .iter()
        .map(|(_, hit_ref)| hit_ref.clone())
        .collect::<Vec<_>>();
    let memory_turn_hits = MemoryTurnContext::from_injected_hits("cli_run", hits).injected_hits;
    let debugging_experience_hit_count =
        origin_hit_count(hits, axon_domain::MemorySourceOrigin::DebuggingExperience);
    let skill_reflection_hit_count =
        origin_hit_count(hits, axon_domain::MemorySourceOrigin::SkillReflection);
    let trace = recall_trace(hits, scope, persisted);
    RunMemoryRecallEvent {
        status,
        hit_count: hits.len(),
        hit_memory_ids,
        hit_ref_pairs,
        memory_turn_hits,
        hit_refs,
        scope: scope.clone(),
        trace,
        debugging_experience_hit_count,
        skill_reflection_hit_count,
        procedural_review_hit_count: procedural_review_hit_count(hits),
        embedding_status,
        reason,
        tool_guidance: tool_guidance(hits),
        remote_payload_allowed: false,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
    }
}

fn merge_scoped_hits(
    global_hits: Vec<MemoryHybridHit>,
    workspace_hits: Vec<MemoryHybridHit>,
) -> Vec<MemoryHybridHit> {
    let mut by_id = BTreeMap::new();
    for hit in global_hits.into_iter().chain(workspace_hits) {
        by_id.entry(hit.record.record.id).or_insert(hit);
    }
    let mut hits = by_id.into_values().collect::<Vec<_>>();
    hits.sort_by(|left, right| {
        right
            .score_basis_points
            .cmp(&left.score_basis_points)
            .then_with(|| left.record.record.id.0.cmp(&right.record.record.id.0))
    });
    hits.truncate(MEMORY_HIT_LIMIT);
    hits
}

fn recall_trace(
    hits: &[MemoryHybridHit],
    scope: &RunMemoryScopeEvent,
    persisted: &[MemorySqliteRecord],
) -> Vec<RunMemoryRecallTraceEvent> {
    let records = persisted_by_id(persisted);
    hits.iter()
        .filter_map(|hit| {
            let content_ref = hit_ref(hit)?;
            let scope_match = records
                .get(&hit.record.record.id.0)
                .map_or("unscoped", |record| scope_match(scope, record));
            let tool_guidance_applied = hit
                .record
                .record
                .source_trace
                .as_ref()
                .and_then(|trace| agent_model_memory_guidance_for_origin(trace.origin))
                .is_some();
            Some(RunMemoryRecallTraceEvent {
                content_ref,
                score_basis_points: hit.score_basis_points,
                scope_match,
                prompt_injected: true,
                tool_guidance_applied,
            })
        })
        .collect()
}

fn persisted_by_id(records: &[MemorySqliteRecord]) -> BTreeMap<u64, &MemorySqliteRecord> {
    records
        .iter()
        .map(|record| (record.record.id.0, record))
        .collect()
}

fn scope_match(scope: &RunMemoryScopeEvent, record: &MemorySqliteRecord) -> &'static str {
    if record.user_ref.is_none()
        && record.namespace_ref.as_deref() == Some(MEMORY_GLOBAL_NAMESPACE_REF)
    {
        "global"
    } else if record.repo_ref.as_ref() == scope.repo.as_ref() && scope.repo.is_some() {
        "same_repo"
    } else if record.namespace_ref.as_ref() == Some(&scope.namespace) {
        "same_workspace"
    } else if record.repo_ref.is_some() || record.namespace_ref.is_some() {
        "scope_mismatch"
    } else {
        "unscoped"
    }
}

fn hit_ref(hit: &MemoryHybridHit) -> Option<String> {
    hit.record
        .record
        .content_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn origin_hit_count(hits: &[MemoryHybridHit], origin: axon_domain::MemorySourceOrigin) -> usize {
    hits.iter()
        .filter(|hit| {
            hit.record
                .record
                .source_trace
                .as_ref()
                .is_some_and(|trace| trace.origin == origin)
        })
        .count()
}

fn procedural_review_hit_count(hits: &[MemoryHybridHit]) -> usize {
    hits.iter()
        .filter(|hit| {
            hit.record
                .record
                .source_trace
                .as_ref()
                .is_some_and(|trace| {
                    trace.origin == axon_domain::MemorySourceOrigin::SkillReflection
                })
                && hit.record.importance_basis_points < 6_000
        })
        .count()
}

fn tool_guidance(hits: &[MemoryHybridHit]) -> Vec<String> {
    agent_model_memory_guidance_projection(hits).tool_guidance
}

pub(super) fn memory_scope(snapshot: Option<&WorkspaceSnapshot>) -> RunMemoryScopeEvent {
    let Some(snapshot) = snapshot else {
        return RunMemoryScopeEvent {
            namespace: MEMORY_GLOBAL_NAMESPACE_REF.to_owned(),
            repo: None,
            branch: None,
        };
    };
    let workspace = sha256_tagged_ref(
        "workspace",
        snapshot.requested_cwd.to_string_lossy().as_bytes(),
    );
    let branch_ref = snapshot
        .git
        .branch
        .as_ref()
        .map(|branch| sha256_tagged_ref("branch", branch.as_bytes()));
    let repo = snapshot.git.remote.clone().or_else(|| {
        (snapshot.git.remote_state == WorkspaceSnapshotFieldState::Absent
            && snapshot.git.dirty_state == WorkspaceSnapshotFieldState::Collected)
            .then(|| snapshot.workspace_root.to_string_lossy().replace('\\', "/"))
    });
    let repo_ref = repo.map(|repo| sha256_tagged_ref("repo", repo.as_bytes()));
    RunMemoryScopeEvent {
        namespace: branch_ref.as_ref().map_or_else(
            || workspace.clone(),
            |branch| format!("{workspace}:{branch}"),
        ),
        repo: repo_ref,
        branch: branch_ref,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{MemoryCandidateId, MemoryKind, MemoryRecord, MemoryState};

    #[test]
    fn scoped_hit_merge_is_bounded_independently_from_context_window() {
        let hits = (0..MEMORY_HIT_LIMIT + 1)
            .map(|index| test_hit(u64::try_from(index + 1).expect("测试 ID 不溢出")))
            .collect::<Vec<_>>();

        let merged = merge_scoped_hits(hits, Vec::new());

        assert_eq!(merged.len(), MEMORY_HIT_LIMIT);
    }

    #[test]
    fn home_scope_is_global_and_does_not_create_workspace_namespace() {
        let scope = memory_scope(None);

        assert_eq!(scope.namespace, MEMORY_GLOBAL_NAMESPACE_REF);
        assert_eq!(scope.repo, None);
        assert_eq!(scope.branch, None);
    }

    #[test]
    fn workspace_snapshot_preserves_memory_scope_hash_inputs() {
        let root = axon_test_support::test_temp_dir();
        let nested = root.path().join("nested");
        std::fs::create_dir_all(&nested).expect("创建嵌套目录");
        run_git(root.path(), &["init", "-q"]);
        run_git(
            root.path(),
            &["remote", "add", "origin", "https://example.test/axon.git"],
        );
        let snapshot = axon_session::WorkspaceResolver
            .resolve_snapshot(&nested, 1)
            .expect("解析 Workspace 快照");
        let branch = snapshot.git.branch.as_deref().expect("应解析当前分支");

        let scope = memory_scope(Some(&snapshot));
        let workspace_ref = sha256_tagged_ref(
            "workspace",
            snapshot.requested_cwd.to_string_lossy().as_bytes(),
        );
        let branch_ref = sha256_tagged_ref("branch", branch.as_bytes());

        assert_eq!(scope.namespace, format!("{workspace_ref}:{branch_ref}"));
        assert_eq!(scope.branch.as_deref(), Some(branch_ref.as_str()));
        assert_eq!(
            scope.repo,
            Some(sha256_tagged_ref("repo", b"https://example.test/axon.git"))
        );
    }

    #[test]
    fn workspace_without_remote_keeps_git_root_repo_fallback() {
        let root = axon_test_support::test_temp_dir();
        run_git(root.path(), &["init", "-q"]);
        let snapshot = axon_session::WorkspaceResolver
            .resolve_snapshot(root.path(), 1)
            .expect("解析 Workspace 快照");
        let repo = snapshot.workspace_root.to_string_lossy().replace('\\', "/");

        let scope = memory_scope(Some(&snapshot));

        assert_eq!(scope.repo, Some(sha256_tagged_ref("repo", repo.as_bytes())));
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let status = axon_process::non_interactive_command("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("Git 命令可执行");
        assert!(status.success(), "Git fixture 命令失败：{args:?}");
    }

    fn test_hit(memory_id: u64) -> MemoryHybridHit {
        MemoryHybridHit {
            record: MemorySqliteRecord {
                memory_kind: MemoryKind::Knowledge,
                record: MemoryRecord {
                    id: axon_domain::MemoryId(memory_id),
                    candidate_id: MemoryCandidateId(memory_id),
                    state: MemoryState::Strong,
                    content_ref: Some(format!("memory-ref:test-{memory_id}")),
                    source_trace: None,
                    tombstone: None,
                },
                namespace_ref: Some(MEMORY_GLOBAL_NAMESPACE_REF.to_owned()),
                repo_ref: None,
                user_ref: None,
                created_at_ms: memory_id,
                updated_at_ms: memory_id,
                last_active_at_ms: memory_id,
                ttl_expires_at_ms: None,
                confidence_basis_points: 9_000,
                importance_basis_points: 8_000,
                vector_id: None,
                archived_at_ms: None,
                prompt_text_persisted: false,
                raw_payload_persisted: false,
                remote_write_allowed: false,
            },
            score_basis_points: 1,
            text_score: None,
            vector_distance: None,
            text_rank: None,
            vector_rank: None,
            explain: axon_memory::MemoryHybridExplain {
                text_component_basis_points: 0,
                vector_component_basis_points: 0,
                governance_component_basis_points: 1,
                weights: "test",
            },
        }
    }
}
