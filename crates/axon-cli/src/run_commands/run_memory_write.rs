//! CLI builtin Memory 读写适配器。

use std::path::{Path, PathBuf};
use std::time::Duration;

use axon_agent_runtime::{
    CanonicalMemorySearchInput, CanonicalMemoryWriteInput, MemoryToolDispatcher, MemoryToolScope,
    MemoryWriteTriggerReport, memory_scope_refs, memory_scope_refs_with_namespace,
};
use axon_config::ConfigLoader;
use axon_domain::{AxonOwnerId, CellId, MemoryId, SessionId};
use serde_json::{Value, json};

const MEMORY_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const PERSONAL_SCOPE_LABEL: &str = "personal";
const WORKSPACE_SCOPE_LABEL: &str = "workspace";
const MEMORY_SEARCH_SCOPE_FIELD: &str = "scope";
const MEMORY_SEARCH_HITS_FIELD: &str = "hits";
const MEMORY_SEARCH_SCORE_FIELD: &str = "scoreBasisPoints";
const MEMORY_SEARCH_ID_FIELD: &str = "memoryId";
const MEMORY_SEARCH_EMBEDDING_STATUS_FIELD: &str = "embeddingStatus";
const MEMORY_SEARCH_EMBEDDING_REASON_FIELD: &str = "embeddingReason";

pub(crate) struct AgentMemorySearchInput<'a> {
    pub(crate) config_path: &'a Path,
    pub(crate) cwd: &'a Path,
    pub(crate) query: &'a str,
    pub(crate) limit: usize,
    pub(crate) now_ms: u64,
    pub(crate) timeout: Duration,
    pub(crate) is_root_agent: bool,
}

pub(crate) struct AgentMemoryWriteInput<'a> {
    pub(crate) config_path: &'a Path,
    pub(crate) cwd: &'a Path,
    pub(crate) session_id: SessionId,
    pub(crate) cell_id: CellId,
    pub(crate) kind: &'a str,
    pub(crate) summary: &'a str,
    pub(crate) evidence: &'a str,
    pub(crate) supersedes_memory_ids: &'a [u64],
    pub(crate) now_ms: u64,
    pub(crate) is_root_agent: bool,
}

/// 将 CLI tool loop 绑定到 canonical Memory dispatcher。
#[derive(Debug, Clone, Default)]
pub(crate) struct AgentMemoryWriteRuntime {
    owner_id: Option<AxonOwnerId>,
    principal_diagnostic: Option<String>,
    /// 本机 Axon 根；缺失 owner identity 时用于在首次 Personal 写入处按安装器合同补齐。
    identity_root: Option<PathBuf>,
}

impl AgentMemoryWriteRuntime {
    pub(crate) fn new(owner_id: Option<AxonOwnerId>, principal_diagnostic: Option<String>) -> Self {
        Self {
            owner_id,
            principal_diagnostic,
            identity_root: None,
        }
    }

    /// 绑定本机 Axon 根，使 Personal scope 的首次写入可以补齐 owner identity。
    pub(crate) fn with_identity_root(mut self, identity_root: Option<PathBuf>) -> Self {
        self.identity_root = identity_root;
        self
    }

    pub(crate) fn trigger(
        &self,
        input: AgentMemoryWriteInput<'_>,
    ) -> Result<MemoryWriteTriggerReport, String> {
        let namespace = self.registry_namespace(input.config_path, input.cwd);
        if !matches!(input.kind, "profile" | "preference") && namespace.is_none() {
            return Err(
                "当前工作目录不可访问，未能解析 Memory 作用域；请确认目录存在后重试".to_owned(),
            );
        }
        let scope = self.resolve_scope_with_namespace(
            input.kind,
            input.cwd,
            input.is_root_agent,
            namespace,
        )?;
        let loaded = ConfigLoader::load_from_path(input.config_path)
            .map_err(|error| format!("builtin.memory_write 配置加载失败: {error:?}"))?;
        MemoryToolDispatcher::new(MEMORY_WRITE_TIMEOUT)
            .write_canonical(CanonicalMemoryWriteInput {
                config_path: input.config_path,
                config: &loaded.config,
                scope: &scope,
                conversation_id: &input.session_id.0.to_string(),
                turn_id: &input.cell_id.0.to_string(),
                kind: input.kind,
                summary: input.summary,
                evidence: input.evidence,
                supersedes_memory_ids: input
                    .supersedes_memory_ids
                    .iter()
                    .copied()
                    .map(MemoryId)
                    .collect(),
                now_ms: input.now_ms,
            })
            .map_err(|error| format!("{error:?}"))
    }

