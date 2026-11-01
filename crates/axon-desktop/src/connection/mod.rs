//! 连接档案：端点解析、状态分层、凭据存储与探活。
//!
//! 同一时刻只有一个档案连接（合同 §4.1）。切换档案等于整链重建，并允许直接重启
//! 应用；非生效档案不主动探测，健康标记只反映当前生效档案。
//!
//! 模块分工：
//!
//! | 文件 | 职责 |
//! |---|---|
//! | [`profile`] | 数据模型：端点规格、凭据来源、能力位三态 |
//! | [`store`] | 客户端自有根与 v2 状态文件（含 v1 迁移与损坏护栏） |
//! | [`secret_store`] | 系统凭据库与试连接期临时凭据的生命周期 |
//! | [`endpoint`] | 端点三级解析与云端地址规范化 |
//! | [`probe`] | `/api/v1/meta` 探活与失败三分类 |
//! | [`views`] | 读数的展示形态：快照、能力位清单与试连接回执 |
//! | [`commands`] | Tauri IPC 面：状态、试连接、档案增删改、切换与探测 |
//! | [`keys`] | 密钥管理面：经网关列出、签发与吊销 Gateway 多 Key |
//! | [`relaunch`] | 切档重启：launcher 专用解析、中继进程与失败回滚 |
//!
//! 本文件是**运行态**：把上面几层装配成一份进程内事实。界面与网关客户端都只读
//! 这一份事实，不各自再查一次盘。

pub mod commands;
pub mod endpoint;
pub mod keys;
pub mod probe;
pub mod profile;
pub mod relaunch;
pub mod secret_store;
pub mod store;
/// v1 状态文件的读取、归档与迁移。
mod store_migration;
pub mod tls;
pub mod views;

use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use axon_config::ConfigLoader;

use self::profile::{BackendProfile, CapsState, CredentialSource, EndpointSpec, ProfileEntry};
use self::store::{ClientStateDocument, ProfileProjection};

/// 进程内唯一的连接运行态。
#[must_use]
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(Runtime::bootstrap)
}

/// 运行态内部状态。
struct Inner {
    /// v2 状态文档，进程内唯一一份。
    document: ClientStateDocument,
    /// 生效档案；`None` 表示当前没有可用档案（未连接态）。
    active: Option<ProfileEntry>,
    /// 状态文件来自更高的结构版本：只读，任何写入都被拒绝。
    read_only: bool,
    /// 本机档案的凭据环境变量名，启动时解析一次。
    token_env: String,
    /// `SessionOnly` 档案在本次会话内输入的凭据，永不落盘。
    session_secret: Option<String>,
    /// 启动期需要告诉用户一次的事实（迁移、损坏、只读）。
    notice: Option<String>,
    /// 健康结论：只反映当前生效档案。
    health: HealthState,
    /// 最近一次成功的探活结论；探测失败或换档即清空。
    last_probe: Option<probe::ProbeReady>,
}

/// 健康结论：只有一档状态词加一句可执行的说明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthState {
    /// 供界面选状态点的档位。
    pub dot: HealthDot,
    /// 中文状态词。
    pub label: String,
    /// 一句可执行的说明。
    pub detail: String,
}

/// 状态点档位，与界面 `StateDot` 的语义一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthDot {
    /// 尚未探测。
    Idle,
    /// 连接中。
    Ongoing,
    /// 已连接。
    Done,
    /// 连不上、凭据无效。
    Error,
    /// 凭据缺失、后台需升级。
    Warning,
}

impl HealthState {
    /// 尚未探测：入口可见但置灰，不得判为版本过旧。
    #[must_use]
    pub fn idle() -> Self {
        Self {
            dot: HealthDot::Idle,
            label: "未连接".to_owned(),
            detail: "尚未探测当前档案".to_owned(),
        }
    }

    /// 连接中。
    #[must_use]
    pub fn ongoing() -> Self {
        Self {
            dot: HealthDot::Ongoing,
            label: "连接中".to_owned(),
            detail: "正在取后端能力位".to_owned(),
        }
    }

    /// 凭据库缺少这把 Key（换机、重装后最常见）。
    #[must_use]
    pub fn credential_missing() -> Self {
        Self {
            dot: HealthDot::Warning,
            label: "需重新输入凭据".to_owned(),
            detail: "本机凭据库里没有这把 Key，请在档案里重新输入".to_owned(),
        }
    }

