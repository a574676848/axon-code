//! Canonical builtin Memory 工具执行边界。

use std::{path::Path, time::Duration};

use axon_config::AxonConfig;
use axon_domain::{
    AxonError, AxonOwnerId, AxonResult, CellId, MemoryCandidate, MemoryCandidateId,
    MemoryCandidateKind, MemoryId, MemoryKind, MemoryRetentionPolicy, MemorySensitivity,
    MemorySourceOrigin, MemorySourceTrace, MemoryVerification, SessionId,
};
use axon_memory_read::{MemoryHybridScope, memory_query_summary, memory_write_embedding};
use axon_memory_write::{
    MemoryWriteTriggerInput, MemoryWriteTriggerReport, MemoryWriteTriggerRuntime,
    personal_memory_scope,
};
use sha2::{Digest, Sha256};

use crate::{AgentRunToolResultReceipt, ProviderToolCall, agent_run_completed_tool_receipt};

const MEMORY_TOOL_CONFIDENCE_BASIS_POINTS: u16 = 8_500;
const MEMORY_TOOL_IMPORTANCE_BASIS_POINTS: u16 = 8_000;
const SQLITE_MAX_SAFE_ID: u64 = 9_223_372_036_854_775_806;

/// 受信任的 Memory 运行时作用域。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryToolScope {
    /// Coding Agent 的 owner-scoped Personal Memory。
    Personal {
        /// 稳定 AxonOwnerId。
        owner_id: AxonOwnerId,
    },
    /// Coding Agent 的 workspace-scoped Memory。
    Workspace {
        /// 已由 Coding Agent runtime 绑定的 Workspace scope。
        scope: axon_memory_write::MemoryWriteScope,
    },
}

impl MemoryToolScope {
    fn hybrid_scope(&self) -> AxonResult<MemoryHybridScope> {
        match self {
            Self::Personal { owner_id } => {
                let scope = personal_memory_scope(owner_id.as_str());
                let user_ref = scope.user_ref.ok_or_else(|| {
                    AxonError::tool_invalid_arguments("Personal Memory 缺少稳定 owner")
                })?;
                Ok(MemoryHybridScope {
                    user_ref: Some(user_ref),
                    namespace_ref: scope.namespace_ref,
                })
            }
            Self::Workspace { scope } => Ok(MemoryHybridScope {
                user_ref: scope.user_ref.clone(),
                namespace_ref: scope.namespace_ref.clone(),
            }),
        }
    }
}

/// canonical Memory 查询输入；scope 只能由可信运行时构造。
pub struct CanonicalMemorySearchInput<'a> {
    /// Axon 配置路径。
    pub config_path: &'a Path,
    /// 已加载配置。
    pub config: &'a AxonConfig,
    /// 可信作用域。
    pub scope: &'a MemoryToolScope,
    /// 语义查询。
    pub query: &'a str,
    /// 最大返回条目数。
    pub limit: usize,
    /// 查询时间。
    pub now_ms: u64,
}

/// canonical Memory 写入输入；scope 只能由可信运行时构造。
pub struct CanonicalMemoryWriteInput<'a> {
    /// Axon 配置路径。
    pub config_path: &'a Path,
    /// 已加载配置。
    pub config: &'a AxonConfig,
    /// 可信作用域。
    pub scope: &'a MemoryToolScope,
    /// 会话 ID。
    pub conversation_id: &'a str,
    /// turn ID。
    pub turn_id: &'a str,
    /// 记忆类型。
    pub kind: &'a str,
    /// 记忆摘要。
    pub summary: &'a str,
    /// 证据摘要。
    pub evidence: &'a str,
    /// 被覆盖的真实 Memory ID。
    pub supersedes_memory_ids: Vec<MemoryId>,
    /// 写入时间。
    pub now_ms: u64,
}

/// canonical Memory 工具执行器。
#[derive(Debug, Clone, Copy)]
pub struct MemoryToolDispatcher {
    timeout: Duration,
}

