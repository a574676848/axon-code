//! 连接读数的展示形态：把运行态翻译成界面直接可渲染的结构。
//!
//! 这一层只做形状与文案，不做判定：健康态、能力位与探测结论都由
//! [`super::Runtime`] 与 [`super::probe`] 给出，这里只按合同 §5.3 与 §4.5 的
//! 呈现规则组装，避免界面各自推导一套口径。

use serde::Serialize;

use super::endpoint;
use super::probe::{self, ProbeFailure, ProbeOutcome};
use super::profile::{CapabilitySet, CapsState, CredentialSource, EndpointSpec};
use super::runtime;
use super::secret_store::{self, SecretStoreStatus};

/// 一条能力位的展示形态。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityView {
    /// 能力位名（12 个名字之一）。
    pub key: String,
    /// 中文标签。
    pub label: String,
    /// 覆盖后的取值。
    pub enabled: bool,
    /// 是否应当隐藏：只有「已知且为 false」才隐藏。
    pub hidden: bool,
}

/// 健康结论的展示形态。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    /// 状态点档位：`idle` / `ongoing` / `done` / `error` / `warning`。
    pub dot: String,
    /// 中文状态词。
    pub label: String,
    /// 一句可执行的说明。
    pub detail: String,
}

/// 档案清单的一条。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    /// 档案 id。
    pub id: String,
    /// 档案名。
    pub label: String,
    /// 模式：本机 / 云端。
    pub mode: String,
    /// 后端地址。
    pub endpoint: String,
    /// 是否为生效档案。
    pub active: bool,
    /// 凭据来源的可读说明。
    pub credential: String,
    /// 连通结论；惰性探测，未探测为「未探测」。
    pub health_label: String,
    /// 该结论对应的状态点档位。
    pub health_dot: String,
}

/// 连接快照：界面渲染连接分区与胶囊的唯一输入。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSnapshot {
    /// 生效档案 id。
    pub active_id: Option<String>,
    /// 生效档案名。
    pub active_label: String,
    /// 模式。
    pub mode: String,
    /// 是否云端档案。
    pub is_cloud: bool,
    /// 后端地址。
    pub endpoint: String,
    /// 事件流地址。两个协议面是两条独立监听，因此它可能与 HTTP 地址不同主机或不同端口。
    pub ws_endpoint: String,
    /// 是否只读运行。
    pub read_only: bool,
    /// 已完成引导版本；0 表示尚未完成。
    pub onboarding_version: u32,
    /// 是否需要走首开引导。
    pub needs_onboarding: bool,
    /// 启动期一次性提示。
    pub notice: Option<String>,
    /// 健康结论。
    pub health: HealthView,
    /// 后端构建版本；未知为 None。
    pub backend_version: Option<String>,
    /// 延迟（毫秒）；未知为 None。
    pub latency_ms: Option<u64>,
    /// 与本机的时钟偏移（毫秒）；未校验为 None。
    pub clock_offset_ms: Option<i64>,
    /// 时钟偏移是否已交叉校验。
    pub clock_verified: bool,
    /// 能力位是否已知。
    pub caps_known: bool,
    /// 覆盖后的能力位清单。
    pub capabilities: Vec<CapabilityView>,
    /// 本次凭据是否超管。
    pub super_admin: bool,
    /// 上传文件本身上限。
    pub upload_limit_bytes: u64,
    /// 当前地址是否为明文 http（界面要常驻警示条）。
    pub plaintext_private: bool,
    /// 系统凭据库是否可用。
    pub secret_store_available: bool,
    /// 凭据库不可用的原因。
    pub secret_store_reason: Option<String>,
    /// 档案清单。
    pub profiles: Vec<ProfileView>,
    /// 是否渲染密钥管理卡片。
    pub key_management_visible: bool,
}

/// 试连接的回执。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestResult {
    /// 结论：`ok` / `credential` / `certificate` / `unreachable` / `address` / `protocol`。
    pub conclusion: String,
    /// 一句中文说明。
    pub message: String,
    /// 规范化后的后端地址；`address` 结论时为 None。
    pub endpoint: Option<String>,
    /// 事件流地址。
    pub ws_endpoint: Option<String>,
    /// 事件流端口的探测结论：`ok` / `wrongPort` / `credential` / `unreachable` / `notTested`。
    pub ws_conclusion: String,
    /// 事件流端口结论的中文说明。
    pub ws_message: String,
    /// 后端构建版本。
    pub backend_version: Option<String>,
    /// 延迟（毫秒）。
    pub latency_ms: Option<u64>,
    /// 覆盖后的能力位清单。
    pub capabilities: Vec<CapabilityView>,
    /// 证书结论：`trusted` / `invalid` / `unknown`。
    pub certificate: String,
    /// 是否为明文 http 地址。
    pub plaintext_private: bool,
}

/// 能力位的 12 个中文标签。
const CAPABILITY_LABELS: [(&str, &str); 12] = [
    ("files", "文件列表"),
    ("dirs", "目录浏览"),
    ("attachments", "附件上传"),
    ("config", "配置读写"),
    ("secrets", "密钥状态"),
    ("providers", "模型发现"),
    ("mcp", "MCP 配置"),
    ("skills", "技能清单"),
    ("documents", "人设与准则"),
    ("system_status", "运行状态"),
    ("service_control", "后台服务控制"),
    ("terminal", "终端"),
];