    pub(crate) fn search(&self, input: AgentMemorySearchInput<'_>) -> Result<String, String> {
        let loaded = ConfigLoader::load_from_path(input.config_path)
            .map_err(|error| format!("builtin.memory_search 配置加载失败: {error:?}"))?;
        let dispatcher = MemoryToolDispatcher::new(input.timeout);
        let mut scoped_summaries = Vec::new();
        for (scope_label, scope) in self.resolve_search_scopes_with_namespace(
            input.config_path,
            input.cwd,
            input.is_root_agent,
        ) {
            let summary = dispatcher
                .search_canonical(CanonicalMemorySearchInput {
                    config_path: input.config_path,
                    config: &loaded.config,
                    scope: &scope,
                    query: input.query,
                    limit: input.limit,
                    now_ms: input.now_ms,
                })
                .map_err(|error| format!("{error:?}"))?;
            scoped_summaries.push((scope_label, summary));
        }
        let merged = merge_memory_search_summaries(
            input.query,
            input.limit,
            scoped_summaries,
            self.personal_search_status(input.is_root_agent),
        )?;
        Ok(merged)
    }

    fn resolve_scope_with_namespace(
        &self,
        kind: &str,
        cwd: &Path,
        is_root_agent: bool,
        namespace: Option<String>,
    ) -> Result<MemoryToolScope, String> {
        if matches!(kind, "profile" | "preference") {
            if !is_root_agent {
                return Err("子代理不得写入 Personal Memory".to_owned());
            }
            let owner_id = self.owner_identity()?;
            return Ok(MemoryToolScope::Personal { owner_id });
        }
        Ok(MemoryToolScope::Workspace {
            scope: namespace.map_or_else(
                || memory_scope_refs(cwd),
                |namespace_ref| memory_scope_refs_with_namespace(cwd, &namespace_ref),
            ),
        })
    }

    fn resolve_search_scopes_with_namespace(
        &self,
        config_path: &Path,
        cwd: &Path,
        is_root_agent: bool,
    ) -> Vec<(&'static str, MemoryToolScope)> {
        let namespace = self.registry_namespace(config_path, cwd);
        if namespace.is_some() {
            return self.resolve_search_scopes_with_namespace_inner(cwd, is_root_agent, namespace);
        }
        let mut scopes = Vec::with_capacity(1);
        if is_root_agent && let Some(owner_id) = self.owner_id.clone() {
            scopes.push((PERSONAL_SCOPE_LABEL, MemoryToolScope::Personal { owner_id }));
        }
        scopes
    }

    fn resolve_search_scopes_with_namespace_inner(
        &self,
        cwd: &Path,
        is_root_agent: bool,
        namespace: Option<String>,
    ) -> Vec<(&'static str, MemoryToolScope)> {
        let mut scopes = Vec::with_capacity(2);
        if is_root_agent && let Some(owner_id) = self.owner_id.clone() {
            scopes.push((PERSONAL_SCOPE_LABEL, MemoryToolScope::Personal { owner_id }));
        }
        scopes.push((
            WORKSPACE_SCOPE_LABEL,
            MemoryToolScope::Workspace {
                scope: namespace.map_or_else(
                    || memory_scope_refs(cwd),
                    |namespace_ref| memory_scope_refs_with_namespace(cwd, &namespace_ref),
                ),
            },
        ));
        scopes
    }

