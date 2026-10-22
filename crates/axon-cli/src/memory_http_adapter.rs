//! 记忆管理 HTTP Gateway 适配器。
//!
//! 读路径（列表、统计、详情、scope 面）只触碰 SQLite 事实源：连接按请求新开，
//! schema 初始化在进程内只做一次，绝不为了读一页而重建 Tantivy/HNSW 运行时。
//! 写路径继续复用 `MemoryToolDispatcher` 这一条统一流水线；只有遗忘与删除需要
//! 清理投影，才启用完整的 `MemoryOrchestrator`，且进程内只构建一次。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use axon_agent_runtime::{
    CanonicalMemoryWriteInput, MemoryConfig, MemoryOrchestrator, MemorySqliteRecord,
    MemoryTimelineCursor, MemoryToolDispatcher, MemoryToolScope,
};
use axon_config::ConfigLoader;
use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_domain::{MemoryId, MemoryKind, MemoryState, MemoryTombstone};
use axon_memory::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryPipelineJobKind, MemoryPipelineStateStore,
    MemorySqliteStore, MemoryVisibility,
};
use serde_json::{Value, json};

use crate::memory_http_scope::{
    MemoryScopeError, is_personal_kind, personal_scope, scope_for_request, workspace_labels,
};

#[path = "memory_http_adapter_row.rs"]
mod memory_http_adapter_row;
pub(crate) use memory_http_adapter_row::{MemoryWriteItem, memory_view, parse_write_body};

const MEMORY_PATH: &str = "/api/v1/memory";
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
/// 管理页写入使用的会话标识；只用于写入溯源，不承载用户身份。
const MANAGEMENT_CONVERSATION_ID: &str = "desktop-memory-management";

/// 已完成 schema 初始化的 Memory 库路径集合。
///
/// 每请求重跑 `initialize_schema` 要执行一批 DDL 与 `CREATE INDEX`，
/// 是列表页延迟的主要来源；这里按库路径做进程内一次性初始化。
static MEMORY_SCHEMA_READY: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// 记忆管理 Gateway 适配器。
pub struct MemoryHttpAdapter {
    axon_root: PathBuf,
    config_path: PathBuf,
    /// 投影清理专用运行时；构建代价高（Tantivy + 可选 HNSW），进程内只建一次。
    projection_runtime: OnceLock<MemoryOrchestrator>,
}

