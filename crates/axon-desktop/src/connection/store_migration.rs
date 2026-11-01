//! v1 状态文件的读取、归档与迁移。
//!
//! 这一层只处理旧结构：文件名、双位置优先级、备份与归档规则、未知字段警告。
//! 新结构的读写留在 [`super::store`]，两边通过 `pub(super)` 的入口互相调用，
//! 迁移规则因此可以在临时目录上逐条复现。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::store::{
    ClientStateDocument, LOCAL_PROFILE_ID, LOCAL_PROFILE_LABEL, LoadedDocument, ONBOARDING_VERSION,
    ProfileProjection, ProfileSection, V1_BACKUP_NAME, V1_FILE_NAME, V1_SUPERSEDED_NAME,
    backup_corrupt, save_document_at, unique_suffixed_path,
};
use crate::connection::profile::{BackendProfile, EndpointSpec};
use crate::state::{PendingLane, RegisteredWorkspace};

/// v1 状态文件的两处位置，按读取优先级排列：客户端根优先，缺失才读后端运行时根。
fn v1_candidates(root: &Path, backend: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = vec![root.join(V1_FILE_NAME)];
    if let Some(backend) = backend {
        let backend_file = backend.join(V1_FILE_NAME);
        if backend_file != candidates[0] {
            candidates.push(backend_file);
        }
    }
    candidates
}

/// 从 v1 迁移；两处都没有 v1 时产出默认文档。
pub(super) fn migrate_from_v1(root: &Path, backend: Option<&Path>) -> LoadedDocument {
    let mut found: Option<(PathBuf, V1State)> = None;
    for candidate in v1_candidates(root, backend) {
        match read_v1(&candidate) {
            Ok(Some(state)) => {
                found = Some((candidate, state));
                break;
            }
            Ok(None) => {}
            Err(reason) => {
                eprintln!(
                    "WARNING: v1 桌面状态解析失败，已另存损坏副本后跳过：{}（{reason}）",
                    candidate.display()
                );
                backup_corrupt(&candidate);
            }
        }
    }

    let Some((source, legacy)) = found else {
        return LoadedDocument {
            document: ClientStateDocument::default(),
            read_only: false,
            migrated_from_v1: false,
            corrupt_backup: None,
        };
    };

    let document = legacy.into_document();
    // 回滚护栏第一步：v2 首次写入前把 v1 备份到客户端根，只备份一份。
    let backup = root.join(V1_BACKUP_NAME);
    if !backup.exists() {
        let _ = fs::create_dir_all(root);
        if let Err(error) = fs::copy(&source, &backup) {
            eprintln!("WARNING: v1 状态备份失败：{error}");
        }
    }
    let mut loaded = LoadedDocument {
        document,
        read_only: false,
        migrated_from_v1: true,
        corrupt_backup: None,
    };
    if save_document_at(root, &loaded.document).is_ok() {
        retire_v1_files(root, backend);
    } else {
        eprintln!("WARNING: v2 状态首次写入失败，v1 文件保持原样");
        loaded.migrated_from_v1 = false;
    }
    loaded
}

/// v2 首次写成功后把 v1 归档：客户端根用固定名且只生成一次，后端根就地改名并追加序号。
fn retire_v1_files(root: &Path, backend: Option<&Path>) {
    let client_file = root.join(V1_FILE_NAME);
    if client_file.exists() {
        let marker = root.join(V1_SUPERSEDED_NAME);
        if !marker.exists()
            && let Err(error) = fs::rename(&client_file, &marker)
        {
            eprintln!("WARNING: v1 状态归档失败：{error}");
        }
    }
    let Some(backend) = backend else {
        return;
    };
    let backend_file = backend.join(V1_FILE_NAME);
    if backend_file.exists() && backend_file != client_file {
        let target = unique_suffixed_path(&backend_file, ".superseded");
        if let Err(error) = fs::rename(&backend_file, &target) {
            eprintln!("WARNING: 后端根 v1 状态归档失败：{error}");
        }
    }
}

/// v1 状态文件里我们认识的键；其余键在迁移时丢弃，但会记一行警告。
const V1_KNOWN_KEYS: [&str; 11] = [
    "version",
    "workspaces",
    "lane_workspace",
    "lane_pinned",
    "lane_archived",
    "lane_title",
    "lane_order",
    "pending_lanes",
    "last_workspace_dir",
    "notifications_enabled",
    "theme",
];

/// 迁移时把 v1 里的未知字段记一行警告，而不是静默丢弃。
fn warn_unknown_v1_keys(path: &Path, text: &str) {
    let Ok(serde_json::Value::Object(map)) =
        serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        // 解析失败由调用方按「损坏」处理，这里只负责未知键。
        return;
    };
    let unknown = map
        .keys()
        .filter(|key| !V1_KNOWN_KEYS.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        eprintln!(
            "WARNING: v1 桌面状态含未识别字段，迁移时丢弃：{}（{}）",
            unknown.join(", "),
            path.display()
        );
    }
}

