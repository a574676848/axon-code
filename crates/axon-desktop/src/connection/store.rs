//! 客户端自有根与 v2 状态文件：路径解析、v1 迁移、损坏护栏。
//!
//! 客户端自有根固定落在用户主目录下的 `~/.axon-desktop`，与后端运行时根 `~/.axon`
//! 平级、互不包含：一边是客户端自己的主题、档案、附件与草稿，另一边是 daemon 的
//! 配置、数据、会话与凭据，任何一方的环境变量都不得把另一方的目录拉进自己下面。
//!
//! 状态文件用文件名隔离两代结构（[`V1_FILE_NAME`] 与 [`V2_FILE_NAME`]）：旧版
//! `load_state()` 完全不校验版本号，读到新结构会得到「工作区全空」，随后任一次写入
//! 都会把整份状态覆写成空。隔离让旧版永远只动它认识的文件。
//!
//! 写入方式是**直接覆盖**而不是「临时文件加原子替换」：Windows 上原子替换必须先拿到
//! 目标文件的 DELETE 权限，同步盘、索引器与杀毒软件只要短暂持有它，替换就以「拒绝
//! 访问 (os error 5)」失败，而同一时刻按共享写打开再截断是可以成功的。这里的代价
//! 由读取侧兜住：读到坏内容会先另存 `.corrupt` 再回落默认值或迁移 v1。

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use axon_config::ConfigLoader;
use serde::{Deserialize, Serialize};

use crate::connection::profile::BackendProfile;
use crate::state::{PendingLane, RegisteredWorkspace};

use super::store_migration::{migrate_from_v1, warn_if_v1_returned};

/// v1 状态文件名（旧版读写，可能同时出现在客户端根与后端运行时根）。
pub const V1_FILE_NAME: &str = "desktop-state.json";
/// v2 状态文件名（本期新写，只在客户端根）。
pub const V2_FILE_NAME: &str = "desktop-state.v2.json";
/// v1 迁移前备份名（只在客户端根生成一份）。
pub const V1_BACKUP_NAME: &str = "desktop-state.v1.json.bak";
/// 已迁移标记名（只在客户端根生成一份，一经占用不再覆盖）。
pub const V1_SUPERSEDED_NAME: &str = "desktop-state.json.superseded";
/// 损坏副本追加的后缀。
const CORRUPT_SUFFIX: &str = ".corrupt";
/// 本机档案的固定 id。
pub const LOCAL_PROFILE_ID: &str = "local";
/// 本机档案的默认名称。
pub const LOCAL_PROFILE_LABEL: &str = "本机";
/// 引导流程的当前版本号；0 表示尚未完成引导。
pub const ONBOARDING_VERSION: u32 = 1;
/// 客户端自有根环境变量。
pub const CLIENT_ROOT_ENV: &str = "AXON_CLIENT_ROOT";
/// 客户端数据目录名：与后端运行时根 `~/.axon` 平级、互不包含。
pub const CLIENT_HOME_DIR_NAME: &str = ".axon-desktop";
/// 状态文件写入的重试次数。
///
/// 目标文件被杀毒软件或索引器短暂持有是常态，一次性失败不代表真的写不进去；
/// 三次之后仍失败才如实回报。
const WRITE_ATTEMPTS: usize = 3;
/// 两次写入之间的等待。
const WRITE_RETRY_DELAY: Duration = Duration::from_millis(60);

/// 客户端层：全局一份的偏好与定位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientLayer {
    /// 界面主题标识；空值就是默认的纸白主题。
    pub theme: Option<String>,
    /// 是否发送系统通知。
    pub notifications_enabled: bool,
    /// 上一次在原生目录选择器中选中的目录。
    pub last_workspace_dir: Option<String>,
    /// 当前生效档案 id。
    pub last_profile_id: Option<String>,
    /// 切换前的档案 id，供重启失败回滚。
    pub previous_profile_id: Option<String>,
    /// 已完成的引导版本；0 表示尚未完成。
    pub onboarding_version: u32,
}

impl Default for ClientLayer {
    fn default() -> Self {
        Self {
            theme: None,
            notifications_enabled: true,
            last_workspace_dir: None,
            last_profile_id: None,
            previous_profile_id: None,
            onboarding_version: 0,
        }
    }
}