impl MemoryHttpAdapter {
    /// 构造适配器。
    #[must_use]
    pub fn new(axon_root: impl Into<PathBuf>, config_path: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
            config_path: config_path.into(),
            projection_runtime: OnceLock::new(),
        }
    }

    /// 读取 Memory 物理配置；不初始化任何索引。
    fn memory_config(&self) -> Result<MemoryConfig, MemoryHttpError> {
        let loaded = ConfigLoader::load_from_path(&self.config_path)
            .map_err(|error| MemoryHttpError::internal(format!("加载 Axon 配置失败：{error:?}")))?;
        let dimension = if loaded.config.memory.embedding.enabled {
            loaded.config.memory.embedding.dimension
        } else {
            MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
        };
        MemoryConfig::from_config_dir(
            &self.axon_root,
            loaded.config.memory.embedding.enabled,
            dimension,
        )
        .map_err(|error| MemoryHttpError::internal(format!("构造 Memory 配置失败：{error:?}")))
    }

    /// 打开 SQLite 事实源；schema 在本进程内只初始化一次。
    fn store(&self) -> Result<MemorySqliteStore, MemoryHttpError> {
        let config = self.memory_config()?;
        let store = MemorySqliteStore::open(&config).map_err(|error| {
            MemoryHttpError::internal(format!("打开 Memory SQLite 失败：{error:?}"))
        })?;
        let mut ready = MEMORY_SCHEMA_READY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ready.contains(&config.db_path) {
            store.initialize_schema().map_err(|error| {
                MemoryHttpError::internal(format!("初始化 Memory SQLite 失败：{error:?}"))
            })?;
            ready.insert(config.db_path.clone());
        }
        Ok(store)
    }

    /// 投影清理运行时；首次使用时构建，之后复用同一份索引句柄。
    fn projection_runtime(&self) -> Result<&MemoryOrchestrator, MemoryHttpError> {
        if let Some(runtime) = self.projection_runtime.get() {
            return Ok(runtime);
        }
        let runtime = MemoryOrchestrator::init(self.memory_config()?).map_err(|error| {
            MemoryHttpError::internal(format!("初始化 Memory 运行时失败：{error:?}"))
        })?;
        // 并发首用只保留先到者；重复构建幂等。
        let _ = self.projection_runtime.set(runtime);
        self.projection_runtime
            .get()
            .ok_or_else(|| MemoryHttpError::internal("初始化 Memory 运行时失败"))
    }

    /// 解析当前登录 owner。
    fn owner(&self) -> Result<String, MemoryHttpError> {
        match personal_scope(&self.axon_root).map_err(MemoryHttpError::from_scope)? {
            MemoryToolScope::Personal { owner_id } => Ok(owner_id.as_str().to_owned()),
            MemoryToolScope::Workspace { .. } => {
                Err(MemoryHttpError::internal("Personal scope 解析失败"))
            }
        }
    }

    /// 解析请求的 scope 选择：`all`（默认）、`personal`、`workspace` 或 `workspace:<namespace>`。
    fn requested_scope(&self, query: Option<&str>) -> Result<ScopeChoice, MemoryHttpError> {
        let value = query_value(query, "scope").unwrap_or_default();
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
            return Ok(ScopeChoice::All);
        }
        if trimmed.eq_ignore_ascii_case("personal") {
            return Ok(ScopeChoice::Personal);
        }
        if trimmed.eq_ignore_ascii_case("workspace") {
            return Ok(ScopeChoice::Workspace);
        }
        trimmed
            .strip_prefix("workspace:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|namespace| ScopeChoice::Namespace(format!("workspace:{namespace}")))
            .ok_or_else(|| {
                MemoryHttpError::bad_request(
                    "scope 必须是 all、personal、workspace 或 workspace:<namespace>",
                )
            })
    }

    fn list(&self, query: Option<&str>) -> Result<Value, MemoryHttpError> {
        let kind = query_value(query, "memoryKind")
            .or_else(|| query_value(query, "kind"))
            .and_then(|value| MemoryKind::from_code(value.trim()))
            .ok_or_else(|| MemoryHttpError::bad_request("memoryKind 必须是五种稳定类型之一"))?;
        let limit = query_value(query, "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);
        let cursor = parse_cursor(query)?;
        let store = self.store()?;
        let owner = self.owner()?;
        let choice = self.requested_scope(query)?;
        let records = store
            .records_timeline_page(
                kind,
                choice.namespace_ref(),
                choice.visibility(&owner),
                cursor,
                limit,
            )
            .map_err(|error| {
                MemoryHttpError::internal(format!("读取 Memory 时间轴失败：{error:?}"))
            })?;
        let next_cursor = if records.len() == limit {
            records.last().map(|record| {
                json!({
                    "updatedAtMs": record.updated_at_ms,
                    "memoryId": record.record.id.0.to_string(),
                })
            })
        } else {
            None
        };
        Ok(json!({
            "status": "memory_ready",
            "memoryKind": kind.as_str(),
            "scope": choice.code(),
            "namespaceRef": choice.namespace_ref(),
            "limit": limit,
            "records": records.iter().map(memory_view).collect::<Vec<_>>(),
            "nextCursor": next_cursor,
            "needsRepair": self.needs_repair()?,
        }))
    }

    fn stats(&self, query: Option<&str>) -> Result<Value, MemoryHttpError> {
        let store = self.store()?;
        let owner = self.owner()?;
        let choice = self.requested_scope(query)?;
        let summary = store
            .memory_summary_scoped(choice.visibility(&owner), choice.namespace_ref())
            .map_err(|error| {
                MemoryHttpError::internal(format!("读取 Memory 汇总失败：{error:?}"))
            })?;
        let counts = store
            .memory_kind_counts(choice.visibility(&owner), choice.namespace_ref())
            .map_err(|error| {
                MemoryHttpError::internal(format!("读取 Memory 类型统计失败：{error:?}"))
            })?;
        Ok(json!({
            "status": "memory_ready",
            "scope": choice.code(),
            "namespaceRef": choice.namespace_ref(),
            "totalRecords": summary.total_records,
            "activeRecords": summary.active_records,
            "archivedRecords": summary.archived_records,
            "memoryKindCounts": counts.into_iter().map(|item| json!({
                "memoryKind": item.memory_kind.as_str(),
                "count": item.count,
            })).collect::<Vec<_>>(),
            "needsRepair": self.needs_repair()?,
            "legacyRecordTables": store.legacy_record_tables().map_err(|error| {
                MemoryHttpError::internal(format!("读取归档表失败：{error:?}"))
            })?,
        }))
    }

    fn scopes(&self) -> Result<Value, MemoryHttpError> {
        let store = self.store()?;
        let owner = self.owner()?;
        let labels = workspace_labels(&self.axon_root);
        let facets = store
            .memory_scope_facets(MemoryVisibility::OwnerScoped(&owner))
            .map_err(|error| {
                MemoryHttpError::internal(format!("读取 Memory scope 失败：{error:?}"))
            })?;
        let scopes = facets
            .into_iter()
            .map(|facet| {
                let personal = facet.scope_kind == "personal";
                json!({
                    "scope": if personal {
                        "personal".to_owned()
                    } else {
                        format!("workspace:{}", facet.namespace_ref)
                    },
                    "scopeKind": facet.scope_kind,
                    "namespaceRef": facet.namespace_ref,
                    "label": if personal {
                        "Personal（本机画像与偏好）".to_owned()
                    } else {
                        labels
                            .get(&facet.namespace_ref)
                            .cloned()
                            .unwrap_or_else(|| facet.namespace_ref.clone())
                    },
                    "recordCount": facet.record_count,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "status": "memory_ready",
            "scopes": scopes,
            "needsRepair": self.needs_repair()?,
        }))
    }

    fn get(&self, id: &str) -> Result<Value, MemoryHttpError> {
        let memory_id = parse_memory_id(id)?;
        let store = self.store()?;
        let record = store
            .get_record(memory_id)
            .map_err(|error| MemoryHttpError::internal(format!("读取 Memory 详情失败：{error:?}")))?
            .ok_or_else(|| MemoryHttpError::not_found("未找到 Memory 记录"))?;
        self.ensure_visible(&record)?;
        Ok(
            json!({ "status": "memory_ready", "record": memory_view(&record), "needsRepair": false }),
        )
    }

    fn write(&self, body: &[u8], supersedes: Vec<MemoryId>) -> Result<Value, MemoryHttpError> {
        let items = parse_write_body(body).map_err(MemoryHttpError::from_parse)?;
        if items.is_empty() {
            return Err(MemoryHttpError::bad_request("Memory 写入请求为空"));
        }
        let loaded = ConfigLoader::load_from_path(&self.config_path)
            .map_err(|error| MemoryHttpError::internal(format!("加载 Axon 配置失败：{error:?}")))?;
        // 先整批校验再落库：任何一条非法都不产生半批记录。
        let mut planned = Vec::with_capacity(items.len());
        for item in &items {
            planned.push(self.plan_write(item, &supersedes)?);
        }
        let mut reports = Vec::with_capacity(planned.len());
        let mut needs_repair = false;
        for (item, (kind, scope, targets)) in items.iter().zip(planned.iter()) {
            let now_ms = axon_time::unix_now_ms();
            let report = MemoryToolDispatcher::new(WRITE_TIMEOUT)
                .write_canonical(CanonicalMemoryWriteInput {
                    config_path: &self.config_path,
                    config: &loaded.config,
                    scope,
                    conversation_id: MANAGEMENT_CONVERSATION_ID,
                    turn_id: &format!("memory-{now_ms}"),
                    kind: kind.as_str(),
                    summary: &item.summary,
                    evidence: &item.evidence,
                    // 路由级与请求体级替代目标合并后原样下沉，替代关系必须真正落库。
                    supersedes_memory_ids: targets.clone(),
                    now_ms,
                })
                .map_err(|error| {
                    MemoryHttpError::bad_request(format!("Memory 写入失败：{error:?}"))
                })?;
            needs_repair |= report.needs_repair;
            reports.push(json!({
                "memoryKind": kind.as_str(),
                "memoryId": report.record.as_ref().map(|record| record.id.0.to_string()),
                "sqliteWritten": report.sqlite_written,
                "textIndexWritten": report.text_index_written,
                "vectorProjectionWritten": report.vector_projection_written,
                "needsRepair": report.needs_repair,
            }));
        }
        Ok(json!({
            "status": "memory_written",
            "records": reports,
            "needsRepair": needs_repair,
        }))
    }

    /// 校验单条写入并解析其目标 scope。
    fn plan_write(
        &self,
        item: &MemoryWriteItem,
        supersedes: &[MemoryId],
    ) -> Result<(MemoryKind, MemoryToolScope, Vec<MemoryId>), MemoryHttpError> {
        let kind = MemoryKind::from_code(item.memory_kind.trim())
            .ok_or_else(|| MemoryHttpError::bad_request("memoryKind 无效"))?;
        if item.summary.trim().is_empty() {
            return Err(MemoryHttpError::bad_request("summary 不能为空"));
        }
        if item.evidence.trim().is_empty() {
            return Err(MemoryHttpError::bad_request("evidence 不能为空"));
        }
        let mut targets = supersedes.to_vec();
        for raw in &item.supersedes_memory_ids {
            targets.push(parse_memory_id(raw)?);
        }
        let mut inherited_scope = None;
        for target in &targets {
            let record = self.store()?.get_record(*target).map_err(|error| {
                MemoryHttpError::internal(format!("读取被替代 Memory 失败：{error:?}"))
            })?;
            let record =
                record.ok_or_else(|| MemoryHttpError::not_found("被替代的 Memory 记录不存在"))?;
            self.ensure_visible(&record)?;
            if record.memory_kind != kind {
                return Err(MemoryHttpError::bad_request(
                    "替代记录的类型必须与被替代记录一致",
                ));
            }
            if inherited_scope.is_none() {
                inherited_scope = Some(self.scope_of_record(&record)?);
            }
        }
        // 修改继承被替代记录的作用域：仓库记忆的 namespace 与 repo 是派生哈希，客户端
        // 无法也不应重新提供，因此只有新建才要求文件夹目录。
        let scope = match inherited_scope {
            Some(scope) => scope,
            None => {
                if !is_personal_kind(kind)
                    && item
                        .workspace_path
                        .as_deref()
                        .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(MemoryHttpError::bad_request(
                        "仓库记忆必须提供文件夹目录（workspacePath）",
                    ));
                }
                scope_for_request(
                    &self.axon_root,
                    &self.config_path,
                    kind,
                    item.workspace_path.as_deref(),
                )
                .map_err(MemoryHttpError::from_scope)?
            }
        };
        Ok((kind, scope, targets))
    }

    /// 由既有记录派生其写入 scope。
    fn scope_of_record(
        &self,
        record: &MemorySqliteRecord,
    ) -> Result<MemoryToolScope, MemoryHttpError> {
        if record.user_ref.is_some() {
            return crate::memory_http_scope::personal_scope(&self.axon_root)
                .map_err(MemoryHttpError::from_scope);
        }
        let namespace_ref = record
            .namespace_ref
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| MemoryHttpError::internal("被替代 Memory 缺少作用域"))?;
        Ok(MemoryToolScope::Workspace {
            scope: axon_agent_runtime::MemoryWriteScope {
                namespace_ref,
                repo_ref: record.repo_ref.clone(),
                user_ref: None,
            },
        })
    }

    fn forget(&self, id: &str) -> Result<Value, MemoryHttpError> {
        let memory_id = parse_memory_id(id)?;
        let store = self.store()?;
        let mut record = store
            .get_record(memory_id)
            .map_err(|error| MemoryHttpError::internal(format!("读取 Memory 详情失败：{error:?}")))?
            .ok_or_else(|| MemoryHttpError::not_found("未找到 Memory 记录"))?;
        self.ensure_visible(&record)?;
        let vector_ids = record.vector_id.into_iter().collect::<Vec<_>>();
        let blocked_ref = record
            .record
            .content_ref
            .clone()
            .or_else(|| {
                record
                    .record
                    .tombstone
                    .as_ref()
                    .map(|item| item.blocked_ref.clone())
            })
            .ok_or_else(|| MemoryHttpError::bad_request("目标 Memory 缺少可遗忘引用"))?;
        record.record.state = MemoryState::Tombstone;
        record.record.content_ref = None;
        record.record.tombstone = Some(MemoryTombstone {
            blocked_ref,
            reason: "管理页显式遗忘".to_owned(),
        });
        record.updated_at_ms = axon_time::unix_now_ms();
        record.last_active_at_ms = record.updated_at_ms;
        record.archived_at_ms = Some(record.updated_at_ms);
        store
            .update_governance_records(&[record])
            .map_err(|error| {
                MemoryHttpError::internal(format!("写入遗忘 tombstone 失败：{error:?}"))
            })?;
        let needs_repair = !self.prune_projections(&[memory_id], &vector_ids);
        Ok(
            json!({ "status": "memory_forgotten", "memoryId": memory_id.0.to_string(), "needsRepair": needs_repair }),
        )
    }

    fn remove(&self, id: &str) -> Result<Value, MemoryHttpError> {
        let memory_id = parse_memory_id(id)?;
        let store = self.store()?;
        let record = store
            .get_record(memory_id)
            .map_err(|error| MemoryHttpError::internal(format!("读取 Memory 详情失败：{error:?}")))?
            .ok_or_else(|| MemoryHttpError::not_found("未找到 Memory 记录"))?;
        self.ensure_visible(&record)?;
        let vector_ids = record.vector_id.into_iter().collect::<Vec<_>>();
        let deleted = store
            .delete_records(&[memory_id])
            .map_err(|error| MemoryHttpError::internal(format!("删除 Memory 失败：{error:?}")))?;
        if deleted == 0 {
            return Err(MemoryHttpError::not_found("未找到 Memory 记录"));
        }
        let needs_repair = !self.prune_projections(&[memory_id], &vector_ids);
        Ok(
            json!({ "status": "memory_deleted", "memoryId": memory_id.0.to_string(), "needsRepair": needs_repair }),
        )
    }

    /// 清理文本索引、向量投影与 HNSW；任一失败即要求修复，绝不伪报成功。
    fn prune_projections(&self, memory_ids: &[MemoryId], vector_ids: &[i64]) -> bool {
        let Ok(orchestrator) = self.projection_runtime() else {
            return false;
        };
        let text_ok = orchestrator
            .text_index()
            .and_then(|index| index.delete_documents(memory_ids))
            .is_ok();
        let projection_ok = orchestrator
            .vector_projection_store()
            .and_then(|store| store.delete_projections_by_memory_ids(memory_ids))
            .is_ok();
        let vector_ok = if vector_ids.is_empty() {
            true
        } else {
            orchestrator
                .vector_index()
                .and_then(|index| {
                    index.ok_or_else(|| {
                        axon_domain::AxonError::memory_persistence("Memory HNSW 索引不可用")
                    })
                })
                .and_then(|index| index.remove_vectors(vector_ids))
                .is_ok()
        };
        text_ok && projection_ok && vector_ok
    }

    /// 是否存在需要异步修复的 Memory 投影。
    fn needs_repair(&self) -> Result<bool, MemoryHttpError> {
        let config = self.memory_config()?;
        let Ok(store) = MemoryPipelineStateStore::open(&config) else {
            return Ok(false);
        };
        for kind in [
            MemoryPipelineJobKind::ExplicitWrite,
            MemoryPipelineJobKind::Phase1,
            MemoryPipelineJobKind::Phase2,
        ] {
            if store
                .stats_for_kind(kind)
                .is_ok_and(|stats| stats.needs_repair > 0)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 校验记录属于当前可见面：Workspace 记录或本人 Personal 记录。
    fn ensure_visible(&self, record: &MemorySqliteRecord) -> Result<(), MemoryHttpError> {
        let owner = self.owner()?;
        if record
            .user_ref
            .as_deref()
            .is_none_or(|user_ref| user_ref == owner)
        {
            return Ok(());
        }
        Err(MemoryHttpError::not_found("Memory 不属于当前作用域"))
    }
}

impl GatewayHttpRouteHandler for MemoryHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let suffix = path.strip_prefix(MEMORY_PATH)?;
        let parts = suffix
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let result = match (method, parts.as_slice()) {
            ("GET", []) => self.list(query),
            ("GET", ["stats"]) => self.stats(query),
            ("GET", ["scopes"]) => self.scopes(),
            ("GET", [id]) => self.get(id),
            ("POST", []) => self.write(body, Vec::new()),
            ("POST", [id]) => match parse_memory_id(id) {
                Ok(memory_id) => self.write(body, vec![memory_id]),
                Err(error) => Err(error),
            },
            ("POST", [id, "forget"]) => self.forget(id),
            ("DELETE", [id]) => self.remove(id),
            _ => return None,
        };
        Some(match result {
            Ok(payload) => GatewayHttpResponse::json(200, &payload),
            Err(error) => GatewayHttpResponse::json(error.status, &json!({"error": error.message})),
        })
    }
}