/// 组装快照。
#[must_use]
pub fn snapshot() -> ConnectionSnapshot {
    let active = runtime().active();
    let caps = runtime().effective_caps();
    let last_probe = runtime().last_probe();
    let notice = runtime().notice();
    let onboarding_version = runtime().onboarding_version();
    let profiles = runtime().profiles();
    let health = runtime().health();
    let active_id = active.as_ref().map(|entry| entry.id.clone());
    let plaintext_private = active.as_ref().is_some_and(plaintext_profile);
    let secret_status = secret_store::status();

    let (mode, endpoint_text, ws_endpoint_text) = match active.as_ref() {
        Some(entry) => {
            let resolved = endpoint::resolve(&entry.profile.spec);
            (
                entry.profile.spec.mode_label().to_owned(),
                resolved.http_base,
                resolved.ws_base,
            )
        }
        None => {
            let resolved = endpoint::local_endpoints();
            ("未连接".to_owned(), resolved.http_base, resolved.ws_base)
        }
    };

    ConnectionSnapshot {
        active_id: active_id.clone(),
        active_label: active
            .as_ref()
            .map(|entry| super::display_label(&entry.profile))
            .unwrap_or_else(|| "未连接".to_owned()),
        mode,
        is_cloud: active
            .as_ref()
            .is_some_and(|entry| entry.profile.is_cloud()),
        endpoint: endpoint_text,
        ws_endpoint: ws_endpoint_text,
        read_only: runtime().is_read_only(),
        onboarding_version,
        needs_onboarding: onboarding_version == 0,
        notice,
        health: HealthView {
            dot: dot_name(health.dot).to_owned(),
            label: health.label,
            detail: health.detail,
        },
        backend_version: last_probe
            .as_ref()
            .and_then(|ready| ready.build_version.clone()),
        latency_ms: last_probe.as_ref().map(|ready| ready.latency_ms),
        clock_offset_ms: last_probe.as_ref().map(|ready| ready.clock_offset_ms),
        clock_verified: last_probe
            .as_ref()
            .is_some_and(|ready| ready.clock_verified),
        caps_known: caps.set().is_some(),
        capabilities: capability_views(caps),
        super_admin: last_probe
            .as_ref()
            .is_some_and(|ready| ready.meta.super_admin),
        upload_limit_bytes: last_probe
            .as_ref()
            .map_or(probe::FALLBACK_UPLOAD_LIMIT_BYTES, |ready| {
                ready.meta.upload_limit_bytes
            }),
        plaintext_private,
        secret_store_available: secret_status.is_available(),
        secret_store_reason: match secret_status {
            SecretStoreStatus::Available => None,
            SecretStoreStatus::Unavailable { reason } => Some(reason),
        },
        profiles: profiles
            .iter()
            .map(|entry| {
                let is_active = active_id.as_deref() == Some(entry.id.as_str());
                ProfileView {
                    id: entry.id.clone(),
                    label: super::display_label(&entry.profile),
                    mode: entry.profile.spec.mode_label().to_owned(),
                    endpoint: endpoint::resolve(&entry.profile.spec).http_base,
                    active: is_active,
                    credential: credential_label(&entry.profile.credential),
                    health_label: if is_active {
                        runtime().health().label
                    } else {
                        "未探测".to_owned()
                    },
                    health_dot: if is_active {
                        dot_name(runtime().health().dot).to_owned()
                    } else {
                        "idle".to_owned()
                    },
                }
            })
            .collect(),
        key_management_visible: active.is_some()
            && last_probe
                .as_ref()
                .is_some_and(|ready| ready.meta.super_admin),
    }
}

/// 档案是否使用明文地址。
///
/// 口径与 [`crate::connection::endpoint::normalize_cloud_address`] 的
/// `plaintext_private` 一致：只看协议是否为 `http`，不看主机是不是私网。
/// 程序不做网络层限制，这里只用于给一条「凭据将以明文过网」的信息性提示。
fn plaintext_profile(entry: &super::ProfileEntry) -> bool {
    match &entry.profile.spec {
        EndpointSpec::Local => true,
        EndpointSpec::Cloud { http_base, .. } => http_base.starts_with("http://"),
    }
}

/// 凭据来源的中文说明。
fn credential_label(source: &CredentialSource) -> String {
    match source {
        CredentialSource::LocalEnvFile => "配置相邻 .env".to_owned(),
        CredentialSource::Keychain { .. } => "系统凭据库".to_owned(),
        CredentialSource::SessionOnly => "本次会话有效".to_owned(),
    }
}

/// 状态点档位的对外名字。
const fn dot_name(dot: super::HealthDot) -> &'static str {
    match dot {
        super::HealthDot::Idle => "idle",
        super::HealthDot::Ongoing => "ongoing",
        super::HealthDot::Done => "done",
        super::HealthDot::Error => "error",
        super::HealthDot::Warning => "warning",
    }
}