/// 档案投影：每档一份的车道与工作区视图。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileProjection {
    /// 用户登记的工作区清单。
    pub workspaces: Vec<RegisteredWorkspace>,
    /// 车道到所属工作区路径的归属映射。
    pub lane_workspace: HashMap<String, String>,
    /// 已置顶的车道标识。
    pub lane_pinned: Vec<String>,
    /// 已归档的车道标识。
    pub lane_archived: Vec<String>,
    /// 用户重命名后的车道标题。
    pub lane_title: HashMap<String, String>,
    /// 每个工作区下车道的用户自定义顺序。
    pub lane_order: HashMap<String, Vec<String>>,
    /// 本应用新建、尚未产生 Interaction 的车道。
    pub pending_lanes: Vec<PendingLane>,
}

/// 一个档案在状态文件里的整段内容：元数据加投影。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileSection {
    /// 档案元数据（不含任何凭据明文，也不含 `caps`）。
    pub meta: BackendProfile,
    /// 车道与工作区投影，字段与 `meta` 同层。
    #[serde(flatten)]
    pub projection: ProfileProjection,
}

/// v2 状态文件的根结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStateDocument {
    /// 结构版本，本期恒为 2。
    pub version: u32,
    /// 客户端层。
    pub client: ClientLayer,
    /// 档案清单，键为档案 id。
    pub profiles: BTreeMap<String, ProfileSection>,
}

impl Default for ClientStateDocument {
    fn default() -> Self {
        Self {
            version: 2,
            client: ClientLayer::default(),
            profiles: BTreeMap::new(),
        }
    }
}

impl ClientStateDocument {
    /// 取一条档案的投影；档案不存在时返回默认投影。
    #[must_use]
    pub fn projection(&self, profile_id: &str) -> ProfileProjection {
        self.profiles
            .get(profile_id)
            .map(|section| section.projection.clone())
            .unwrap_or_default()
    }

    /// 写入一条档案的投影。
    pub fn set_projection(&mut self, profile_id: &str, projection: ProfileProjection) {
        self.profiles
            .entry(profile_id.to_owned())
            .or_default()
            .projection = projection;
    }

    /// 取一条档案的元数据；档案不存在时为 None。
    #[must_use]
    pub fn meta(&self, profile_id: &str) -> Option<BackendProfile> {
        self.profiles
            .get(profile_id)
            .map(|section| section.meta.clone())
    }

    /// 当前生效档案 id；未登记时返回 None。
    #[must_use]
    pub fn active_profile_id(&self) -> Option<String> {
        let recorded = self.client.last_profile_id.as_deref();
        match recorded {
            Some(id) if self.profiles.contains_key(id) => Some(id.to_owned()),
            _ => {
                if self.profiles.contains_key(LOCAL_PROFILE_ID) {
                    Some(LOCAL_PROFILE_ID.to_owned())
                } else {
                    self.profiles.keys().next().cloned()
                }
            }
        }
    }
}

/// 一次加载的结果。
#[derive(Debug, Clone)]
pub struct LoadedDocument {
    /// 状态文档。
    pub document: ClientStateDocument,
    /// 文件来自更高的结构版本：只读，不写盘。
    pub read_only: bool,
    /// 本次加载完成了 v1 到 v2 的迁移。
    pub migrated_from_v1: bool,
    /// 损坏副本路径（发生了才会是 `Some`）。
    pub corrupt_backup: Option<PathBuf>,
}

/// 解析客户端自有根。
///
/// 优先级：`AXON_CLIENT_ROOT` > 用户主目录下的 `~/.axon/client`。它**不读**
/// `AXON_RUNTIME_ROOT`：两者互不推导，任一设置都不得改变另一方的解析结果。
#[must_use]
pub fn client_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| resolve_client_root(std::env::var_os(CLIENT_ROOT_ENV).as_deref()))
        .clone()
}

/// 解析客户端自有根的纯函数形态，便于按环境组合逐项断言。
#[must_use]
pub fn resolve_client_root(override_root: Option<&OsStr>) -> PathBuf {
    if let Some(custom) = override_root.filter(|value| !value.is_empty()) {
        let candidate = PathBuf::from(custom);
        if !is_inside_backend_root(&candidate) {
            return candidate;
        }
        // 落在后端自己的运行态目录里会与 daemon 抢同一批文件，直接退回默认位置。
        eprintln!(
            "WARNING: {CLIENT_ROOT_ENV} 指向后端运行态目录，已忽略并改用默认位置：{}",
            candidate.display()
        );
    }
    platform_default_client_root(
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("USERPROFILE").as_deref(),
    )
}