    fn registry_namespace(&self, config_path: &Path, cwd: &Path) -> Option<String> {
        match crate::runtime_tools::resolve_workspace_cwd(config_path, Some(cwd)) {
            Ok(Some(workspace_root)) => super::run_workspace_identity::workspace_record(
                &workspace_root,
                super::run_execution::now_ms(),
            )
            .ok()
            .map(|workspace| workspace.memory_namespace),
            // Home 目录树内没有可注册的普通 Workspace，但经验、知识与流程仍必须落在一个
            // 稳定的桶里：取 Axon 主目录的固定 namespace，与个人画像严格分开，也让检索
            // 与写入指向同一个位置。
            Ok(None) => Some(axon_memory::AXON_HOME_MEMORY_NAMESPACE.to_owned()),
            Err(_) => None,
        }
    }

    fn personal_search_status(&self, is_root_agent: bool) -> Value {
        if !is_root_agent {
            return json!({ "loaded": false, "reason": "subagent_scope" });
        }
        match (&self.owner_id, &self.principal_diagnostic) {
            (Some(_), _) => json!({ "loaded": true }),
            (None, Some(reason)) => json!({ "loaded": false, "reason": reason }),
            (None, None) => json!({ "loaded": false, "reason": "owner_identity_missing" }),
        }
    }

    /// 解析本机 owner identity；缺失时在首次 Personal 写入处按安装器合同补齐。
    ///
    /// 打包安装器之外的安装路径（`cargo install --path crates/axon-cli`）从不写
    /// `identity/owner.json`；不在这里补齐，Personal Memory 会在整台机器上永久关闭。
    fn owner_identity(&self) -> Result<AxonOwnerId, String> {
        if let Some(owner_id) = self.owner_id.clone() {
            return Ok(owner_id);
        }
        let Some(identity_root) = self.identity_root.as_deref() else {
            return Err(self.owner_identity_diagnostic());
        };
        crate::runtime_principal_context::load_or_provision_owner_id(identity_root)
    }

    fn owner_identity_diagnostic(&self) -> String {
        self.principal_diagnostic
            .clone()
            .unwrap_or_else(|| "Personal Memory 已关闭：本机 owner identity 不存在".to_owned())
    }
}