impl MemoryToolDispatcher {
    /// 创建执行器。
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// 执行当前 scope 下的 memory_search 或 memory_write。
    pub fn dispatch(
        &self,
        config_path: &Path,
        config: &AxonConfig,
        scope: &MemoryToolScope,
        conversation_id: &str,
        turn_id: &str,
        call: &ProviderToolCall,
    ) -> AxonResult<AgentRunToolResultReceipt> {
        match call.dispatch_name() {
            "builtin.memory_search" => self.search(config_path, config, scope, call),
            "builtin.memory_write" => {
                self.write(config_path, config, scope, conversation_id, turn_id, call)
            }
            _ => Err(AxonError::tool_invalid_definition(
                "非 Memory canonical tool",
            )),
        }
    }

    fn search(
        &self,
        config_path: &Path,
        config: &AxonConfig,
        scope: &MemoryToolScope,
        call: &ProviderToolCall,
    ) -> AxonResult<AgentRunToolResultReceipt> {
        let value = call.input.as_json().ok_or_else(|| {
            AxonError::tool_invalid_arguments("builtin.memory_search 需要 JSON 参数")
        })?;
        let query = value
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AxonError::tool_invalid_arguments("memory_search query 必须非空"))?;
        let limit = value
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(Ok(crate::MEMORY_SEARCH_DEFAULT_LIMIT), |value| {
                usize::try_from(value)
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| {
                        AxonError::tool_invalid_arguments("memory_search limit 必须为正整数")
                    })
            })?;
        let summary = memory_query_summary(
            config_path,
            config,
            query,
            limit,
            now_ms(),
            self.timeout,
            Some(scope.hybrid_scope()?),
        )
        .map_err(AxonError::runtime_protocol)?;
        let projection = agent_run_completed_tool_receipt(call, summary, None, false);
        Ok(receipt(projection))
    }

    /// 在单个可信 scope 内执行 canonical Memory 查询。
    pub fn search_canonical(&self, input: CanonicalMemorySearchInput<'_>) -> AxonResult<String> {
        let query = input.query.trim();
        if query.is_empty() || input.limit == 0 {
            return Err(AxonError::tool_invalid_arguments(
                "memory_search query 与 limit 无效",
            ));
        }
        memory_query_summary(
            input.config_path,
            input.config,
            query,
            input.limit,
            input.now_ms,
            self.timeout,
            Some(input.scope.hybrid_scope()?),
        )
        .map_err(AxonError::runtime_protocol)
    }

    fn write(
        &self,
        config_path: &Path,
        config: &AxonConfig,
        scope: &MemoryToolScope,
        conversation_id: &str,
        turn_id: &str,
        call: &ProviderToolCall,
    ) -> AxonResult<AgentRunToolResultReceipt> {
        let value = call.input.as_json().ok_or_else(|| {
            AxonError::tool_invalid_arguments("builtin.memory_write 需要 JSON 参数")
        })?;
        let kind = value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let summary = non_blank(value, "summary")?;
        let evidence = non_blank(value, "evidence")?;
        let supersedes_memory_ids = parse_ids(value.get("supersedes_memory_ids"))?;
        let report = self.write_canonical(CanonicalMemoryWriteInput {
            config_path,
            config,
            scope,
            conversation_id,
            turn_id,
            kind,
            summary,
            evidence,
            supersedes_memory_ids,
            now_ms: now_ms(),
        })?;
        let projection = agent_run_completed_tool_receipt(
            call,
            if report.record.is_some() {
                "Memory 已写入"
            } else {
                "Memory 未通过准入"
            },
            None,
            false,
        );
        Ok(receipt(projection))
    }

    /// 通过统一准入、SQLite 与索引投影流水线写入 Memory。
    pub fn write_canonical(
        &self,
        input: CanonicalMemoryWriteInput<'_>,
    ) -> AxonResult<MemoryWriteTriggerReport> {
        validate_kind_scope(input.scope, input.kind)?;
        let summary = input.summary.trim();
        let evidence = input.evidence.trim();
        if summary.is_empty() || evidence.is_empty() {
            return Err(AxonError::tool_invalid_arguments(
                "memory_write summary 与 evidence 必须非空",
            ));
        }
        let scope_value = scope_value(input.scope)?;
        let candidate_kind = candidate_kind(input.kind)?;
        let memory_kind = memory_kind(input.kind)?;
        let identity = stable_id(&format!(
            "{}:{}:{}:{}",
            input.conversation_id, input.turn_id, input.kind, summary
        ));
        let candidate = MemoryCandidate {
            id: MemoryCandidateId(identity),
            source_trace: Some(MemorySourceTrace {
                origin: source_origin(input.kind),
                session_id: Some(SessionId(stable_id(input.conversation_id))),
                cell_id: Some(CellId(stable_id(input.turn_id))),
                summary: summary.to_owned(),
                evidence_ref: Some(format!("memory-tool:{}", digest(evidence))),
            }),
            candidate_kind,
            content_ref: format!("memory-tool:{}", digest(summary)),
            confidence_basis_points: MEMORY_TOOL_CONFIDENCE_BASIS_POINTS,
            sensitivity: if matches!(input.scope, MemoryToolScope::Personal { .. }) {
                MemorySensitivity::Personal
            } else {
                MemorySensitivity::Internal
            },
            proposed_retention: if matches!(input.scope, MemoryToolScope::Personal { .. }) {
                MemoryRetentionPolicy::LongTerm
            } else {
                MemoryRetentionPolicy::Project
            },
            verification: MemoryVerification::AgentVerified,
        };
        let orchestrator =
            axon_memory_read::init_memory_orchestrator(input.config_path, input.config)
                .map_err(AxonError::runtime_protocol)?;
        let embedding = memory_write_embedding(input.config, summary, self.timeout);
        MemoryWriteTriggerRuntime
            .trigger(
                &orchestrator,
                &MemoryWriteTriggerInput {
                    candidate,
                    memory_id: MemoryId(identity.saturating_add(2)),
                    memory_kind,
                    supersedes_memory_ids: input.supersedes_memory_ids,
                    scope: scope_value,
                    now_ms: input.now_ms,
                    ttl_expires_at_ms: None,
                    importance_basis_points: MEMORY_TOOL_IMPORTANCE_BASIS_POINTS,
                    embedding: embedding.embedding,
                    embedding_model_ref: embedding.model_ref,
                },
            )
            .map_err(|error| AxonError::runtime_protocol(format!("{error:?}")))
    }
}

