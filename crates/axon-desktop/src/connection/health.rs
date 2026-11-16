//! 连接健康结论：一档状态词、一句可执行说明，以及「探活结论到档位」的唯一映射。
//!
//! 从运行态里拆出来，是因为这一段是纯判定：不碰锁、不碰状态文件、不碰网络。
//! 判定必须能离线逐档验证，尤其是「网关通了但后台租约失效」这一档：它曾经被
//! 当成「已连接」，界面上于是给出一个绿点和一个很好看的延迟。

use super::probe;

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
    /// 凭据缺失、后台需升级、后台已停机。
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
}

/// 把一次探活结论翻成健康结论。
///
/// 租约失效必须与「已连接」分开报：网关应答只证明进程在跑，租约失效说明后台 Worker
/// 已全部停下、会话不会被领取。此时延迟再低也没有意义，绿色等于替一个不干活的进程背书。
pub(super) fn health_from_probe(outcome: &probe::ProbeOutcome) -> HealthState {
    match outcome {
        probe::ProbeOutcome::Ready(ready) if ready.lease_active == Some(false) => HealthState {
            dot: HealthDot::Warning,
            label: "后台已停机".to_owned(),
            detail: "网关有响应，但后台租约已失效：新会话不会被领取，重启后台服务即可恢复"
                .to_owned(),
        },
        probe::ProbeOutcome::Ready(ready) => HealthState {
            dot: HealthDot::Done,
            label: "已连接".to_owned(),
            detail: format!("延迟 {} 毫秒", ready.latency_ms),
        },
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
    }
}