/// 能力位清单：已知按覆盖后取值，未知则全部可见但置灰。
#[must_use]
pub fn capability_views(caps: CapsState) -> Vec<CapabilityView> {
    CAPABILITY_LABELS
        .iter()
        .map(|(key, label)| {
            let known = caps.set();
            let enabled = known.is_some_and(|set| value_of(&set, key));
            CapabilityView {
                key: (*key).to_owned(),
                label: (*label).to_owned(),
                enabled,
                // 未知时不隐藏：入口可见但置灰，点击给「未连接」。
                hidden: known.is_some() && !enabled,
            }
        })
        .collect()
}

/// 按能力位名取值。
fn value_of(set: &CapabilitySet, key: &str) -> bool {
    set.entries()
        .into_iter()
        .find(|(name, _)| *name == key)
        .is_some_and(|(_, value)| value)
}

/// 组装一次试连接的结论。
#[must_use]
pub fn build_test_result(
    http_base: &str,
    ws_base: &str,
    plaintext_private: bool,
    outcome: &ProbeOutcome,
    ws: &WsProbeView,
) -> ConnectionTestResult {
    let endpoint_text = Some(http_base.to_owned());
    let ws_text = Some(ws_base.to_owned());
    let (ws_conclusion, ws_message) = (ws.conclusion.to_owned(), ws.message.to_owned());
    match outcome {
        ProbeOutcome::Ready(ready) => ConnectionTestResult {
            conclusion: "ok".to_owned(),
            message: format!("连接成功，延迟 {} 毫秒", ready.latency_ms),
            endpoint: endpoint_text,
            ws_endpoint: ws_text,
            ws_conclusion,
            ws_message,
            backend_version: ready.build_version.clone(),
            latency_ms: Some(ready.latency_ms),
            capabilities: capability_views(CapsState::Known(super::profile::KnownCaps {
                set: ready.meta.capabilities.with_mode_override(true),
            })),
            certificate: "trusted".to_owned(),
            plaintext_private,
        },
        ProbeOutcome::Failed(failure) => ConnectionTestResult {
            conclusion: conclusion_name(failure).to_owned(),
            message: failure.message(),
            endpoint: endpoint_text,
            ws_endpoint: ws_text,
            ws_conclusion,
            ws_message,
            backend_version: None,
            latency_ms: None,
            capabilities: Vec::new(),
            certificate: if matches!(failure, ProbeFailure::CertificateInvalid(_)) {
                "invalid".to_owned()
            } else {
                "unknown".to_owned()
            },
            plaintext_private,
        },
    }
}

/// 事件流端口探测的展示形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsProbeView {
    /// 结论名：`ok` / `wrongPort` / `credential` / `unreachable` / `notTested`。
    pub conclusion: &'static str,
    /// 一句可执行的中文说明。
    pub message: String,
}

/// 把事件流端口探测结果翻译成可执行的结论。
#[must_use]
pub fn ws_probe_view(outcome: &super::probe::WsProbeOutcome) -> WsProbeView {
    match outcome {
        super::probe::WsProbeOutcome::Ready => WsProbeView {
            conclusion: "ok",
            message: "事件流端口正常".to_owned(),
        },
        super::probe::WsProbeOutcome::WrongPort => WsProbeView {
            conclusion: "wrongPort",
            message:
                "事件流端口不对：该地址上没有 /gateway/v1/ws，后端也没有自报 webSocketPort。请确认后端版本是否支持 /api/v1/system/status，以及 remote.gateway.websocket_bind 是否已配置"
                    .to_owned(),
        },
        super::probe::WsProbeOutcome::CredentialRejected { status } => WsProbeView {
            conclusion: "credential",
            message: format!("事件流端口拒绝了这把凭据（HTTP {status}）"),
        },
        super::probe::WsProbeOutcome::Unexpected { status } => WsProbeView {
            conclusion: "unreachable",
            message: format!(
                "事件流端口返回了 HTTP {status}：既不是升级拒绝，也不是 404，无法确认这里是不是 WebSocket 监听。请核对 remote.gateway.websocket_bind 是否为此端口，以及中间层是否改写了响应"
            ),
        },
        super::probe::WsProbeOutcome::Unreachable(reason) => WsProbeView {
            conclusion: "unreachable",
            message: format!("事件流端口连不上：{reason}"),
        },
    }
}

/// HTTP 都没通时，事件流不单独探测。
#[must_use]
pub fn ws_probe_not_tested() -> WsProbeView {
    WsProbeView {
        conclusion: "notTested",
        message: "HTTP 未连通，本次没有探测事件流端口".to_owned(),
    }
}

/// 失败结论的对外名字。
const fn conclusion_name(failure: &ProbeFailure) -> &'static str {
    match failure {
        ProbeFailure::CredentialRejected { .. } => "credential",
        ProbeFailure::CertificateInvalid(_) => "certificate",
        ProbeFailure::ProtocolUnsupported(_) => "protocol",
        ProbeFailure::Unreachable(_) | ProbeFailure::BodyInvalid(_) => "unreachable",
    }
}
