//! v1 到 v2 迁移、损坏护栏与平台默认目录的逐条断言。
//!
//! 全部在临时目录上跑，走 `*_at` 这类显式入参入口，不依赖本机真实主目录。

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    ClientStateDocument, V1_BACKUP_NAME, V1_SUPERSEDED_NAME, V2_FILE_NAME,
    is_inside_backend_root_at, load_document_at, platform_default_client_root,
    profile_draft_history_path, save_document_at,
};
use crate::connection::profile::{CredentialSource, EndpointSpec};

/// v1 样例：字段名是旧版的 snake_case。
const V1_SAMPLE: &str = r#"{
  "version": 1,
  "workspaces": [{"path": "D:/work/alpha", "name": "alpha"}],
  "lane_workspace": {"lane-1": "D:/work/alpha"},
  "lane_pinned": ["lane-1"],
  "lane_archived": ["lane-2"],
  "lane_title": {"lane-1": "重构计划"},
  "lane_order": {"D:/work/alpha": ["lane-1", "lane-2"]},
  "pending_lanes": [{"lane_id": "lane-9", "workspace_id": "D:/work/alpha", "destination": "s1", "created_at": 7}],
  "last_workspace_dir": "D:/work",
  "notifications_enabled": false,
  "theme": "obsidian"
}"#;

/// 把分隔符统一成正斜杠，使跨平台的路径断言只比较结构。
fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// 造一个只属于本次用例的临时目录。
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "axon-store-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
    ));
    fs::create_dir_all(&dir).expect("临时目录应可创建");
    dir
}

fn write_v1(dir: &Path, body: &str) {
    fs::create_dir_all(dir).expect("目录应可创建");
    fs::write(dir.join(super::V1_FILE_NAME), body).expect("v1 应可写入");
}

#[test]
fn v1_in_client_root_is_migrated_field_by_field() {
    let root = temp_dir("client");
    write_v1(&root, V1_SAMPLE);

    let loaded = load_document_at(&root, None);
    assert!(!loaded.read_only);
    assert!(loaded.migrated_from_v1, "应记录发生了迁移");

    let document = &loaded.document;
    assert_eq!(document.version, 2);
    assert_eq!(document.client.theme.as_deref(), Some("obsidian"));
    assert!(!document.client.notifications_enabled);
    assert_eq!(
        document.client.last_workspace_dir.as_deref(),
        Some("D:/work")
    );
    assert_eq!(document.client.last_profile_id.as_deref(), Some("local"));
    assert_eq!(
        document.client.previous_profile_id.as_deref(),
        Some("local")
    );

    let projection = document.projection("local");
    assert_eq!(projection.workspaces.len(), 1);
    assert_eq!(projection.workspaces[0].path, "D:/work/alpha");
    assert_eq!(projection.lane_pinned, vec!["lane-1".to_owned()]);
    assert_eq!(projection.lane_archived, vec!["lane-2".to_owned()]);
    assert_eq!(
        projection.lane_title.get("lane-1").map(String::as_str),
        Some("重构计划")
    );
    assert_eq!(projection.pending_lanes.len(), 1);
    assert_eq!(projection.pending_lanes[0].lane_id, "lane-9");

    let meta = document.meta("local").expect("应建立本机档案元数据");
    assert_eq!(meta.label, "本机");
    assert_eq!(meta.spec, EndpointSpec::Local);
    assert_eq!(meta.credential, CredentialSource::LocalEnvFile);

    assert!(root.join(V2_FILE_NAME).exists(), "v2 必须已写盘");
    assert!(root.join(V1_BACKUP_NAME).exists(), "v1 必须已备份");
    assert!(
        root.join(V1_SUPERSEDED_NAME).exists(),
        "客户端根的 v1 必须归档为固定名"
    );
    assert!(
        !root.join(super::V1_FILE_NAME).exists(),
        "归档后不应再留下 v1"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn v1_in_backend_root_is_retired_in_place_without_touching_client_marker() {
    let root = temp_dir("client-backend");
    let backend = temp_dir("backend");
    write_v1(&backend, V1_SAMPLE);

    let loaded = load_document_at(&root, Some(&backend));
    assert!(loaded.migrated_from_v1);
    assert_eq!(loaded.document.projection("local").workspaces.len(), 1);

    // 固定名只在客户端根生成一份；后端根就地改名。
    assert!(root.join(V1_BACKUP_NAME).exists());
    assert!(!root.join(V1_SUPERSEDED_NAME).exists());
    assert!(
        backend
            .join(format!("{}{}", super::V1_FILE_NAME, ".superseded"))
            .exists()
    );
    assert!(!backend.join(super::V1_FILE_NAME).exists());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&backend);
}

#[test]
fn client_root_v1_wins_when_both_places_have_one() {
    let root = temp_dir("both-client");
    let backend = temp_dir("both-backend");
    write_v1(&root, V1_SAMPLE);
    let mut other = V1_SAMPLE.replace("D:/work/alpha", "D:/work/beta");
    other = other.replace("\"theme\": \"obsidian\"", "\"theme\": \"paper\"");
    write_v1(&backend, &other);

    let loaded = load_document_at(&root, Some(&backend));
    let projection = loaded.document.projection("local");
    assert_eq!(
        projection.workspaces[0].path, "D:/work/alpha",
        "客户端根那份优先"
    );
    assert_eq!(loaded.document.client.theme.as_deref(), Some("obsidian"));

    // 后端根那份仍按「不覆盖证据」归档，且加序号同名。
    let backend_superseded = backend.join(format!("{}{}", super::V1_FILE_NAME, ".superseded"));
    assert!(backend_superseded.exists());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&backend);
}

