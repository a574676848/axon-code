use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use axon_agent_runtime::{
    AGENT_TOOL_NAMESPACE, GLOB_TOOL_NAME, READ_FILE_TOOL_NAME, SEARCH_TOOL_NAME, ToolLedgerDecision,
};
use axon_crypto::sha256_hex;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{Map, Value, json};

use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolKind, ToolOrchestratorContext, ToolReceipt, ToolRuntimeObserver,
    execute_tool_call_with_observer,
};
use axon_domain::ToolCallInput;

const DEFAULT_CACHE_CAPACITY: usize = 128;
const CACHE_METADATA_KEY: &str = "readOnlyToolCache";
const CACHE_METADATA_HIT_FIELD: &str = "hit";
const CACHE_METADATA_REVISION_FIELD: &str = "workspaceRevision";
const ORIGINAL_METADATA_KEY: &str = "toolMetadata";
const COMPLETED_STATUS: &str = "completed";

static PROCESS_CACHE: LazyLock<Mutex<ReadOnlyToolCacheRuntime>> =
    LazyLock::new(|| Mutex::new(ReadOnlyToolCacheRuntime::new(DEFAULT_CACHE_CAPACITY)));

pub(super) fn execute_tool_call_with_cache(
    context: &ToolOrchestratorContext,
    request: &ToolExecutionRequest,
    runtime_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    let lookup = {
        let mut cache = process_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let lookup = cache.lookup(&context.cwd, request);
        if matches!(&lookup, CacheLookup::Disabled) {
            cache.schedule_workspace_watch(&context.cwd);
        }
        lookup
    };

    match lookup {
        CacheLookup::Hit(mut receipt) => {
            let scope_id = crate::turn_execution_ledger::scope_id(context);
            match crate::turn_execution_ledger::begin(
                &scope_id,
                request,
                workspace_revision(&context.cwd),
                &crate::turn_execution_ledger::turn_id(context),
            ) {
                ToolLedgerDecision::Reuse(receipt) => receipt,
                ToolLedgerDecision::RejectUnchanged(message) => {
                    failed_cached_tool_receipt(request, message)
                }
                ToolLedgerDecision::Execute => {
                    rewrite_receipt_identity(&mut receipt, request);
                    crate::turn_execution_ledger::settle(
                        &scope_id,
                        request,
                        receipt.clone(),
                        &crate::turn_execution_ledger::turn_id(context),
                    );
                    attach_cache_metadata(&mut receipt, true, workspace_revision(&context.cwd));
                    receipt
                }
                ToolLedgerDecision::InFlight => unreachable!("begin 已等待 in-flight 完成"),
            }
        }
        CacheLookup::Miss(pending) => {
            let mut receipt = execute_tool_call_with_observer(context, request, runtime_observer);
            if receipt.status == COMPLETED_STATUS {
                process_cache()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .store_if_current(&pending, &receipt);
            }
            attach_cache_metadata(&mut receipt, false, pending.snapshot.revision);
            receipt
        }
        CacheLookup::Disabled | CacheLookup::Bypassed => {
            let receipt = execute_tool_call_with_observer(context, request, runtime_observer);
            invalidate_after_write(&context.cwd, &receipt);
            receipt
        }
    }
}

fn failed_cached_tool_receipt(request: &ToolExecutionRequest, message: String) -> ToolReceipt {
    ToolReceipt {
        tool_call_id: request.tool_call_id.0,
        part_id: request.part_id.clone(),
        provider_call_id: request.tool_call.id.clone(),
        tool_name: request.tool_call.dispatch_name().to_owned(),
        status: "failed".to_owned(),
        summary: message,
        metadata: None,
        error_type: Some("repeated_operation".to_owned()),
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: None,
    }
}

fn process_cache() -> &'static Mutex<ReadOnlyToolCacheRuntime> {
    &PROCESS_CACHE
}

pub(crate) fn workspace_revision(cwd: &Path) -> u64 {
    let mut cache = process_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .workspace_snapshot(cwd)
        .map_or(0, |snapshot| snapshot.revision)
}

fn invalidate_after_write(cwd: &Path, receipt: &ToolReceipt) {
    if receipt.filesystem_write_performed {
        process_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate_workspace(cwd);
    }
}

struct ReadOnlyToolCacheRuntime {
    capacity: usize,
    entries: VecDeque<CacheEntry>,
    workspaces: HashMap<PathBuf, WorkspaceWatch>,
    initializing_workspaces: HashSet<PathBuf>,
}