fn validate_kind_scope(scope: &MemoryToolScope, kind: &str) -> AxonResult<()> {
    let valid = matches!(
        kind,
        "profile" | "preference" | "experience" | "knowledge" | "procedure"
    );
    if !valid {
        return Err(AxonError::tool_invalid_arguments("memory_write kind 无效"));
    }
    if matches!(scope, MemoryToolScope::Personal { .. })
        && !matches!(kind, "profile" | "preference")
    {
        return Err(AxonError::tool_invalid_arguments(
            "Personal Memory 只允许 profile 或 preference",
        ));
    }
    if matches!(scope, MemoryToolScope::Workspace { .. })
        && matches!(kind, "profile" | "preference")
    {
        return Err(AxonError::tool_invalid_arguments(
            "profile 或 preference 必须写入 Personal Memory",
        ));
    }
    Ok(())
}

fn scope_value(scope: &MemoryToolScope) -> AxonResult<axon_memory_write::MemoryWriteScope> {
    match scope {
        MemoryToolScope::Personal { owner_id } => Ok(personal_memory_scope(owner_id.as_str())),
        MemoryToolScope::Workspace { scope } if scope.user_ref.is_none() => Ok(scope.clone()),
        MemoryToolScope::Workspace { .. } => Err(AxonError::tool_invalid_arguments(
            "Workspace Memory 不得携带 Personal owner",
        )),
    }
}

fn candidate_kind(kind: &str) -> AxonResult<MemoryCandidateKind> {
    match kind {
        "profile" => Ok(MemoryCandidateKind::Profile),
        "preference" => Ok(MemoryCandidateKind::UserPreference),
        "experience" => Ok(MemoryCandidateKind::Experience),
        "knowledge" => Ok(MemoryCandidateKind::Knowledge),
        "procedure" => Ok(MemoryCandidateKind::ProceduralMemory),
        _ => Err(AxonError::tool_invalid_arguments("memory_write kind 无效")),
    }
}

fn memory_kind(kind: &str) -> AxonResult<MemoryKind> {
    MemoryKind::from_code(kind)
        .ok_or_else(|| AxonError::tool_invalid_arguments("memory_write kind 无效"))
}

