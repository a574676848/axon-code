//! 连接档案的数据模型：端点规格、凭据来源、能力位三态。
//!
//! 这一层只有数据与判定，不碰磁盘、不碰网络。落盘形态由 [`super::store`] 负责，
//! 端点解析由 [`super::endpoint`] 负责，探活由 [`super::probe`] 负责。

use serde::{Deserialize, Serialize};

/// 能力位的 12 个名字：本常量是全仓唯一事实源。
///
/// 后端 `/api/v1/meta` 的 `capabilities` 只允许出现这 12 个键，客户端也只按这
/// 12 个键判定；多一个或少一个都算协议不合规，不做「缺了就默认为 false」的宽容。
pub const CAPABILITY_KEYS: [&str; 12] = [
    "files",
    "dirs",
    "attachments",
    "config",
    "secrets",
    "providers",
    "mcp",
    "skills",
    "documents",
    "system_status",
    "service_control",
    "terminal",
];

/// 后端自报的能力位集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct CapabilitySet {
    /// 服务端目录列表。
    pub files: bool,
    /// 服务端目录浏览。
    pub dirs: bool,
    /// 附件上传。
    pub attachments: bool,
    /// 配置文本读写。
    pub config: bool,
    /// 密钥状态与写入。
    pub secrets: bool,
    /// 上游模型发现。
    pub providers: bool,
    /// MCP 配置。
    pub mcp: bool,
    /// 技能清单。
    pub skills: bool,
    /// 人设与行为文件。
    pub documents: bool,
    /// 运行态状态查询。
    pub system_status: bool,
    /// 后台服务控制（重启、安装）。
    pub service_control: bool,
    /// 交互式终端。
    pub terminal: bool,
}

impl CapabilitySet {
    /// 解析 `/meta` 的 `capabilities` 对象。
    ///
    /// 严格要求 12 个键齐全且无未登记键：这是「后端支持哪些路由族」的协议事实，
    /// 缺键说明对端不是本协议的实现者，必须显式失败而不是猜。
    ///
    /// # Errors
    ///
    /// 不是对象、缺键或多键时返回中文原因。
    pub fn from_meta_json(value: &serde_json::Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "能力位不是对象".to_string())?;
        for key in object.keys() {
            if !CAPABILITY_KEYS.contains(&key.as_str()) {
                return Err(format!("能力位出现未登记的键：{key}"));
            }
        }
        for key in CAPABILITY_KEYS {
            if !object.contains_key(key) {
                return Err(format!("能力位缺少 {key}"));
            }
        }
        serde_json::from_value(value.clone())
            .map_err(|error| format!("能力位取值不是布尔：{error}"))
    }

    /// 按档案模式覆盖后的结果。
    ///
    /// 覆盖规则存在的理由：后端无法知道自己被谁连接。完全信任自报会让云档案连到
    /// 一台返回 `terminal: true` 的后端时，显示一个「开在别人机器上」的终端入口。
    #[must_use]
    pub fn with_mode_override(mut self, cloud: bool) -> Self {
        self.service_control = !cloud;
        self.terminal = !cloud;
        self
    }

    /// 逐项列举能力位名称与取值，供摘要展示。
    #[must_use]
    pub fn entries(&self) -> Vec<(&'static str, bool)> {
        vec![
            ("files", self.files),
            ("dirs", self.dirs),
            ("attachments", self.attachments),
            ("config", self.config),
            ("secrets", self.secrets),
            ("providers", self.providers),
            ("mcp", self.mcp),
            ("skills", self.skills),
            ("documents", self.documents),
            ("system_status", self.system_status),
            ("service_control", self.service_control),
            ("terminal", self.terminal),
        ]
    }
}

/// 已取到能力位之后的事实：集合本身，加上「后端版本过旧」这一独立结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownCaps {
    /// 覆盖后的能力位。
    pub set: CapabilitySet,
    /// `/meta` 返回 404 但既有鉴权路由返回 200：活着，但没有本协议的接口。
    pub backend_outdated: bool,
}

/// 能力位三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapsState {
    /// 尚未成功取到 `/meta`。入口可见但置灰，**不得**判为版本过旧。
    #[default]
    Unknown,
    /// 已取到（含「老后端」这一子情形）。
    Known(KnownCaps),
}

impl CapsState {
    /// 覆盖后的能力位；未知时为 None。
    #[must_use]
    pub fn set(self) -> Option<CapabilitySet> {
        match self {
            Self::Unknown => None,
            Self::Known(known) => Some(known.set),
        }
    }