/// 请求的 scope 选择，按 owner 是否参与过滤区分。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ScopeChoice {
    /// Workspace 记录加本人 Personal 记录。
    All,
    /// 仅本人 Personal 记录。
    Personal,
    /// 仅 Workspace 记录（全部 namespace）。
    Workspace,
    /// 单个 Workspace namespace。
    Namespace(String),
}

impl ScopeChoice {
    /// 生成绑定 owner 后的可见范围。
    fn visibility<'a>(&'a self, owner: &'a str) -> MemoryVisibility<'a> {
        match self {
            Self::All | Self::Namespace(_) => MemoryVisibility::OwnerScoped(owner),
            Self::Personal => MemoryVisibility::Personal(owner),
            Self::Workspace => MemoryVisibility::Workspace,
        }
    }

    /// 受限 namespace；`None` 表示不限制。
    fn namespace_ref(&self) -> Option<&str> {
        match self {
            Self::Namespace(namespace) => Some(namespace),
            Self::All | Self::Personal | Self::Workspace => None,
        }
    }

    /// 对外 scope 代码。
    fn code(&self) -> &'static str {
        match self {
            Self::All | Self::Namespace(_) => "all",
            Self::Personal => "personal",
            Self::Workspace => "workspace",
        }
    }
}

fn parse_memory_id(value: &str) -> Result<MemoryId, MemoryHttpError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .map(MemoryId)
        .ok_or_else(|| MemoryHttpError::bad_request("memoryId 无效"))
}