impl ReadOnlyToolCacheRuntime {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::with_capacity(capacity.max(1)),
            workspaces: HashMap::new(),
            initializing_workspaces: HashSet::new(),
        }
    }

    fn lookup(&mut self, cwd: &Path, request: &ToolExecutionRequest) -> CacheLookup {
        let Some(tool_name) = cacheable_tool_name(request) else {
            return CacheLookup::Bypassed;
        };
        let Some(snapshot) = self.workspace_snapshot(cwd) else {
            return CacheLookup::Disabled;
        };
        let key = CacheKey::new(&snapshot, &tool_name, &request.tool_call.input);
        let Some(index) = self.entries.iter().position(|entry| entry.key == key) else {
            return CacheLookup::Miss(PendingCacheEntry { key, snapshot });
        };
        let entry = self.entries.remove(index).expect("缓存索引必须有效");
        let mut receipt = entry.receipt.clone();
        self.entries.push_back(entry);
        rewrite_receipt_identity(&mut receipt, request);
        attach_cache_metadata(&mut receipt, true, snapshot.revision);
        CacheLookup::Hit(receipt)
    }

    fn store_if_current(&mut self, pending: &PendingCacheEntry, receipt: &ToolReceipt) {
        if self.current_revision(&pending.snapshot.root) != Some(pending.snapshot.revision) {
            return;
        }
        self.entries.retain(|entry| entry.key != pending.key);
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(CacheEntry {
            key: pending.key.clone(),
            receipt: receipt.clone(),
        });
    }

    fn invalidate_workspace(&self, cwd: &Path) {
        let Some(root) = canonical_workspace(cwd) else {
            return;
        };
        if let Some(workspace) = self.workspaces.get(&root) {
            workspace.revision.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn workspace_snapshot(&mut self, cwd: &Path) -> Option<WorkspaceSnapshot> {
        let root = canonical_workspace(cwd)?;
        let revision = self.current_revision(&root)?;
        Some(WorkspaceSnapshot { root, revision })
    }

    fn schedule_workspace_watch(&mut self, cwd: &Path) {
        let Some(root) = canonical_workspace(cwd) else {
            return;
        };
        if self.workspaces.contains_key(&root) || !self.initializing_workspaces.insert(root.clone())
        {
            return;
        }
        thread::spawn(move || {
            let watch = WorkspaceWatch::start(&root);
            let mut cache = process_cache()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.initializing_workspaces.remove(&root);
            if let Some(watch) = watch {
                cache.workspaces.entry(root).or_insert(watch);
            }
        });
    }

    fn current_revision(&self, root: &Path) -> Option<u64> {
        self.workspaces
            .get(root)
            .map(|workspace| workspace.revision.load(Ordering::Acquire))
    }
}

struct WorkspaceWatch {
    revision: Arc<AtomicU64>,
    _watcher: RecommendedWatcher,
}

impl WorkspaceWatch {
    fn start(root: &Path) -> Option<Self> {
        let revision = Arc::new(AtomicU64::new(0));
        let event_revision = Arc::clone(&revision);
        let mut watcher = RecommendedWatcher::new(
            move |event: notify::Result<Event>| {
                if event.is_ok_and(|event| event_invalidates_cache(event.kind)) {
                    event_revision.fetch_add(1, Ordering::AcqRel);
                }
            },
            Config::default(),
        )
        .ok()?;
        watcher.watch(root, RecursiveMode::Recursive).ok()?;
        Some(Self {
            revision,
            _watcher: watcher,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct CacheKey {
    workspace: PathBuf,
    tool_name: String,
    arguments_hash: String,
    revision: u64,
}

impl CacheKey {
    fn new(snapshot: &WorkspaceSnapshot, tool_name: &str, input: &ToolCallInput) -> Self {
        Self {
            workspace: snapshot.root.clone(),
            tool_name: tool_name.to_owned(),
            arguments_hash: sha256_hex(normalized_input(input).as_bytes()),
            revision: snapshot.revision,
        }
    }
}

struct CacheEntry {
    key: CacheKey,
    receipt: ToolReceipt,
}

#[derive(Clone)]
struct WorkspaceSnapshot {
    root: PathBuf,
    revision: u64,
}

struct PendingCacheEntry {
    key: CacheKey,
    snapshot: WorkspaceSnapshot,
}

enum CacheLookup {
    Hit(ToolReceipt),
    Miss(PendingCacheEntry),
    Disabled,
    Bypassed,
}

fn cacheable_tool_name(request: &ToolExecutionRequest) -> Option<String> {
    match request.kind {
        ToolKind::Search => qualified_builtin_name(request, SEARCH_TOOL_NAME),
        ToolKind::ReadFile => qualified_builtin_name(request, READ_FILE_TOOL_NAME),
        ToolKind::Glob => qualified_builtin_name(request, GLOB_TOOL_NAME),
        ToolKind::MemorySearch
        | ToolKind::MemoryWrite
        | ToolKind::History
        | ToolKind::Notes
        | ToolKind::GetContextRemaining
        | ToolKind::NewContext
        | ToolKind::ListMcpResources
        | ToolKind::ReadMcpResource
        | ToolKind::UpdatePlan
        | ToolKind::WorkContract
        | ToolKind::GoalTool
        | ToolKind::Subagent
        | ToolKind::Mcp
        | ToolKind::InvokeSkill
        | ToolKind::Shell
        | ToolKind::Edit
        | ToolKind::AskUserQuestion
        // 作业回执是增量输出，随读取时刻变化；缓存会返回过期输出。
        | ToolKind::Job
        | ToolKind::Unsupported => None,
    }
}

fn qualified_builtin_name(request: &ToolExecutionRequest, bare_name: &str) -> Option<String> {
    let qualified_name = format!("{AGENT_TOOL_NAMESPACE}.{bare_name}");
    (request.tool_call.dispatch_name() == qualified_name).then_some(qualified_name)
}

fn canonical_workspace(cwd: &Path) -> Option<PathBuf> {
    fs::canonicalize(cwd).ok()
}

fn event_invalidates_cache(kind: EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

fn normalized_input(input: &ToolCallInput) -> String {
    match input {
        ToolCallInput::Json(value) => {
            let mut value = value.clone();
            sort_json_value(&mut value);
            serde_json::to_string(&value).unwrap_or_else(|_| "null".to_owned())
        }
        ToolCallInput::Text(value) => value.trim().to_owned(),
    }
}

fn sort_json_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut sorted = Map::new();
            let source = std::mem::take(object);
            let mut entries = source.into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            for (key, mut child) in entries {
                sort_json_value(&mut child);
                sorted.insert(key, child);
            }
            *object = sorted;
        }
        Value::Array(values) => values.iter_mut().for_each(sort_json_value),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn rewrite_receipt_identity(receipt: &mut ToolReceipt, request: &ToolExecutionRequest) {
    receipt.tool_call_id = request.tool_call_id.0;
    receipt.part_id.clone_from(&request.part_id);
    receipt.provider_call_id.clone_from(&request.tool_call.id);
    receipt.tool_name = request.tool_call.dispatch_name().to_owned();
    receipt.duration_ms = None;
}

fn attach_cache_metadata(receipt: &mut ToolReceipt, hit: bool, revision: u64) {
    let cache_metadata = json!({
        CACHE_METADATA_HIT_FIELD: hit,
        CACHE_METADATA_REVISION_FIELD: revision,
    });
    let mut metadata = receipt.metadata.take().unwrap_or_else(|| json!({}));
    if let Some(object) = metadata.as_object_mut() {
        object.insert(CACHE_METADATA_KEY.to_owned(), cache_metadata);
        receipt.metadata = Some(metadata);
        return;
    }
    receipt.metadata = Some(json!({
        ORIGINAL_METADATA_KEY: metadata,
        CACHE_METADATA_KEY: cache_metadata,
    }));
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use axon_agent_runtime::ProviderToolCall;
    use axon_domain::{ExecutionRequestId, ToolCallId, ToolExecutionPermission};

    use super::*;
    use crate::tool_orchestrator::ToolKind;

    #[test]
    fn repeated_read_reuses_full_receipt_and_rewrites_identity() {
        let workspace = axon_test_support::test_temp_dir();
        let mut runtime = ReadOnlyToolCacheRuntime::new(2);
        enable_workspace_cache(&mut runtime, workspace.path());
        let first = request(1, "part-1", &json!({"path":"a.rs","offset":1}));
        let pending = expect_miss(runtime.lookup(workspace.path(), &first));
        let receipt = receipt(&first, json!({"preview":"完整内容","lines":3}));
        runtime.store_if_current(&pending, &receipt);

        let second = request(2, "part-2", &json!({"offset":1,"path":"a.rs"}));
        let CacheLookup::Hit(cached) = runtime.lookup(workspace.path(), &second) else {
            panic!("规范参数相同的重复读取必须命中缓存");
        };

        assert_eq!(cached.provider_call_id, second.tool_call.id);
        assert_eq!(cached.tool_call_id, 2);
        assert_eq!(cached.part_id, "part-2");
        assert_eq!(cached.metadata.as_ref().unwrap()["preview"], "完整内容");
        assert_eq!(
            cached.metadata.as_ref().unwrap()[CACHE_METADATA_KEY][CACHE_METADATA_HIT_FIELD],
            true
        );
    }

    #[test]
    fn axon_write_revision_invalidates_cached_reads() {
        let workspace = axon_test_support::test_temp_dir();
        let mut runtime = ReadOnlyToolCacheRuntime::new(2);
        enable_workspace_cache(&mut runtime, workspace.path());
        let request = request(1, "part-1", &json!({"path":"a.rs"}));
        let pending = expect_miss(runtime.lookup(workspace.path(), &request));
        runtime.store_if_current(&pending, &receipt(&request, json!({"preview":"old"})));

        runtime.invalidate_workspace(workspace.path());

        assert!(matches!(
            runtime.lookup(workspace.path(), &request),
            CacheLookup::Miss(_)
        ));
    }

    #[test]
    fn external_file_modification_invalidates_cached_reads() {
        let workspace = axon_test_support::test_temp_dir();
        let file = workspace.path().join("a.rs");
        fs::write(&file, "old").unwrap();
        let mut runtime = ReadOnlyToolCacheRuntime::new(2);
        enable_workspace_cache(&mut runtime, workspace.path());
        let request = request(1, "part-1", &json!({"path":"a.rs"}));
        let pending = expect_miss(runtime.lookup(workspace.path(), &request));
        runtime.store_if_current(&pending, &receipt(&request, json!({"preview":"old"})));

        fs::write(file, "new").unwrap();

        for _ in 0..100 {
            if matches!(
                runtime.lookup(workspace.path(), &request),
                CacheLookup::Miss(_)
            ) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("外部文件修改必须推进工作区 revision");
    }

    #[test]
    fn uninitialized_workspace_bypasses_cache_before_background_watch_is_ready() {
        let workspace = axon_test_support::test_temp_dir();
        let mut runtime = ReadOnlyToolCacheRuntime::new(2);
        let request = request(1, "part-1", &json!({"path":"a.rs"}));

        assert!(matches!(
            runtime.lookup(workspace.path(), &request),
            CacheLookup::Disabled
        ));
        assert!(runtime.workspaces.is_empty());
    }

    fn enable_workspace_cache(runtime: &mut ReadOnlyToolCacheRuntime, cwd: &Path) {
        let root = canonical_workspace(cwd).expect("临时工作区必须可规范化");
        let watch = WorkspaceWatch::start(&root).expect("临时工作区必须可建立监听");
        runtime.workspaces.insert(root, watch);
    }

    fn request(id: u64, part_id: &str, arguments: &Value) -> ToolExecutionRequest {
        ToolExecutionRequest {
            tool_call: ProviderToolCall::new(
                format!("provider-{id}"),
                format!("{AGENT_TOOL_NAMESPACE}.{READ_FILE_TOOL_NAME}"),
                arguments.to_string(),
            ),
            tool_call_id: ToolCallId(id),
            execution_request_id: ExecutionRequestId(id),
            part_id: part_id.to_owned(),
            kind: ToolKind::ReadFile,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: ToolExecutionPermission::Allowed,
        }
    }

    fn receipt(request: &ToolExecutionRequest, metadata: Value) -> ToolReceipt {
        ToolReceipt {
            tool_call_id: request.tool_call_id.0,
            part_id: request.part_id.clone(),
            provider_call_id: request.tool_call.id.clone(),
            tool_name: READ_FILE_TOOL_NAME.to_owned(),
            status: COMPLETED_STATUS.to_owned(),
            summary: "读取完成".to_owned(),
            metadata: Some(metadata),
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: Some(30),
        }
    }

    fn expect_miss(lookup: CacheLookup) -> PendingCacheEntry {
        let CacheLookup::Miss(pending) = lookup else {
            panic!("首次读取必须未命中缓存");
        };
        pending
    }
}