    /// 该能力位是否应当隐藏：只有「已知且为 false」才隐藏。
    ///
    /// 与「置灰」的区别是本合同消除多口径的关键：`Known(false)` 是后端的确定事实，
    /// 留着只会误导；`Unknown` 才置灰可见。
    #[must_use]
    pub fn hides(self, capability: bool) -> bool {
        match self {
            Self::Unknown => false,
            Self::Known(_) => !capability,
        }
    }

    /// 后端版本是否已判定为过旧。
    #[must_use]
    pub fn backend_outdated(self) -> bool {
        matches!(self, Self::Known(known) if known.backend_outdated)
    }
}

/// 端点规格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EndpointSpec {
    /// 本机：从 `config.toml` 的 `http_bind` / `websocket_bind` 派生，缺失时兜底。
    Local,
    /// 云端：显式地址。
    Cloud {
        /// HTTP 根地址，形如 `https://axon.example.com`。
        #[serde(rename = "httpBase")]
        http_base: String,
        /// WebSocket 根地址，形如 `wss://axon.example.com`。
        #[serde(rename = "wsBase")]
        ws_base: String,
    },
}

impl Default for EndpointSpec {
    /// 缺省按本机档案处理：老用户升级后必须先连上本机。
    fn default() -> Self {
        Self::Local
    }
}

impl EndpointSpec {
    /// 是否为云端档案。
    #[must_use]
    pub const fn is_cloud(&self) -> bool {
        matches!(self, Self::Cloud { .. })
    }

    /// 对外展示的模式名。
    #[must_use]
    pub const fn mode_label(&self) -> &'static str {
        if self.is_cloud() { "云端" } else { "本机" }
    }
}

/// 凭据来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CredentialSource {
    /// 读配置相邻 `.env`。绝不迁入凭据库：daemon 自己也在读该文件。
    LocalEnvFile,
    /// 系统凭据库，键为档案 id。
    Keychain {
        /// 凭据库条目的键。
        #[serde(rename = "keyId")]
        key_id: String,
    },
    /// 凭据库不可用时的受限模式：仅本次会话保留，明确不落盘。
    SessionOnly,
}

impl Default for CredentialSource {
    /// 缺省读配置相邻 `.env`：本机档案的凭据就是 daemon 用的那一把。
    fn default() -> Self {
        Self::LocalEnvFile
    }
}

impl CredentialSource {
    /// 凭据是否需要用户重新输入才能连接。
    #[must_use]
    pub const fn needs_user_input(&self) -> bool {
        matches!(self, Self::SessionOnly)
    }

    /// 凭据库条目键；非 `Keychain` 来源为 None。
    #[must_use]
    pub fn keychain_key(&self) -> Option<&str> {
        match self {
            Self::Keychain { key_id } => Some(key_id.as_str()),
            Self::LocalEnvFile | Self::SessionOnly => None,
        }
    }
}

/// 一个连接档案。
///
/// `caps` 是**运行态**，随连接重取，不落盘（`serde(skip)`）；落盘的是 `label`、
/// `spec` 与 `credential`，其中不含任何凭据明文。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendProfile {
    /// 用户可读名称。
    pub label: String,
    /// 端点规格。
    pub spec: EndpointSpec,
    /// 凭据来源。
    pub credential: CredentialSource,
    /// 能力位；仅内存。
    #[serde(skip)]
    pub caps: CapsState,
}

impl BackendProfile {
    /// 本机档案。
    #[must_use]
    pub fn local(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            spec: EndpointSpec::Local,
            credential: CredentialSource::LocalEnvFile,
            caps: CapsState::Unknown,
        }
    }

    /// 云端档案。
    #[must_use]
    pub fn cloud(
        label: impl Into<String>,
        http_base: impl Into<String>,
        ws_base: impl Into<String>,
        credential: CredentialSource,
    ) -> Self {
        Self {
            label: label.into(),
            spec: EndpointSpec::Cloud {
                http_base: http_base.into(),
                ws_base: ws_base.into(),
            },
            credential,
            caps: CapsState::Unknown,
        }
    }

    /// 是否为云端档案。
    #[must_use]
    pub const fn is_cloud(&self) -> bool {
        self.spec.is_cloud()
    }

    /// 按模式覆盖后的能力位。
    #[must_use]
    pub fn effective_caps(&self) -> CapsState {
        match self.caps {
            CapsState::Unknown => CapsState::Unknown,
            CapsState::Known(known) => CapsState::Known(KnownCaps {
                set: known.set.with_mode_override(self.is_cloud()),
                backend_outdated: known.backend_outdated,
            }),
        }
    }
}

/// 携带档案 id 的档案记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEntry {
    /// 档案 id，不可变；凭据库键与状态分区都以它为键。
    pub id: String,
    /// 档案本体。
    pub profile: BackendProfile,
}