/// v2 已生效时检查 v1 是否又出现：出现且比 v2 新就记一行警告，不自动回灌。
pub(super) fn warn_if_v1_returned(root: &Path, backend: Option<&Path>, v2_path: &Path) {
    let Ok(v2_modified) = fs::metadata(v2_path).and_then(|meta| meta.modified()) else {
        return;
    };
    for candidate in v1_candidates(root, backend) {
        let Ok(modified) = fs::metadata(&candidate).and_then(|meta| meta.modified()) else {
            continue;
        };
        if modified > v2_modified {
            eprintln!(
                "WARNING: 又出现比 v2 更新的 v1 桌面状态，本次不回灌，请人工确认：{}",
                candidate.display()
            );
        }
    }
}

/// 读一份 v1 状态文件。
fn read_v1(path: &Path) -> Result<Option<V1State>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    warn_unknown_v1_keys(path, &text);
    serde_json::from_str::<V1State>(text.trim_start_matches('\u{feff}'))
        .map(Some)
        .map_err(|error| error.to_string())
}

/// v1 状态文件的结构：字段名是旧版的 snake_case。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct V1State {
    version: u32,
    workspaces: Vec<RegisteredWorkspace>,
    lane_workspace: HashMap<String, String>,
    lane_pinned: Vec<String>,
    lane_archived: Vec<String>,
    lane_title: HashMap<String, String>,
    lane_order: HashMap<String, Vec<String>>,
    pending_lanes: Vec<V1PendingLane>,
    last_workspace_dir: Option<String>,
    notifications_enabled: Option<bool>,
    theme: Option<String>,
}

/// v1 的待激活车道：字段名是旧版的 snake_case。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct V1PendingLane {
    lane_id: String,
    workspace_id: String,
    destination: String,
    created_at: u64,
}

impl V1State {
    /// 逐字段搬到 v2：禁止把整份对象装进新结构。
    fn into_document(self) -> ClientStateDocument {
        if self.version != 1 {
            eprintln!(
                "WARNING: v1 状态文件声明的结构版本是 {}，仍按 v1 结构逐字段迁移",
                self.version
            );
        }
        let mut document = ClientStateDocument::default();
        document.client.theme = self.theme;
        // 缺省视为开启：轮次结束提示是桌面应用的默认预期行为。
        document.client.notifications_enabled = self.notifications_enabled.unwrap_or(true);
        document.client.last_workspace_dir = self.last_workspace_dir;
        document.client.last_profile_id = Some(LOCAL_PROFILE_ID.to_owned());
        document.client.previous_profile_id = Some(LOCAL_PROFILE_ID.to_owned());
        // 有旧状态文件的人不是新用户：迁移完成即视为已过引导，直接进主界面。
        document.client.onboarding_version = ONBOARDING_VERSION;
        document.profiles.insert(
            LOCAL_PROFILE_ID.to_owned(),
            ProfileSection {
                meta: BackendProfile::local(LOCAL_PROFILE_LABEL),
                projection: ProfileProjection {
                    workspaces: self.workspaces,
                    lane_workspace: self.lane_workspace,
                    lane_pinned: self.lane_pinned,
                    lane_archived: self.lane_archived,
                    lane_title: self.lane_title,
                    lane_order: self.lane_order,
                    pending_lanes: self
                        .pending_lanes
                        .into_iter()
                        .map(|lane| PendingLane {
                            lane_id: lane.lane_id,
                            workspace_id: lane.workspace_id,
                            destination: lane.destination,
                            created_at: lane.created_at,
                        })
                        .collect(),
                },
            },
        );
        document
    }
}

impl ClientStateDocument {
    /// 依据「零档案时的兼容路径」自愈出一份本机档案。
    ///
    /// 只有在**零档案**且后端配置文件存在时生效：这是老用户升级的第一条分支，
    /// 也是「回退读 `config.toml` 永久保留」的落点。命中后返回新档案 id。
    ///
    /// 自愈成功即视为「已过引导」：有 `config.toml` 的人是老用户（§8.4 的三分支），
    /// 不该被当成新用户推到四步引导里。
    pub fn ensure_local_profile(&mut self, backend_config_exists: bool) -> Option<String> {
        if let Some(id) = self.active_profile_id() {
            return Some(id);
        }
        if !backend_config_exists {
            return None;
        }
        self.profiles.insert(
            LOCAL_PROFILE_ID.to_owned(),
            ProfileSection {
                meta: BackendProfile::local(LOCAL_PROFILE_LABEL),
                projection: ProfileProjection::default(),
            },
        );
        self.client.last_profile_id = Some(LOCAL_PROFILE_ID.to_owned());
        self.client.previous_profile_id = Some(LOCAL_PROFILE_ID.to_owned());
        self.client.onboarding_version = ONBOARDING_VERSION;
        Some(LOCAL_PROFILE_ID.to_owned())
    }

    /// 登记一条云档案并返回其 id；已存在同址档案时复用既有 id。
    pub fn upsert_cloud_profile(
        &mut self,
        label: &str,
        http_base: &str,
        ws_base: &str,
        credential: crate::connection::profile::CredentialSource,
    ) -> String {
        for (id, section) in &mut self.profiles {
            if let EndpointSpec::Cloud {
                http_base: existing,
                ..
            } = &section.meta.spec
                && existing == http_base
            {
                section.meta.label = label.to_owned();
                section.meta.credential = credential;
                return id.clone();
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.profiles.insert(
            id.clone(),
            ProfileSection {
                meta: BackendProfile::cloud(label, http_base, ws_base, credential),
                projection: ProfileProjection::default(),
            },
        );
        id
    }
}