fn source_origin(kind: &str) -> MemorySourceOrigin {
    match kind {
        "profile" | "preference" => MemorySourceOrigin::AgentUserContext,
        "experience" => MemorySourceOrigin::AgentExperience,
        "knowledge" => MemorySourceOrigin::AgentKnowledge,
        "procedure" => MemorySourceOrigin::AgentProcedure,
        _ => MemorySourceOrigin::AgentKnowledge,
    }
}

fn non_blank<'a>(value: &'a serde_json::Value, field: &str) -> AxonResult<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AxonError::tool_invalid_arguments(format!("memory_write {field} 必须非空")))
}

fn parse_ids(value: Option<&serde_json::Value>) -> AxonResult<Vec<MemoryId>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .map_or_else(|| std::slice::from_ref(value), |items| items);
    items
        .iter()
        .map(|item| {
            item.as_u64()
                .or_else(|| {
                    item.as_str().and_then(|text| {
                        let text = text.trim();
                        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
                            .then(|| text.parse::<u64>().ok())
                            .flatten()
                    })
                })
                .filter(|id| *id > 0)
                .map(MemoryId)
                .ok_or_else(|| {
                    AxonError::tool_invalid_arguments("supersedes_memory_ids 包含无效 ID")
                })
        })
        .collect()
}

fn stable_id(value: &str) -> u64 {
    let bytes = Sha256::digest(value.as_bytes());
    (u64::from_be_bytes(bytes[..8].try_into().expect("digest")) % SQLITE_MAX_SAFE_ID) + 1
}

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn receipt(projection: crate::AgentRunToolReceiptProjection) -> AgentRunToolResultReceipt {
    AgentRunToolResultReceipt {
        provider_call_id: projection.provider_call_id,
        tool_name: projection.tool_name,
        status: projection.status,
        summary: projection.summary,
        metadata: projection.metadata,
        output_ref: projection.output_ref,
        error_type: projection.error_type,
        remote_trigger_allowed: projection.remote_trigger_allowed,
        filesystem_write_performed: projection.filesystem_write_performed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_tool_scope_maps_to_hybrid_scope() {
        let personal = MemoryToolScope::Personal {
            owner_id: AxonOwnerId::parse("owner_550e8400-e29b-41d4-a716-446655440000")
                .expect("owner"),
        }
        .hybrid_scope()
        .expect("personal scope");
        assert!(personal.user_ref.is_some());

        let scope = MemoryToolScope::Workspace {
            scope: axon_memory_write::MemoryWriteScope {
                namespace_ref: "workspace:repo".to_owned(),
                repo_ref: Some("repo".to_owned()),
                user_ref: None,
            },
        }
        .hybrid_scope()
        .expect("workspace scope");
        assert!(scope.user_ref.is_none());
        assert_eq!(scope.namespace_ref, "workspace:repo");
    }

    #[test]
    fn canonical_write_accepts_user_authorized_sensitive_plaintext() {
        let temp = axon_test_support::test_temp_dir();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "").expect("创建测试配置");
        let config = AxonConfig::default();
        let scope = MemoryToolScope::Workspace {
            scope: axon_memory_write::MemoryWriteScope {
                namespace_ref: "workspace:plaintext-memory".to_owned(),
                repo_ref: Some("plaintext-memory".to_owned()),
                user_ref: None,
            },
        };
        let summary = "测试账号 admin，密码 p@ssw0rd，token sk-test-plaintext";
        let report = MemoryToolDispatcher::new(Duration::from_secs(1))
            .write_canonical(CanonicalMemoryWriteInput {
                config_path: &config_path,
                config: &config,
                scope: &scope,
                conversation_id: "conversation-plaintext",
                turn_id: "turn-plaintext",
                kind: "knowledge",
                summary,
                evidence: "用户明确授权保存该敏感信息",
                supersedes_memory_ids: Vec::new(),
                now_ms: 1,
            })
            .expect("敏感明文应进入普通 Memory");
        assert_eq!(
            report
                .record
                .as_ref()
                .and_then(|record| record.source_trace.as_ref())
                .map(|source| source.summary.as_str()),
            Some(summary)
        );
    }
}