/// 默认客户端根：用户主目录下的 `.axon-desktop`。
///
/// 三平台同一条规则。它与后端运行时根 `~/.axon` 平级：客户端不往后端目录里写任何
/// 文件，后端也不碰客户端目录，清理与迁移互不牵连。
#[must_use]
pub fn platform_default_client_root(home: Option<&OsStr>, user_profile: Option<&OsStr>) -> PathBuf {
    let base = home
        .filter(|value| !value.is_empty())
        .or_else(|| user_profile.filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_default();
    base.join(CLIENT_HOME_DIR_NAME)
}

/// 后端运行时根（daemon 存放 `config.toml` 与 `data/` 的目录）。
#[must_use]
pub fn backend_root() -> Option<PathBuf> {
    ConfigLoader::default_user_axon_root_path().ok()
}

/// 候选是否落在后端的运行态目录里。
#[must_use]
pub fn is_inside_backend_root(candidate: &Path) -> bool {
    let Some(root) = backend_root() else {
        return false;
    };
    is_inside_backend_root_at(candidate, &root)
}

/// 候选是否落在给定后端运行时根之内。
///
/// 两处目录是平级的（`~/.axon-desktop` 与 `~/.axon`），因此任何落在后端根之内的
/// 候选都要拒绝：那里是 daemon 的配置、数据、会话与凭据，客户端写进去只会互相踩踏。
#[must_use]
pub fn is_inside_backend_root_at(candidate: &Path, root: &Path) -> bool {
    let candidate = normalize_for_compare(candidate);
    let root = normalize_for_compare(root);
    candidate == root || candidate.starts_with(&root)
}

/// 比较用的路径归一：去掉尾部分隔符，Windows 下按小写比较。
fn normalize_for_compare(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    let normalized = if trimmed.is_empty() { "\\" } else { trimmed };
    if cfg!(windows) {
        PathBuf::from(normalized.to_lowercase())
    } else {
        PathBuf::from(normalized)
    }
}

/// v2 状态文件路径。
#[must_use]
pub fn state_file_path() -> PathBuf {
    state_file_path_at(&client_root())
}

/// 指定客户端根下的 v2 状态文件路径。
#[must_use]
pub fn state_file_path_at(client_root: &Path) -> PathBuf {
    client_root.join(V2_FILE_NAME)
}

/// 附件目录（客户端根）。
#[must_use]
pub fn attachments_dir() -> PathBuf {
    client_root().join("attachments")
}

/// 界面失败接管日志（客户端根）。
#[must_use]
pub fn ui_failure_log_path() -> PathBuf {
    client_root().join("logs").join("desktop-ui-failure.jsonl")
}

/// 云档案输入历史文件：客户端根下按档案 id 分文件。
///
/// 本机档案不用这条路径：它继续读后端运行时根下的
/// `data/runtime/draft_history.jsonl`，与 TUI 共享同一份历史。
#[must_use]
pub fn profile_draft_history_path(profile_id: &str) -> PathBuf {
    client_root()
        .join("draft-history")
        .join(format!("{}.jsonl", sanitize_file_stem(profile_id)))
}

/// 把任意档案 id 收敛成安全文件名。
fn sanitize_file_stem(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "profile".to_owned()
    } else {
        cleaned
    }
}

/// 加载 v2 状态文件，必要时从 v1 迁移。
///
/// 任何一次读盘或解析失败都不阻断界面：损坏文件先另存 `.corrupt` 并记一行警告，
/// 然后回落默认值。现状 `unwrap_or_default()` 的静默清空会让用户在一次拖拽、
/// 一次置顶或一次改主题之后永久丢掉全部工作区。
#[must_use]
pub fn load_document() -> LoadedDocument {
    let root = client_root();
    load_document_at(&root, backend_root().as_deref())
}

/// 在显式给定的两处根目录上加载 v2，并允许从 v1 迁移。
///
/// 显式入参是迁移逻辑可被逐条断言的前提：真实路径由环境决定，而迁移规则必须能在
/// 临时目录上把「两处都有 v1」「目标名已被占用」这类组合逐项复现。
#[must_use]
pub fn load_document_at(root: &Path, backend: Option<&Path>) -> LoadedDocument {
    let v2_path = root.join(V2_FILE_NAME);
    match read_document(&v2_path) {
        ReadOutcome::Missing => migrate_from_v1(root, backend),
        ReadOutcome::Parsed(document) if document.version > 2 => LoadedDocument {
            document,
            read_only: true,
            migrated_from_v1: false,
            corrupt_backup: None,
        },
        ReadOutcome::Parsed(document) => {
            // v1 又出现（旧版二进制被再次运行过）时只记一行警告，不自动回灌：
            // 本期假设升级是单向的，真要合并由用户决定，提示里给出文件路径。
            warn_if_v1_returned(root, backend, &v2_path);
            let mut document = document;
            normalize_local_label(&mut document);
            LoadedDocument {
                document,
                read_only: false,
                migrated_from_v1: false,
                corrupt_backup: None,
            }
        }
        ReadOutcome::Broken(reason) => {
            eprintln!(
                "WARNING: 桌面状态文件解析失败，已回落默认值：{}（{reason}）",
                v2_path.display()
            );
            let corrupt_backup = backup_corrupt(&v2_path);
            let mut migrated = migrate_from_v1(root, backend);
            migrated.corrupt_backup = corrupt_backup;
            migrated
        }
    }
}