    /// 后台需要升级。
    #[must_use]
    pub fn backend_outdated() -> Self {
        Self {
            dot: HealthDot::Warning,
            label: "后台需升级".to_owned(),
            detail: "后端不含 /api/v1/meta，能力位按最小集处理".to_owned(),
        }
    }
}

/// 连接运行态。
pub struct Runtime {
    inner: RwLock<Inner>,
}

impl Runtime {
    /// 从状态文件装配运行态。
    ///
    /// 装配顺序固定：读文档 → 若为只读或损坏则先记住要提示的事实 → 零档案自愈 →
    /// 解析生效档案 → 缓存凭据变量名。自愈只在**非只读**时写盘。
    fn bootstrap() -> Self {
        let loaded = store::load_document();
        let mut document = loaded.document;
        let mut notice = None;
        if loaded.read_only {
            notice =
                Some("桌面状态文件来自更高的结构版本，本次只读运行，不写入任何状态".to_owned());
        }
        if let Some(backup) = loaded.corrupt_backup.as_ref() {
            notice = Some(format!(
                "桌面状态文件已损坏，原文件已另存为 {}，本次使用默认设置",
                backup.display()
            ));
        }
        if loaded.migrated_from_v1 && notice.is_none() {
            notice = Some("桌面状态已从旧版结构迁移，原文件已归档保留".to_owned());
        }

        let backend_config_exists = ConfigLoader::default_user_config_path()
            .map(|path| path.exists())
            .unwrap_or(false);
        let was_empty = document.profiles.is_empty();
        document.ensure_local_profile(backend_config_exists);
        if was_empty
            && !document.profiles.is_empty()
            && !loaded.read_only
            && let Err(error) = store::save_document(&document)
        {
            eprintln!("WARNING: 自愈出的本机档案写入失败：{error}");
        }

        let active = document
            .active_profile_id()
            .and_then(|id| document.meta(&id).map(|meta| ProfileEntry::new(id, meta)));
        let token_env = resolve_token_env_name();
        // 生效档案没有凭据时先给「需重新输入」，避免界面把一个空凭据报成连不上。
        let health = initial_health(active.as_ref());
        Self {
            inner: RwLock::new(Inner {
                document,
                active,
                read_only: loaded.read_only,
                token_env,
                session_secret: None,
                notice,
                health,
                last_probe: None,
            }),
        }
    }

    /// 建一个只用于测试的运行态：文档由调用方给定，不读写真实路径。
    #[must_use]
    pub fn from_document(document: ClientStateDocument, read_only: bool) -> Self {
        let active = document
            .active_profile_id()
            .and_then(|id| document.meta(&id).map(|meta| ProfileEntry::new(id, meta)));
        Self {
            inner: RwLock::new(Inner {
                document,
                active,
                read_only,
                token_env: endpoint::FALLBACK_TOKEN_ENV.to_owned(),
                session_secret: None,
                notice: None,
                health: HealthState::idle(),
                last_probe: None,
            }),
        }
    }

    /// 读锁；锁中毒时取回内部值并继续，因为运行态没有需要回滚的不变量。
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 写锁；中毒处理同上。
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 状态文档的只读副本。
    #[must_use]
    pub fn document(&self) -> ClientStateDocument {
        self.read().document.clone()
    }