impl ProfileEntry {
    /// 组装一条记录。
    #[must_use]
    pub fn new(id: impl Into<String>, profile: BackendProfile) -> Self {
        Self {
            id: id.into(),
            profile,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackendProfile, CapabilitySet, CapsState, CredentialSource, EndpointSpec, KnownCaps,
        ProfileEntry,
    };

    const FULL: &str = r#"{"files":true,"dirs":true,"attachments":true,"config":true,
        "secrets":true,"providers":true,"mcp":true,"skills":true,"documents":true,
        "system_status":true,"service_control":true,"terminal":true}"#;

    fn parsed() -> CapabilitySet {
        let value: serde_json::Value = serde_json::from_str(FULL).expect("样例应为合法 JSON");
        CapabilitySet::from_meta_json(&value).expect("12 键齐全应解析成功")
    }

    #[test]
    fn capability_set_requires_exactly_the_twelve_registered_keys() {
        let missing: serde_json::Value = serde_json::json!({"files": true});
        let error = CapabilitySet::from_meta_json(&missing).expect_err("缺键必须失败");
        assert!(error.contains("缺少"), "{error}");

        let extra: serde_json::Value = serde_json::json!({
            "files": true, "dirs": true, "attachments": true, "config": true,
            "secrets": true, "providers": true, "mcp": true, "skills": true,
            "documents": true, "system_status": true, "service_control": true,
            "terminal": true, "cli": true
        });
        let error = CapabilitySet::from_meta_json(&extra).expect_err("多键必须失败");
        assert!(error.contains("未登记"), "{error}");

        assert!(parsed().terminal);
    }

    #[test]
    fn cloud_profiles_never_expose_local_only_capabilities() {
        let cloud = BackendProfile::cloud(
            "公司云",
            "https://axon.example.com",
            "wss://axon.example.com",
            CredentialSource::Keychain {
                key_id: "k1".to_owned(),
            },
        );
        let mut cloud = cloud;
        cloud.caps = CapsState::Known(KnownCaps {
            set: parsed(),
            backend_outdated: false,
        });
        let effective = cloud.effective_caps().set().expect("已知能力位");
        assert!(!effective.terminal, "云档案的终端必须被覆盖为 false");
        assert!(
            !effective.service_control,
            "云档案的服务控制必须被覆盖为 false"
        );
        assert!(effective.files, "业务能力位不受覆盖影响");

        let mut local = BackendProfile::local("本机");
        local.caps = cloud.caps;
        let effective = local.effective_caps().set().expect("已知能力位");
        assert!(effective.terminal);
        assert!(effective.service_control);
    }

    #[test]
    fn unknown_caps_show_entries_instead_of_reporting_outdated() {
        let unknown = CapsState::Unknown;
        assert!(unknown.set().is_none());
        assert!(!unknown.backend_outdated(), "未探测不得被判为版本过旧");
        assert!(!unknown.hides(false), "未探测时入口可见但置灰，不隐藏");

        let known_false = CapsState::Known(KnownCaps {
            set: CapabilitySet::default(),
            backend_outdated: true,
        });
        assert!(known_false.hides(false), "已知为 false 时应隐藏");
        assert!(known_false.backend_outdated());
    }

    #[test]
    fn profile_serde_matches_the_persisted_shape() {
        // 落盘形态：spec 与 credential 都是内部标记枚举，键名 camelCase。
        let json = serde_json::json!({
            "label": "公司云",
            "spec": {"kind": "cloud", "httpBase": "https://a.example.com", "wsBase": "wss://a.example.com"},
            "credential": {"kind": "keychain", "keyId": "3f2b"}
        });
        let profile: BackendProfile = serde_json::from_value(json).expect("应可反序列化");
        assert!(profile.is_cloud());
        assert_eq!(profile.credential.keychain_key(), Some("3f2b"));
        assert_eq!(
            profile.caps,
            CapsState::Unknown,
            "caps 不落盘，反序列化后必为未知"
        );

        let back = serde_json::to_value(&profile).expect("应可序列化");
        assert!(back.get("caps").is_none(), "caps 不得出现在落盘文本里");

        let local = BackendProfile::local("本机");
        let back = serde_json::to_value(&local).expect("应可序列化");
        assert_eq!(back["spec"]["kind"], "local");
        assert_eq!(back["credential"]["kind"], "localEnvFile");
    }

    #[test]
    fn endpoint_spec_reports_mode_labels() {
        assert_eq!(EndpointSpec::Local.mode_label(), "本机");
        assert!(!EndpointSpec::Local.is_cloud());
        let entry = ProfileEntry::new(
            "local",
            BackendProfile::cloud("云", "https://a", "wss://a", CredentialSource::SessionOnly),
        );
        assert_eq!(entry.id, "local");
        assert!(entry.profile.credential.needs_user_input());
    }
}