#[test]
fn existing_superseded_markers_are_never_overwritten() {
    let root = temp_dir("marker");
    let backend = temp_dir("marker-backend");
    write_v1(&root, V1_SAMPLE);
    write_v1(&backend, V1_SAMPLE);
    fs::write(root.join(V1_SUPERSEDED_NAME), "第一条证据").expect("既有标记应可写入");
    fs::write(
        backend.join(format!("{}{}", super::V1_FILE_NAME, ".superseded")),
        "第一份后端证据",
    )
    .expect("既有标记应可写入");

    let _ = load_document_at(&root, Some(&backend));

    let kept = fs::read_to_string(root.join(V1_SUPERSEDED_NAME)).expect("标记应仍可读");
    assert_eq!(kept, "第一条证据", "固定名不得被覆盖");
    let kept =
        fs::read_to_string(backend.join(format!("{}{}", super::V1_FILE_NAME, ".superseded")))
            .expect("标记应仍可读");
    assert_eq!(kept, "第一份后端证据", "后端同序号也不得被覆盖");
    assert!(
        backend
            .join(format!("{}{}.2", super::V1_FILE_NAME, ".superseded"))
            .exists(),
        "后端根应追加序号而不是覆盖"
    );

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&backend);
}

#[test]
fn corrupt_v2_is_backed_up_and_falls_back_to_defaults() {
    let root = temp_dir("corrupt");
    fs::write(root.join(V2_FILE_NAME), "{ 这不是 json ").expect("损坏文件应可写入");

    let loaded = load_document_at(&root, None);
    assert!(!loaded.read_only);
    assert_eq!(loaded.document.version, 2);
    let backup = loaded.corrupt_backup.expect("必须留下损坏副本");
    assert!(backup.to_string_lossy().ends_with(".corrupt"));
    assert!(backup.exists());
    assert!(root.join(V2_FILE_NAME).exists(), "损坏文件本身保持原样");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn future_versions_are_read_only_and_never_rewritten() {
    let root = temp_dir("future");
    let body = r#"{"version": 7, "client": {"theme": "obsidian"}, "profiles": {}}"#;
    fs::write(root.join(V2_FILE_NAME), body).expect("未来版本应可写入");

    let loaded = load_document_at(&root, None);
    assert!(loaded.read_only, "更高结构版本必须只读");
    assert_eq!(loaded.document.version, 7);
    assert_eq!(loaded.document.client.theme.as_deref(), Some("obsidian"));

    let error = save_document_at(&root, &loaded.document).expect_err("只读时写入必须被拒绝");
    assert!(error.contains("只读"), "{error}");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn v1_without_notification_field_defaults_to_enabled() {
    let root = temp_dir("notify");
    write_v1(
        &root,
        r#"{"version": 1, "workspaces": [], "theme": "paper"}"#,
    );
    let loaded = load_document_at(&root, None);
    assert!(
        loaded.document.client.notifications_enabled,
        "缺省必须视为开启"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn zero_profiles_self_heal_a_local_profile_only_when_a_config_exists() {
    let mut document = ClientStateDocument::default();
    assert_eq!(document.ensure_local_profile(false), None);
    assert!(document.profiles.is_empty());

    let healed = document
        .ensure_local_profile(true)
        .expect("应自愈出本机档案");
    assert_eq!(healed, "local");
    assert_eq!(document.client.last_profile_id.as_deref(), Some("local"));
    assert_eq!(
        document.client.previous_profile_id.as_deref(),
        Some("local")
    );

    // 再次自愈不会重复插入，也不会改写已有结论。
    assert_eq!(
        document.ensure_local_profile(true),
        Some("local".to_owned())
    );
    assert_eq!(document.profiles.len(), 1);
}

#[test]
fn cloud_profiles_are_deduplicated_by_address() {
    let mut document = ClientStateDocument::default();
    let first = document.upsert_cloud_profile(
        "公司云",
        "https://axon.example.com",
        "wss://axon.example.com",
        CredentialSource::SessionOnly,
    );
    let second = document.upsert_cloud_profile(
        "公司云改名",
        "https://axon.example.com",
        "wss://axon.example.com",
        CredentialSource::SessionOnly,
    );
    assert_eq!(first, second, "同址档案应复用 id");
    assert_eq!(document.profiles.len(), 1);
    assert_eq!(
        document.meta(&first).map(|meta| meta.label),
        Some("公司云改名".to_owned())
    );
}

#[test]
fn platform_default_client_root_sits_beside_the_backend_home() {
    /*
     * 三平台同一条规则：客户端状态落在 `~/.axon-desktop`，与后端运行时根
     * `~/.axon` 平级、互不包含。用户与支持人员只有一个位置要找。
     */
    for (home, profile) in [
        (Some(OsStr::new("C:/Users/tester")), Some(OsStr::new("C:/Users/tester"))),
        (Some(OsStr::new("/Users/tester")), None),
        (Some(OsStr::new("/home/tester")), None),
    ] {
        let root = platform_default_client_root(home, profile);
        assert!(
            slash_path(&root).ends_with(".axon-desktop"),
            "客户端根必须落在 ~/.axon-desktop：{}",
            root.display()
        );
        assert!(
            !slash_path(&root).contains("/.axon/"),
            "客户端根不得落在后端目录 ~/.axon 之下：{}",
            root.display()
        );
    }
    // 没有 HOME 时退回 USERPROFILE，仍然落回同一个位置。
    let from_profile = platform_default_client_root(None, Some(OsStr::new("C:/Users/tester")));
    assert!(slash_path(&from_profile).ends_with(".axon-desktop"));
}

#[test]
fn client_root_override_must_stay_out_of_the_backend_home() {
    let axon_home = Path::new("C:/Users/tester/.axon");
    // 客户端自己的位置不与后端根相交。
    assert!(!is_inside_backend_root_at(
        Path::new("C:/Users/tester/.axon-desktop"),
        axon_home
    ));
    // 后端自己写的几层一律拒绝，避免与 daemon 抢同一批文件。
    for owned in ["data", "sessions", "identity", "skills", "bin", "client"] {
        assert!(
            is_inside_backend_root_at(&axon_home.join(owned).join("state.json"), axon_home),
            "客户端根不得落进后端的 {owned} 目录"
        );
    }
    // 后端根本身也拒绝。
    assert!(is_inside_backend_root_at(axon_home, axon_home));
    // 完全在别处的路径不受影响。
    assert!(!is_inside_backend_root_at(
        Path::new("D:/axon-client"),
        axon_home
    ));
}

#[test]
fn draft_history_paths_are_sanitized_per_profile() {
    let path = profile_draft_history_path("3f2b/../危险");
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    assert!(name.ends_with(".jsonl"), "{name}");
    assert!(!name.contains('/'), "{name}");
    assert!(!name.contains(".."), "{name}");
}