    /// 状态文档是否只读。
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read().read_only
    }

    /// 启动期一次性提示。
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        self.read().notice.clone()
    }

    /// 追加一条启动期提示；只保留最近一条。
    pub fn set_notice(&self, notice: Option<String>) {
        self.write().notice = notice;
    }

    /// 当前健康结论。
    #[must_use]
    pub fn health(&self) -> HealthState {
        self.read().health.clone()
    }

    /// 最近一次成功的探活结论。
    #[must_use]
    pub fn last_probe(&self) -> Option<probe::ProbeReady> {
        self.read().last_probe.clone()
    }

    /// 标记进入探测中。
    pub fn mark_probing(&self) {
        self.write().health = HealthState::ongoing();
    }

    /// 标记回到「尚未探测」：凭据刚补上、还没发起请求。
    pub fn mark_idle(&self) {
        self.write().health = HealthState::idle();
    }

    /// 记下一次探活的结论。
    ///
    /// 结论同时决定健康点、能力位与「最近一次读数」：三者必须同源，界面才不会
    /// 出现「胶囊说已连接、连接分区说版本未知」这种自相矛盾。
    pub fn record_probe(&self, outcome: &probe::ProbeOutcome) {
        let mut inner = self.write();
        inner.last_probe = None;
        inner.health = match outcome {
            probe::ProbeOutcome::Ready(ready) => {
                inner.last_probe = Some((**ready).clone());
                HealthState {
                    dot: HealthDot::Done,
                    label: "已连接".to_owned(),
                    detail: format!("往返 {} 毫秒", ready.latency_ms),
                }
            }
            probe::ProbeOutcome::Outdated => HealthState::backend_outdated(),
            probe::ProbeOutcome::Failed(failure) => match failure {
                probe::ProbeFailure::CredentialRejected { .. } => HealthState {
                    dot: HealthDot::Error,
                    label: "凭据无效".to_owned(),
                    detail: failure.message(),
                },
                probe::ProbeFailure::ProtocolUnsupported(_) => HealthState {
                    dot: HealthDot::Warning,
                    label: "需升级客户端".to_owned(),
                    detail: failure.message(),
                },
                _ => HealthState {
                    dot: HealthDot::Error,
                    label: "连不上".to_owned(),
                    detail: failure.message(),
                },
            },
        };
        let caps = outcome.caps_state();
        if let Some(active) = inner.active.as_mut() {
            active.profile.caps = caps;
            let id = active.id.clone();
            let meta = BackendProfile {
                caps: CapsState::Unknown,
                ..active.profile.clone()
            };
            if let Some(section) = inner.document.profiles.get_mut(&id) {
                section.meta = meta;
            }
        }
    }

    /// 标记凭据缺失（换机、重装后最常见）。
    pub fn mark_credential_missing(&self) {
        self.write().health = HealthState::credential_missing();
    }

    /// 生效档案记录。
    #[must_use]
    pub fn active(&self) -> Option<ProfileEntry> {
        self.read().active.clone()
    }

    /// 生效档案 id。
    #[must_use]
    pub fn active_profile_id(&self) -> Option<String> {
        self.read().active.as_ref().map(|entry| entry.id.clone())
    }

    /// 全部档案元数据，按 id 排序。
    #[must_use]
    pub fn profiles(&self) -> Vec<ProfileEntry> {
        let inner = self.read();
        inner
            .document
            .profiles
            .iter()
            .map(|(id, section)| ProfileEntry::new(id.clone(), section.meta.clone()))
            .collect()
    }

    /// 本机档案的凭据环境变量名。
    #[must_use]
    pub fn token_env_name(&self) -> String {
        self.read().token_env.clone()
    }

    /// 生效档案的能力位（未经过模式覆盖）。
    #[must_use]
    pub fn caps(&self) -> CapsState {
        self.read()
            .active
            .as_ref()
            .map(|entry| entry.profile.caps)
            .unwrap_or_default()
    }

    /// 生效档案经模式覆盖后的能力位；这是界面唯一依据。
    #[must_use]
    pub fn effective_caps(&self) -> CapsState {
        self.read()
            .active
            .as_ref()
            .map(|entry| entry.profile.effective_caps())
            .unwrap_or_default()
    }

    /// 更新生效档案的能力位。
    pub fn set_caps(&self, caps: CapsState) {
        let mut inner = self.write();
        let (id, meta) = match inner.active.as_mut() {
            Some(active) => {
                active.profile.caps = caps;
                // 元数据写回文档，保持「文档是唯一事实」；caps 带 `serde(skip)`，不落盘。
                (
                    active.id.clone(),
                    BackendProfile {
                        caps: CapsState::Unknown,
                        ..active.profile.clone()
                    },
                )
            }
            None => return,
        };
        if let Some(section) = inner.document.profiles.get_mut(&id) {
            section.meta = meta;
        }
    }

    /// 生效档案的凭据来源。
    #[must_use]
    pub fn credential_source(&self) -> Option<CredentialSource> {
        self.read()
            .active
            .as_ref()
            .map(|entry| entry.profile.credential.clone())
    }

    /// 非 `.env` 来源的凭据（凭据库或本次会话）。
    ///
    /// `.env` 来源返回 None：那一支由 [`endpoint::api_token`] 走配置相邻 `.env` 与
    /// 进程缓存，不在这里另建一份。
    #[must_use]
    pub fn profile_api_token(&self) -> Option<String> {
        let (source, session) = {
            let inner = self.read();
            let source = inner
                .active
                .as_ref()
                .map(|entry| entry.profile.credential.clone())?;
            (source, inner.session_secret.clone())
        };
        match source {
            CredentialSource::LocalEnvFile => None,
            CredentialSource::Keychain { key_id } => secret_store::load(&key_id).ok().flatten(),
            CredentialSource::SessionOnly => session,
        }
    }

    /// 记录本次会话内输入的凭据（`SessionOnly` 档案用）。
    pub fn set_session_secret(&self, secret: Option<String>) {
        self.write().session_secret = secret;
    }

    /// 生效档案的 HTTP 与 WebSocket 端点。
    #[must_use]
    pub fn endpoints(&self) -> endpoint::ResolvedEndpoints {
        endpoint::active_endpoints()
    }

    /// 读-改-写状态文档。
    ///
    /// 只读模式下拒绝任何写入并给出中文原因；写入成功后立刻刷新生效档案，
    /// 使界面下一次读取就看到新值。
    ///
    /// # Errors
    ///
    /// 只读模式，或闭包返回错误。
    pub fn update_document<T>(
        &self,
        update: impl FnOnce(&mut ClientStateDocument) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut inner = self.write();
        if inner.read_only {
            return Err("桌面状态文件来自更高的结构版本，本次只读运行，无法保存改动".to_owned());
        }
        let outcome = update(&mut inner.document)?;
        store::save_document(&inner.document)?;
        refresh_active(&mut inner);
        Ok(outcome)
    }

    /// 取一条档案的投影（车道与工作区视图）。
    #[must_use]
    pub fn projection(&self, profile_id: &str) -> ProfileProjection {
        self.read().document.projection(profile_id)
    }

    /// 写入一条档案的投影。
    ///
    /// # Errors
    ///
    /// 只读模式或写盘失败。
    pub fn set_projection(
        &self,
        profile_id: &str,
        projection: ProfileProjection,
    ) -> Result<(), String> {
        self.update_document(|document| {
            document.set_projection(profile_id, projection);
            Ok(())
        })
    }

    /// 切换生效档案。
    ///
    /// 只改状态文件里的两个 id：真正重建连接由「重启应用」完成（事件流是进程级
    /// 单例）。切换前把当前值记入 `previousProfileId`，供重启失败时回滚。
    ///
    /// # Errors
    ///
    /// 档案不存在、目标已是生效档案、只读模式或写盘失败。
    pub fn activate_profile(&self, profile_id: &str) -> Result<(), String> {
        let mut inner = self.write();
        // 先判「档案是否存在」：一个不存在的 id 无论是否只读都该被拒绝，
        // 这条判定也因此不需要写盘。
        if !inner.document.profiles.contains_key(profile_id) {
            return Err(format!("档案不存在：{profile_id}"));
        }
        if inner.read_only {
            return Err("桌面状态文件来自更高的结构版本，本次只读运行，无法切换档案".to_owned());
        }
        if inner.document.client.last_profile_id.as_deref() == Some(profile_id) {
            return Ok(());
        }
        let previous = inner.document.client.last_profile_id.clone();
        inner.document.client.previous_profile_id = previous;
        inner.document.client.last_profile_id = Some(profile_id.to_owned());
        store::save_document(&inner.document)?;
        refresh_active(&mut inner);
        // 切档即重取能力位：迟滞计数与能力位都按档案隔离，切换即清零。
        Ok(())
    }

    /// 回滚到 `previousProfileId`。
    ///
    /// 回滚目标可能已被删除（档案可删，生效中的除外；回滚后它就不再是生效中的那个），
    /// 因此回滚前必须校验目标存在，不存在时**保持当前档案不变**，绝不把
    /// `lastProfileId` 指向一个不存在的 id。
    ///
    /// # Returns
    ///
    /// `Ok(true)` 表示已回滚；`Ok(false)` 表示目标缺失、保持原样。
    ///
    /// # Errors
    ///
    /// 只读模式或写盘失败。
    pub fn rollback_profile(&self) -> Result<bool, String> {
        let mut inner = self.write();
        if inner.read_only {
            return Err("桌面状态文件来自更高的结构版本，本次只读运行，无法回滚档案".to_owned());
        }
        let Some(previous) = inner.document.client.previous_profile_id.clone() else {
            return Ok(false);
        };
        if !inner.document.profiles.contains_key(&previous) {
            return Ok(false);
        }
        inner.document.client.last_profile_id = Some(previous);
        store::save_document(&inner.document)?;
        refresh_active(&mut inner);
        Ok(true)
    }

    /// 刷新状态文档（供档案增删改后调用）。
    ///
    /// # Errors
    ///
    /// 只读模式或写盘失败。
    pub fn replace_document(&self, document: ClientStateDocument) -> Result<(), String> {
        let mut inner = self.write();
        if inner.read_only {
            return Err("桌面状态文件来自更高的结构版本，本次只读运行，无法保存改动".to_owned());
        }
        store::save_document(&document)?;
        inner.document = document;
        refresh_active(&mut inner);
        Ok(())
    }

    /// 已完成的引导版本。
    #[must_use]
    pub fn onboarding_version(&self) -> u32 {
        self.read().document.client.onboarding_version
    }

    /// 记录引导完成。
    ///
    /// # Errors
    ///
    /// 只读模式或写盘失败。
    pub fn complete_onboarding(&self) -> Result<(), String> {
        self.update_document(|document| {
            document.client.onboarding_version = store::ONBOARDING_VERSION;
            Ok(())
        })
    }

    /// 状态文件路径，供界面展示与排查。
    #[must_use]
    pub fn state_file() -> PathBuf {
        store::state_file_path()
    }
}

