//! 记忆管理 Gateway 契约测试。
//!
//! 每个用例都在 `axon_test_support::test_temp_root()` 下的独立目录内建库，
//! 不触碰真实 `$HOME/.axon` 运行态。

use std::path::PathBuf;

use super::*;
use axon_config::AxonConfig;
use axon_domain::{
    CellId, MemoryCandidateId, MemoryRecord, MemorySourceOrigin, MemorySourceTrace, MemoryState,
    SessionId,
};
use axon_memory::MemorySqliteStore;

pub(super) fn test_adapter(test_name: &str) -> (MemoryHttpAdapter, PathBuf) {
    let root = axon_test_support::test_temp_root().join(format!("memory-http-{test_name}"));
    std::fs::create_dir_all(&root).expect("测试根目录应可创建");
    let config_path = root.join("config.toml");
    axon_config::ConfigLoader::save_to_path(&config_path, &AxonConfig::default())
        .expect("测试配置应可写入");
    (MemoryHttpAdapter::new(&root, &config_path), root)
}

pub(super) fn record(
    id: u64,
    kind: MemoryKind,
    user_ref: Option<&str>,
    namespace: &str,
) -> MemorySqliteRecord {
    MemorySqliteRecord {
        record: MemoryRecord {
            id: MemoryId(id),
            candidate_id: MemoryCandidateId(id),
            state: MemoryState::Strong,
            content_ref: Some(format!("memory-ref:{id}")),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::AgentKnowledge,
                session_id: Some(SessionId(1)),
                cell_id: Some(CellId(id)),
                summary: format!("测试记忆 {id}"),
                evidence_ref: Some(format!("evidence:{id}")),
            }),
            tombstone: None,
        },
        memory_kind: kind,
        namespace_ref: Some(namespace.to_owned()),
        repo_ref: None,
        user_ref: user_ref.map(str::to_owned),
        created_at_ms: 1,
        updated_at_ms: id,
        last_active_at_ms: id,
        ttl_expires_at_ms: None,
        confidence_basis_points: 9_000,
        importance_basis_points: 8_000,
        vector_id: None,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}

pub(super) fn seed(root: &std::path::Path, records: &[MemorySqliteRecord]) {
    let config = MemoryConfig::from_config_dir(
        root,
        false,
        axon_memory::MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
    )
    .expect("测试 Memory 配置应可构造");
    let store = MemorySqliteStore::open(&config).expect("测试 SQLite 应可打开");
    store.initialize_schema().expect("测试 schema 应可初始化");
    for item in records {
        store.upsert_record(item).expect("测试记录应可写入");
    }
}

fn response_json(response: GatewayHttpResponse) -> Value {
    serde_json::from_slice(&response.body).expect("Gateway 应返回 JSON")
}

pub(super) fn get(adapter: &MemoryHttpAdapter, path: &str, query: Option<&str>) -> (u16, Value) {
    let response = adapter
        .handle_request("GET", path, query, &[])
        .expect("GET 路由应命中");
    (response.status, response_json(response))
}

pub(super) fn post(adapter: &MemoryHttpAdapter, path: &str, body: &[u8]) -> (u16, Value) {
    let response = adapter
        .handle_request("POST", path, None, body)
        .expect("POST 路由应命中");
    (response.status, response_json(response))
}

#[test]
fn list_returns_kind_cursor_and_repair_contract() {
    let (adapter, root) = test_adapter("list");
    seed(
        &root,
        &[
            record(11, MemoryKind::Knowledge, None, "workspace:registered"),
            record(
                12,
                MemoryKind::Preference,
                Some("other-owner"),
                "personal:other",
            ),
        ],
    );
    let (status, payload) = get(&adapter, MEMORY_PATH, Some("memoryKind=knowledge&limit=1"));
    assert_eq!(status, 200);
    assert_eq!(payload["memoryKind"], "knowledge");
    assert_eq!(payload["records"][0]["memoryKind"], "knowledge");
    assert_eq!(payload["records"][0]["memoryId"], "11");
    assert_eq!(payload["records"][0]["scope"], "workspace");
    assert_eq!(payload["records"][0]["confidenceBasisPoints"], 9_000);
    assert!(
        payload["records"][0]["decayedScoreBasisPoints"]
            .as_u64()
            .is_some(),
        "时间轴必须下发衰减后权重：{payload}"
    );
    assert!(
        payload["records"][0].get("namespaceRef").is_none(),
        "内部命名空间不得下发到界面层：{payload}"
    );
    assert_eq!(payload["needsRepair"], false);
    assert_eq!(payload["nextCursor"]["memoryId"], "11");
}

