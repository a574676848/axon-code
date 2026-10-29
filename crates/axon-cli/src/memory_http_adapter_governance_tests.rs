//! 记忆治理写路径（归档、取消归档、作用域往返与主目录桶）的 Gateway 契约测试。
//!
//! 与 `memory_http_adapter_tests` 分开：读契约与治理写契约各自成文件，避免单文件
//! 越过 700 行硬限。

use super::tests::{get, post, record, seed, test_adapter};
use super::*;
use axon_domain::MemoryState;

#[test]
fn archive_window_filters_records_and_kind_counts_follow_it() {
    let (adapter, root) = test_adapter("archive-window");
    let mut archived = record(61, MemoryKind::Knowledge, None, "workspace:alpha");
    archived.archived_at_ms = Some(9);
    let mut forgotten = record(62, MemoryKind::Knowledge, None, "workspace:alpha");
    forgotten.record.state = MemoryState::Tombstone;
    forgotten.record.content_ref = None;
    forgotten.archived_at_ms = Some(9);
    seed(
        &root,
        &[
            record(60, MemoryKind::Knowledge, None, "workspace:alpha"),
            archived,
            forgotten,
        ],
    );

    let (status, active) = get(&adapter, MEMORY_PATH, Some("memoryKind=knowledge"));
    assert_eq!(status, 200);
    assert_eq!(active["archiveStatus"], "active");
    assert_eq!(
        active["records"].as_array().map(Vec::len),
        Some(1),
        "默认窗口只列活跃记录"
    );

    let (status, archived_page) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&status=archived"),
    );
    assert_eq!(status, 200);
    let ids = archived_page["records"]
        .as_array()
        .expect("records 应为数组")
        .iter()
        .map(|item| item["memoryId"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["62", "61"],
        "已归档窗口按更新时间倒序给出全部归档记录"
    );

    let (status, all) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&status=all"),
    );
    assert_eq!(status, 200);
    assert_eq!(all["records"].as_array().map(Vec::len), Some(3));

    let (status, stats) = get(&adapter, "/api/v1/memory/stats", Some("status=archived"));
    assert_eq!(status, 200);
    assert_eq!(stats["archiveStatus"], "archived");
    assert_eq!(
        stats["memoryKindCounts"][0]["count"], 2,
        "页签计数必须与已归档窗口同口径"
    );

    let (status, _) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&status=unknown"),
    );
    assert_eq!(status, 400, "未知归档状态必须 fail-closed");
}

#[test]
fn archive_and_unarchive_round_trip_between_windows() {
    let (adapter, root) = test_adapter("archive-round-trip");
    seed(
        &root,
        &[record(71, MemoryKind::Knowledge, None, "workspace:alpha")],
    );

    let (status, receipt) = post(&adapter, "/api/v1/memory/71/archive", &[]);
    assert_eq!(status, 200);
    assert_eq!(receipt["status"], "memory_archived");
    let (status, archived) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&status=archived"),
    );
    assert_eq!(status, 200);
    assert_eq!(archived["records"].as_array().map(Vec::len), Some(1));
    let archived_at = archived["records"][0]["archivedAtMs"].as_u64();
    assert!(archived_at.is_some(), "归档记录必须带归档时间供界面识别");
    let (status, active) = get(&adapter, MEMORY_PATH, Some("memoryKind=knowledge"));
    assert_eq!(status, 200);
    assert_eq!(active["records"].as_array().map(Vec::len), Some(0));

    let (status, receipt) = post(&adapter, "/api/v1/memory/71/unarchive", &[]);
    assert_eq!(status, 200);
    assert_eq!(receipt["status"], "memory_unarchived");
    let (status, active) = get(&adapter, MEMORY_PATH, Some("memoryKind=knowledge"));
    assert_eq!(status, 200);
    assert_eq!(active["records"].as_array().map(Vec::len), Some(1));

    let (status, payload) = post(&adapter, "/api/v1/memory/71/unarchive", &[]);
    assert_eq!(status, 400);
    assert_eq!(payload["error"], "这条记忆尚未归档");
}