/// 从文档重新推导生效档案。
///
/// 换档即清零：能力位、最近一次探活读数与健康结论都按档案隔离，迟滞计数同理。
fn refresh_active(inner: &mut Inner) {
    inner.active = inner.document.active_profile_id().and_then(|id| {
        inner
            .document
            .meta(&id)
            .map(|meta| ProfileEntry::new(id, meta))
    });
    inner.last_probe = None;
    inner.health = initial_health(inner.active.as_ref());
}

/// 启动期健康结论：没有档案就是未连接，云端档案缺凭据就是需重新输入。
fn initial_health(active: Option<&ProfileEntry>) -> HealthState {
    let Some(entry) = active else {
        return HealthState::idle();
    };
    if entry.profile.credential.needs_user_input() {
        return HealthState::credential_missing();
    }
    if entry
        .profile
        .credential
        .keychain_key()
        .is_some_and(|key_id| secret_store::load(key_id).ok().flatten().is_none())
    {
        // 换机或重装之后凭据库里没有这把 Key：这不是服务端拒绝，而是本机没有。
        return HealthState::credential_missing();
    }
    HealthState::idle()
}

/// 解析本机档案的凭据环境变量名。
///
/// 只读一个键，且只在进程启动时读一次：凭据名是配置里的稳定事实。
fn resolve_token_env_name() -> String {
    let Ok(config_path) = ConfigLoader::default_user_config_path() else {
        return endpoint::FALLBACK_TOKEN_ENV.to_owned();
    };
    axon_config::raw_string_value(&config_path, endpoint::KEY_API_TOKEN_ENV)
        .unwrap_or_else(|| endpoint::FALLBACK_TOKEN_ENV.to_owned())
}

/// 生效档案是否为云端档案。
#[must_use]
pub fn active_is_cloud() -> bool {
    runtime()
        .active()
        .is_some_and(|entry| matches!(entry.profile.spec, EndpointSpec::Cloud { .. }))
}

/// 生效档案的中文名称；无档案时给「未连接」。
#[must_use]
pub fn active_label() -> String {
    runtime()
        .active()
        .map(|entry| display_label(&entry.profile))
        .unwrap_or_else(|| "未连接".to_owned())
}

/// 档案名称：空值回落到「未命名档案」，绝不把空串交给界面。
#[must_use]
pub fn display_label(profile: &BackendProfile) -> String {
    let trimmed = profile.label.trim();
    if trimmed.is_empty() {
        "未命名档案".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