#[test]
fn list_never_leaks_other_owners_personal_records() {
    let (adapter, root) = test_adapter("list-scope");
    seed(
        &root,
        &[
            record(
                21,
                MemoryKind::Preference,
                Some("other-owner"),
                "personal:other",
            ),
            record(22, MemoryKind::Knowledge, None, "workspace:any"),
        ],
    );
    let (status, payload) = get(&adapter, MEMORY_PATH, Some("memoryKind=preference"));
    assert_eq!(status, 200);
    assert_eq!(
        payload["records"].as_array().map(Vec::len),
        Some(0),
        "其他 owner 的 Personal 记录不得出现在当前可见面：{payload}"
    );

    let (status, payload) = get(
        &adapter,
        MEMORY_PATH,
        Some("scope=personal&memoryKind=knowledge"),
    );
    assert_eq!(status, 200);
    assert_eq!(payload["records"].as_array().map(Vec::len), Some(0));
}

#[test]
fn scopes_lists_every_real_namespace_in_sqlite() {
    let (adapter, root) = test_adapter("scopes");
    seed(
        &root,
        &[
            record(31, MemoryKind::Knowledge, None, "workspace:alpha"),
            record(32, MemoryKind::Experience, None, "workspace:beta"),
            record(33, MemoryKind::Knowledge, None, "workspace:alpha"),
        ],
    );
    let (status, payload) = get(&adapter, "/api/v1/memory/scopes", None);
    assert_eq!(status, 200);
    let scopes = payload["scopes"].as_array().expect("scopes 应为数组");
    let alpha = scopes
        .iter()
        .find(|item| item["namespaceRef"] == "workspace:alpha")
        .expect("workspace:alpha 必须出现在 scope 面中");
    assert_eq!(alpha["recordCount"], 2);
    assert_eq!(alpha["scopeKind"], "workspace");
    assert_eq!(
        scopes
            .iter()
            .filter(|item| item["scopeKind"] == "personal")
            .count(),
        0,
        "没有 Personal 记录时不得凭空列出 Personal scope"
    );
}

#[test]
fn stats_aggregates_the_visible_scope_in_one_pass() {
    let (adapter, root) = test_adapter("stats");
    seed(
        &root,
        &[
            record(41, MemoryKind::Knowledge, None, "workspace:alpha"),
            record(42, MemoryKind::Knowledge, None, "workspace:beta"),
            record(
                43,
                MemoryKind::Preference,
                Some("other-owner"),
                "personal:other",
            ),
        ],
    );
    let (status, payload) = get(&adapter, "/api/v1/memory/stats", None);
    assert_eq!(status, 200);
    assert_eq!(payload["totalRecords"], 2);
    assert_eq!(payload["memoryKindCounts"][0]["memoryKind"], "knowledge");
    assert_eq!(payload["memoryKindCounts"][0]["count"], 2);
    assert_eq!(
        payload["legacyRecordTables"].as_array().map(Vec::len),
        Some(0)
    );

    let (status, scoped) = get(
        &adapter,
        "/api/v1/memory/stats",
        Some("scope=workspace:alpha"),
    );
    assert_eq!(status, 200);
    assert_eq!(scoped["totalRecords"], 1);
}

#[test]
fn request_validation_is_fail_closed() {
    let (adapter, _root) = test_adapter("validation");
    let (status, _) = get(&adapter, MEMORY_PATH, Some("memoryKind=unknown"));
    assert_eq!(status, 400);

    let (status, _) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&cursorMemoryId=3"),
    );
    assert_eq!(status, 400, "半个游标必须拒绝");

    let (status, _) = post(
        &adapter,
        MEMORY_PATH,
        br#"{"memoryKind":"knowledge","summary":"x","evidence":"y","userRef":"spoofed"}"#,
    );
    assert_eq!(status, 400, "客户端不得提交任意 scope 字段");

    let (status, _) = post(
        &adapter,
        MEMORY_PATH,
        br#"{"memoryKind":"knowledge","summary":"x","evidence":"y"}"#,
    );
    assert_eq!(status, 400, "仓库记忆缺少文件夹目录必须拒绝");

    let (status, payload) = post(
        &adapter,
        MEMORY_PATH,
        br#"{"memoryKind":"preference","summary":"x","evidence":"y","workspacePath":"E:/zbg/axon-code"}"#,
    );
    assert_eq!(status, 400, "Personal 类型不得携带文件夹目录");
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|text| text.contains("Personal")),
        "{payload}"
    );

    let (status, _) = post(&adapter, "/api/v1/memory/not-an-id", b"{}");
    assert_eq!(status, 400);
}