fn merge_memory_search_summaries(
    query: &str,
    limit: usize,
    scoped_summaries: Vec<(&'static str, String)>,
    personal_status: Value,
) -> Result<String, String> {
    let mut hits = Vec::new();
    let mut embeddings = Vec::with_capacity(scoped_summaries.len());
    let mut source_truncated = false;
    for (scope_label, summary) in scoped_summaries {
        source_truncated |=
            merge_scoped_memory_summary(scope_label, &summary, &mut hits, &mut embeddings)?;
    }
    hits.sort_by(compare_memory_hits);
    let truncated = source_truncated || hits.len() > limit;
    hits.truncate(limit);
    Ok(json!({
        "scope": "trusted_personal_and_workspace",
        "query": query,
        "hits": hits,
        "embeddings": embeddings,
        "personal": personal_status,
        "truncated": truncated,
    })
    .to_string())
}

fn merge_scoped_memory_summary(
    scope_label: &'static str,
    summary: &str,
    hits: &mut Vec<Value>,
    embeddings: &mut Vec<Value>,
) -> Result<bool, String> {
    let mut payload: Value = serde_json::from_str(summary)
        .map_err(|error| format!("Memory 查询结果无法解析: {error}"))?;
    let truncated = payload
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    embeddings.push(json!({
        MEMORY_SEARCH_SCOPE_FIELD: scope_label,
        MEMORY_SEARCH_EMBEDDING_STATUS_FIELD: payload
            .get(MEMORY_SEARCH_EMBEDDING_STATUS_FIELD)
            .cloned()
            .unwrap_or(Value::Null),
        MEMORY_SEARCH_EMBEDDING_REASON_FIELD: payload
            .get(MEMORY_SEARCH_EMBEDDING_REASON_FIELD)
            .cloned()
            .unwrap_or(Value::Null),
    }));
    if let Some(entries) = payload
        .get_mut(MEMORY_SEARCH_HITS_FIELD)
        .and_then(Value::as_array_mut)
    {
        for mut hit in entries.drain(..) {
            if let Some(fields) = hit.as_object_mut() {
                fields.insert(
                    MEMORY_SEARCH_SCOPE_FIELD.to_owned(),
                    Value::String(scope_label.to_owned()),
                );
                hits.push(hit);
            }
        }
    }
    Ok(truncated)
}

fn compare_memory_hits(left: &Value, right: &Value) -> std::cmp::Ordering {
    memory_hit_score(right)
        .cmp(&memory_hit_score(left))
        .then_with(|| memory_hit_id(left).cmp(&memory_hit_id(right)))
        .then_with(|| memory_hit_scope(left).cmp(memory_hit_scope(right)))
}

fn memory_hit_score(hit: &Value) -> u64 {
    hit.get(MEMORY_SEARCH_SCORE_FIELD)
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

fn memory_hit_id(hit: &Value) -> u64 {
    hit.get(MEMORY_SEARCH_ID_FIELD)
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

fn memory_hit_scope(hit: &Value) -> &str {
    hit.get(MEMORY_SEARCH_SCOPE_FIELD)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner_id() -> AxonOwnerId {
        AxonOwnerId::parse("owner_550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    #[test]
    fn profile_and_preference_use_personal_scope() {
        let runtime = AgentMemoryWriteRuntime::new(Some(owner_id()), None);
        let scope = runtime
            .resolve_scope_with_namespace("preference", Path::new("E:/workspace"), true, None)
            .unwrap();
        assert!(matches!(scope, MemoryToolScope::Personal { .. }));
    }

    #[test]
    fn subagent_cannot_write_personal_memory() {
        let runtime = AgentMemoryWriteRuntime::new(Some(owner_id()), None);
        assert!(
            runtime
                .resolve_scope_with_namespace("profile", Path::new("E:/workspace"), false, None)
                .is_err()
        );
    }

    #[test]
    fn missing_owner_identity_is_provisioned_on_first_personal_write() {
        let root = axon_test_support::test_temp_dir();
        let runtime = AgentMemoryWriteRuntime::new(None, None)
            .with_identity_root(Some(root.path().to_path_buf()));

        let scope = runtime
            .resolve_scope_with_namespace("preference", Path::new("E:/workspace"), true, None)
            .expect("首次 Personal 写入应补齐 owner identity");

        match scope {
            MemoryToolScope::Personal { owner_id } => {
                assert!(owner_id.as_str().starts_with("owner_"));
            }
            MemoryToolScope::Workspace { .. } => panic!("Preference 必须进入 Personal scope"),
        }
        assert!(
            root.path().join("identity").join("owner.json").is_file(),
            "补齐必须落在安装器同一身份文件上"
        );
    }

    #[test]
    fn personal_write_without_identity_root_keeps_fail_closed() {
        let runtime = AgentMemoryWriteRuntime::new(None, None);
        let error = runtime
            .resolve_scope_with_namespace("profile", Path::new("E:/workspace"), true, None)
            .expect_err("未绑定 Axon 根时必须保持 fail-closed");
        assert!(error.contains("owner identity"));
    }

    #[test]
    fn non_personal_kinds_use_workspace_scope_without_owner() {
        let runtime = AgentMemoryWriteRuntime::new(Some(owner_id()), None);
        let scope = runtime
            .resolve_scope_with_namespace("experience", Path::new("E:/workspace"), true, None)
            .unwrap();
        match scope {
            MemoryToolScope::Workspace { scope } => assert!(scope.user_ref.is_none()),
            MemoryToolScope::Personal { .. } => panic!("Experience 不得进入 Personal scope"),
        }
    }

    #[test]
    fn subagent_searches_workspace_only() {
        let runtime = AgentMemoryWriteRuntime::new(Some(owner_id()), None);
        let scopes = runtime.resolve_search_scopes_with_namespace_inner(
            Path::new("E:/workspace"),
            false,
            None,
        );
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].0, WORKSPACE_SCOPE_LABEL);
        assert!(matches!(scopes[0].1, MemoryToolScope::Workspace { .. }));
    }

    #[test]
    fn root_agent_searches_personal_and_workspace() {
        let runtime = AgentMemoryWriteRuntime::new(Some(owner_id()), None);
        let scopes = runtime.resolve_search_scopes_with_namespace_inner(
            Path::new("E:/workspace"),
            true,
            None,
        );
        assert_eq!(
            scopes.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
            vec![PERSONAL_SCOPE_LABEL, WORKSPACE_SCOPE_LABEL]
        );
    }

    #[test]
    fn home_search_includes_the_axon_home_workspace_scope() {
        let root = axon_test_support::test_temp_dir();
        let home = root.path().join("home");
        let cwd = home.join("data");
        std::fs::create_dir_all(&cwd).expect("创建 Home 子目录");
        let config_path = home.join("config.toml");
        let runtime = AgentMemoryWriteRuntime::new(Some(owner_id()), None);

        let scopes = runtime.resolve_search_scopes_with_namespace(&config_path, &cwd, true);

        assert_eq!(
            scopes.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
            vec![PERSONAL_SCOPE_LABEL, WORKSPACE_SCOPE_LABEL],
            "Home 检索必须同时覆盖个人记忆与 Axon 主目录桶"
        );
        match &scopes[1].1 {
            MemoryToolScope::Workspace { scope } => assert_eq!(
                scope.namespace_ref,
                axon_memory::AXON_HOME_MEMORY_NAMESPACE,
                "主目录记忆必须落在固定的 Axon 主目录桶"
            ),
            MemoryToolScope::Personal { .. } => panic!("主目录检索不得退化为仅个人记忆"),
        }
    }

    #[test]
    fn home_workspace_memory_resolves_to_the_axon_home_bucket() {
        let root = axon_test_support::test_temp_dir();
        let home = root.path().join("home");
        let cwd = home.join("data");
        std::fs::create_dir_all(&cwd).expect("创建 Home 子目录");
        let config_path = home.join("config.toml");
        let runtime = AgentMemoryWriteRuntime::new(Some(owner_id()), None);

        assert_eq!(
            runtime.registry_namespace(&config_path, &cwd).as_deref(),
            Some(axon_memory::AXON_HOME_MEMORY_NAMESPACE),
            "在 ~/.axon 里写经验、知识与流程必须落在 Axon 主目录桶，而不是被拒绝或降级成个人记忆"
        );
    }

    #[test]
    fn merged_search_result_orders_and_truncates_hits() {
        let personal = json!({
            "hits": [{ "memoryId": 2, "scoreBasisPoints": 7000 }],
            "embeddingStatus": "ready",
            "embeddingReason": null,
            "truncated": false,
        })
        .to_string();
        let workspace = json!({
            "hits": [{ "memoryId": 1, "scoreBasisPoints": 9000 }],
            "embeddingStatus": "ready",
            "embeddingReason": null,
            "truncated": false,
        })
        .to_string();
        let merged = merge_memory_search_summaries(
            "query",
            1,
            vec![
                (PERSONAL_SCOPE_LABEL, personal),
                (WORKSPACE_SCOPE_LABEL, workspace),
            ],
            json!({ "loaded": true }),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&merged).unwrap();
        let hits = payload[MEMORY_SEARCH_HITS_FIELD].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0][MEMORY_SEARCH_ID_FIELD], 1);
        assert_eq!(hits[0][MEMORY_SEARCH_SCOPE_FIELD], WORKSPACE_SCOPE_LABEL);
        assert_eq!(payload["truncated"], true);
    }
}