fn parse_cursor(query: Option<&str>) -> Result<Option<MemoryTimelineCursor>, MemoryHttpError> {
    let updated = query_value(query, "cursorUpdatedAtMs");
    let id = query_value(query, "cursorMemoryId");
    match (updated, id) {
        (None, None) => Ok(None),
        (Some(updated), Some(id)) => Ok(Some(MemoryTimelineCursor {
            updated_at_ms: updated
                .parse()
                .map_err(|_| MemoryHttpError::bad_request("cursorUpdatedAtMs 无效"))?,
            memory_id: id
                .parse()
                .map_err(|_| MemoryHttpError::bad_request("cursorMemoryId 无效"))?,
        })),
        _ => Err(MemoryHttpError::bad_request(
            "分页游标必须同时提供更新时间和 memoryId",
        )),
    }
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

pub(crate) struct MemoryHttpError {
    status: u16,
    message: String,
}

impl MemoryHttpError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: message.into(),
        }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            message: message.into(),
        }
    }
    fn from_scope(error: MemoryScopeError) -> Self {
        match error {
            MemoryScopeError::BadRequest(message) => Self::bad_request(message),
            MemoryScopeError::Internal(message) => Self::internal(message),
        }
    }
    fn from_parse(
        error: crate::memory_http_adapter::memory_http_adapter_row::WriteBodyError,
    ) -> Self {
        Self::bad_request(error.0)
    }
}

#[cfg(test)]
#[path = "memory_http_adapter_tests.rs"]
mod tests;