#[test]
fn write_body_accepts_single_array_and_records_wrapper() {
    let single = parse_write_body(br#"{"memoryKind":"preference","summary":"s","evidence":"e"}"#)
        .expect("单条对象应可解析");
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].memory_kind, "preference");

    let array = parse_write_body(
        br#"[{"memoryKind":"knowledge","summary":"s","evidence":"e","workspacePath":"E:/w"},{"memoryKind":"experience","summary":"s2","evidence":"e2","workspacePath":"E:/w"}]"#,
    )
    .expect("裸数组应可解析");
    assert_eq!(array.len(), 2);
    assert_eq!(array[1].workspace_path.as_deref(), Some("E:/w"));

    let batch = parse_write_body(
        br#"{"records":[{"memoryKind":"procedure","summary":"s","evidence":"e","workspacePath":"E:/w","supersedesMemoryIds":[7]}]}"#,
    )
    .expect("records 包装体应可解析");
    assert_eq!(batch[0].supersedes_memory_ids, vec!["7".to_owned()]);

    assert!(
        parse_write_body(
            br#"{"records":[{"memoryKind":"procedure","summary":"s","evidence":"e","unknown":1}]}"#
        )
        .is_err(),
        "未知字段必须拒绝，避免静默忽略用户意图"
    );
    assert!(parse_write_body(br#""text""#).is_err());
}

#[test]
fn workspace_write_requires_accessible_absolute_folder() {
    let (adapter, root) = test_adapter("workspace-path");
    let (status, payload) = post(
        &adapter,
        MEMORY_PATH,
        br#"{"memoryKind":"knowledge","summary":"s","evidence":"e","workspacePath":"relative/dir"}"#,
    );
    assert_eq!(status, 400);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|text| text.contains("绝对路径")),
        "{payload}"
    );

    // 「绝对但不存在」的目录必须按平台构造：`E:/...` 在 unix 上不是绝对路径，
    // 会先命中「必须是绝对路径」分支，覆盖不到本用例真正要验证的「不可访问」。
    let missing = root.join("definitely-missing-axon-dir");
    let body = serde_json::json!({
        "memoryKind": "knowledge",
        "summary": "s",
        "evidence": "e",
        "workspacePath": missing.display().to_string(),
    })
    .to_string();
    let (status, payload) = post(&adapter, MEMORY_PATH, body.as_bytes());
    assert_eq!(status, 400);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|text| text.contains("不可访问")),
        "{payload}"
    );
}

#[test]
fn personal_write_round_trips_through_the_management_endpoint() {
    let (adapter, _root) = test_adapter("write-roundtrip");
    let body = serde_json::json!({
        "memoryKind": "preference",
        "summary": "偏好简体中文交付",
        "evidence": "evidence:pref",
    })
    .to_string();
    let (status, payload) = post(&adapter, MEMORY_PATH, body.as_bytes());
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["status"], "memory_written");
    assert_eq!(payload["records"][0]["memoryKind"], "preference");
    assert_eq!(payload["records"][0]["sqliteWritten"], true);
    assert!(
        payload["records"][0]["memoryId"]
            .as_str()
            .is_some_and(|id| id.parse::<u64>().is_ok())
    );

    let (status, listed) = get(&adapter, MEMORY_PATH, Some("memoryKind=preference"));
    assert_eq!(status, 200);
    let records = listed["records"].as_array().expect("列表应为数组");
    assert_eq!(
        records.len(),
        1,
        "刚写入的 Personal 记忆必须立刻可见：{listed}"
    );
    assert_eq!(records[0]["scope"], "personal");
}

#[test]
fn workspace_write_keeps_the_requested_kind_and_folder_scope() {
    let (adapter, root) = test_adapter("workspace-kind");
    let folder = root.join("repo");
    std::fs::create_dir_all(&folder).expect("测试仓库目录应可创建");
    let body = serde_json::json!({
        "memoryKind": "knowledge",
        "summary": "仓库知识必须原样保存类型",
        "evidence": "evidence:workspace-kind",
        "workspacePath": folder.to_string_lossy(),
    })
    .to_string();
    let (status, payload) = post(&adapter, MEMORY_PATH, body.as_bytes());
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["records"][0]["memoryKind"], "knowledge");

    let (status, listed) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&scope=workspace"),
    );
    assert_eq!(status, 200);
    let records = listed["records"].as_array().expect("列表应为数组");
    assert_eq!(records.len(), 1, "仓库知识必须按知识类型可见：{listed}");
    assert_eq!(records[0]["memoryKind"], "knowledge");
    assert_eq!(records[0]["scope"], "workspace");

    let (status, wrong_kind) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=experience&scope=workspace"),
    );
    assert_eq!(status, 200);
    assert_eq!(
        wrong_kind["records"].as_array().map(Vec::len),
        Some(0),
        "写入的知识记录不得以经验类型出现：{wrong_kind}"
    );
}