/// 本机档案的显示名由代码决定。
///
/// 老版本用主机名派生过默认名（「我的开发机」之类），那些字符串会一直留在盘上。
/// 读到旧名字时就地改成当前口径，下一次写盘自然带上；不新增迁移步骤，也不要求
/// 用户为了改一个称呼去编辑状态文件。
fn normalize_local_label(document: &mut ClientStateDocument) {
    let Some(section) = document.profiles.get_mut(LOCAL_PROFILE_ID) else {
        return;
    };
    if section.meta.label != LOCAL_PROFILE_LABEL {
        section.meta.label = LOCAL_PROFILE_LABEL.to_owned();
    }
}

/// 读一份 v2 文件的三种结果。
enum ReadOutcome {
    /// 文件不存在。
    Missing,
    /// 解析成功。
    Parsed(ClientStateDocument),
    /// 文件存在但解析失败。
    Broken(String),
}

fn read_document(path: &Path) -> ReadOutcome {
    let Ok(text) = fs::read_to_string(path) else {
        return ReadOutcome::Missing;
    };
    match serde_json::from_str::<ClientStateDocument>(text.trim_start_matches('\u{feff}')) {
        Ok(document) if document.version >= 2 => ReadOutcome::Parsed(document),
        Ok(document) => ReadOutcome::Broken(format!("结构版本 {} 不是 v2", document.version)),
        Err(error) => ReadOutcome::Broken(error.to_string()),
    }
}

/// 写 v2 状态文件。
///
/// # Errors
///
/// 序列化失败或写盘失败。
pub fn save_document(document: &ClientStateDocument) -> Result<(), String> {
    save_document_at(&client_root(), document)
}

/// 指定客户端根下的 v2 写盘。
///
/// # Errors
///
/// 序列化失败或写盘失败。
pub fn save_document_at(root: &Path, document: &ClientStateDocument) -> Result<(), String> {
    let path = root.join(V2_FILE_NAME);
    // 纵深防御：盘上那份若是更高的结构版本，任何调用点都不得覆写它。
    if let ReadOutcome::Parsed(existing) = read_document(&path)
        && existing.version > 2
    {
        return Err(format!(
            "桌面状态文件来自更高的结构版本（{}），本次只读运行，不做任何写入",
            existing.version
        ));
    }
    let text = serde_json::to_string_pretty(document)
        .map_err(|error| format!("序列化桌面状态失败：{error}"))?;
    write_document(&path, text.as_bytes())
}

/// 直接覆盖写一份状态文件。
///
/// 先建目录、再按共享写打开并截断，因此不需要目标文件的 DELETE 权限，也就不会
/// 被持有该文件的同步盘或杀毒软件拦成「拒绝访问」。失败按 [`WRITE_ATTEMPTS`]
/// 次有限重试，仍失败才把最后一次的原因交回调用方。
fn write_document(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建客户端数据目录：{error}"))?;
    }
    let mut reason = String::new();
    for attempt in 0..WRITE_ATTEMPTS {
        match fs::write(path, bytes) {
            Ok(()) => return Ok(()),
            Err(error) => {
                reason = error.to_string();
                if attempt + 1 < WRITE_ATTEMPTS {
                    std::thread::sleep(WRITE_RETRY_DELAY);
                }
            }
        }
    }
    Err(format!("写入桌面状态失败：{reason}"))
}

/// 把损坏文件另存为 `.corrupt`；目标已存在时追加序号，不覆盖既有证据。
pub(super) fn backup_corrupt(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let target = unique_suffixed_path(path, CORRUPT_SUFFIX);
    fs::copy(path, &target).ok().map(|_| target)
}

/// 在 `path` 后追加后缀；同名已存在时依次尝试 `.2`、`.3`。
pub(super) fn unique_suffixed_path(path: &Path, suffix: &str) -> PathBuf {
    let base = format!("{}{suffix}", path.display());
    let mut candidate = PathBuf::from(&base);
    let mut index = 2_u32;
    while candidate.exists() {
        candidate = PathBuf::from(format!("{base}.{index}"));
        index += 1;
    }
    candidate
}