#[test]
fn tombstone_cannot_be_archived_or_unarchived() {
    let (adapter, root) = test_adapter("archive-tombstone");
    let mut forgotten = record(81, MemoryKind::Knowledge, None, "workspace:alpha");
    forgotten.record.state = MemoryState::Tombstone;
    forgotten.record.content_ref = None;
    forgotten.archived_at_ms = Some(9);
    seed(&root, &[forgotten]);

    let (status, payload) = post(&adapter, "/api/v1/memory/81/unarchive", &[]);
    assert_eq!(status, 400);
    assert!(
        payload["error"]
            .as_str()
            .unwrap_or_default()
            .contains("已遗忘"),
        "已遗忘记录不得被静默恢复：{payload}"
    );
}

#[test]
fn workspace_scope_value_round_trips_to_the_same_namespace() {
    let (adapter, root) = test_adapter("workspace-scope");
    seed(
        &root,
        &[
            record(91, MemoryKind::Knowledge, None, "workspace:alpha"),
            record(92, MemoryKind::Knowledge, None, "workspace:beta"),
        ],
    );

    let (status, payload) = get(&adapter, "/api/v1/memory/scopes", None);
    assert_eq!(status, 200);
    let alpha = payload["scopes"]
        .as_array()
        .expect("scopes 应为数组")
        .iter()
        .find(|item| item["namespaceRef"] == "workspace:alpha")
        .expect("workspace:alpha 必须出现在 scope 面中");
    let scope_value = alpha["scope"].as_str().expect("scope 必须是字符串");
    assert_eq!(
        scope_value, "workspace:alpha",
        "scope 值不得重复拼接 workspace: 前缀"
    );

    let (status, page) = get(
        &adapter,
        MEMORY_PATH,
        Some(&format!("memoryKind=knowledge&scope={scope_value}")),
    );
    assert_eq!(status, 200);
    let ids = page["records"]
        .as_array()
        .expect("records 应为数组")
        .iter()
        .map(|item| item["memoryId"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["91"], "仓库筛选必须命中该 namespace 下的记录");

    // 旧客户端曾把双层前缀原样回传，适配层需要收敛而不是静默返回空页。
    let (status, page) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&scope=workspace:workspace:alpha"),
    );
    assert_eq!(status, 200);
    assert_eq!(page["records"].as_array().map(Vec::len), Some(1));
}

#[test]
fn home_folder_writes_land_in_the_axon_home_bucket() {
    let (adapter, root) = test_adapter("home-bucket");
    let folder = root.join("data");
    std::fs::create_dir_all(&folder).expect("测试主目录应可创建");
    let body = serde_json::json!({
        "memoryKind": "knowledge",
        "summary": "在 Axon 主目录里沉淀的知识",
        "evidence": "evidence:home-bucket",
        "workspacePath": folder.to_string_lossy(),
    })
    .to_string();
    let (status, payload) = post(&adapter, MEMORY_PATH, body.as_bytes());
    assert_eq!(status, 200, "{payload}");

    let (status, scopes) = get(&adapter, "/api/v1/memory/scopes", None);
    assert_eq!(status, 200);
    let home = scopes["scopes"]
        .as_array()
        .expect("scopes 应为数组")
        .iter()
        .find(|item| item["namespaceRef"] == "workspace:axon-home")
        .expect("主目录写入必须落在固定的 Axon 主目录桶");
    assert_eq!(home["recordCount"], 1);
    assert!(
        home["label"]
            .as_str()
            .unwrap_or_default()
            .contains("Axon 主目录"),
        "管理页必须把主目录桶读成可读标签：{home}"
    );

    let (status, listed) = get(
        &adapter,
        MEMORY_PATH,
        Some("memoryKind=knowledge&scope=workspace:axon-home"),
    );
    assert_eq!(status, 200);
    assert_eq!(
        listed["records"].as_array().map(Vec::len),
        Some(1),
        "主目录桶必须能按 scope 精确筛出：{listed}"
    );
}