#[test]
fn superseding_workspace_record_inherits_its_scope_without_folder() {
    let (adapter, root) = test_adapter("workspace-supersede");
    let folder = root.join("repo");
    std::fs::create_dir_all(&folder).expect("测试仓库目录应可创建");
    let created = serde_json::json!({
        "memoryKind": "knowledge",
        "summary": "仓库知识原始摘要",
        "evidence": "evidence:supersede-original",
        "workspacePath": folder.to_string_lossy(),
    })
    .to_string();
    let (status, payload) = post(&adapter, MEMORY_PATH, created.as_bytes());
    assert_eq!(status, 200, "{payload}");
    let original_id = payload["records"][0]["memoryId"]
        .as_str()
        .expect("memoryId 必须是字符串")
        .parse::<u64>()
        .unwrap();

    // 修改不再要求客户端重复提供文件夹目录：作用域从被替代记录继承。
    let edited = serde_json::json!({
        "memoryKind": "knowledge",
        "summary": "仓库知识修改后摘要",
        "evidence": "evidence:supersede-edited",
    })
    .to_string();
    let (status, edited_payload) = post(
        &adapter,
        &format!("{MEMORY_PATH}/{original_id}"),
        edited.as_bytes(),
    );
    assert_eq!(status, 200, "{edited_payload}");
    assert_eq!(edited_payload["records"][0]["memoryKind"], "knowledge");

    let config = MemoryConfig::from_config_dir(
        &root,
        false,
        axon_memory::MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
    )
    .unwrap();
    let store = MemorySqliteStore::open(&config).expect("测试 SQLite 应可打开");
    let original = store
        .get_record(MemoryId(original_id))
        .expect("读取被替代记录")
        .expect("被替代记录应存在");
    let new_id = edited_payload["records"][0]["memoryId"]
        .as_str()
        .expect("memoryId 必须是字符串")
        .parse::<u64>()
        .map(MemoryId)
        .unwrap();
    let created_record = store
        .get_record(new_id)
        .expect("读取新记录")
        .expect("新记录应存在");
    assert_eq!(
        created_record.namespace_ref, original.namespace_ref,
        "修改必须继承被替代记录的 namespace"
    );
    assert_eq!(created_record.repo_ref, original.repo_ref);
    assert_ne!(created_record.record.id, original.record.id);

    // 替代关系必须真正落库：请求体里的 supersedesMemoryIds 不能只用于校验。
    let relations = store
        .supersessions_by_successor(new_id)
        .expect("必须写入替代关系");
    assert_eq!(
        relations
            .iter()
            .map(|relation| relation.predecessor_memory_id)
            .collect::<Vec<_>>(),
        vec![MemoryId(original_id)]
    );
}

#[test]
fn detail_rejects_record_from_another_owner() {
    let (adapter, root) = test_adapter("detail-scope");
    seed(
        &root,
        &[record(
            51,
            MemoryKind::Preference,
            Some("other-owner"),
            "personal:other",
        )],
    );
    let (status, _) = get(&adapter, "/api/v1/memory/51", None);
    assert_eq!(status, 404);
}

/// 显式性能验收基准，不纳入常规门禁。
///
/// 合同 §6 的性能目标是对交付端点而非 store 层提出的：列表与统计每次请求都
/// 必须只触碰 SQLite，不得重建 Tantivy/HNSW 运行时。这里同时给出列表 p95 与
/// 单次完整运行时构建耗时，两者的量级差就是本修复消除的每请求开销。
#[test]
#[ignore = "显式性能验收基准，不纳入常规门禁"]
fn memory_list_p95_stays_within_gateway_budget() {
    use std::time::{Duration, Instant};

    const SAMPLES: usize = 100;
    let (adapter, root) = test_adapter("perf-list");
    let mut seeded = Vec::new();
    for id in 1..=200u64 {
        seeded.push(record(id, MemoryKind::Knowledge, None, "workspace:perf"));
    }
    seed(&root, &seeded);

    // 预热：首次请求完成 schema 初始化。
    let (status, _) = get(&adapter, MEMORY_PATH, Some("memoryKind=knowledge&limit=50"));
    assert_eq!(status, 200);

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let (status, payload) = get(&adapter, MEMORY_PATH, Some("memoryKind=knowledge&limit=50"));
        assert_eq!(status, 200);
        assert_eq!(payload["records"].as_array().map(Vec::len), Some(50));
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p95 = samples[(SAMPLES * 95 / 100).saturating_sub(1)];

    let config = MemoryConfig::from_config_dir(
        &root,
        false,
        axon_memory::MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
    )
    .unwrap();
    let init_started = Instant::now();
    let orchestrator = MemoryOrchestrator::init(config).expect("完整运行时应可构建");
    let init_cost = init_started.elapsed();
    drop(orchestrator);

    eprintln!(
        "memory_list samples={SAMPLES} p95_ms={:.3} full_runtime_init_ms={:.3}",
        p95.as_secs_f64() * 1_000.0,
        init_cost.as_secs_f64() * 1_000.0
    );
    assert!(
        p95 < Duration::from_millis(50),
        "管理页列表 p95 超过 50ms：{p95:?}"
    );
}
