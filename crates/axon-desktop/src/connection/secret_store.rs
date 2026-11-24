//! 云档案凭据的存储与「试连接期临时凭据」的生命周期。
//!
//! 两条硬规则：
//!
//! 1. 凭据明文**永不落盘**：要么在系统凭据库，要么只在本次会话的内存里。
//! 2. 试连接阶段用的那把 Key 只存在于一个会在 [`Drop`] 时清零的对象里，
//!    不写凭据库、不写状态文件、不进日志、不进错误文案。测试成功后才写凭据库，
//!    写成功之后才落档案；测试失败即丢弃，凭据库里不留脏条目。

use std::fmt;

/// 凭据库服务名：同一条目按「服务名 + 档案 id」定位。
const SERVICE_NAME: &str = "axon-desktop";

/// 凭据库的可用性结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretStoreStatus {
    /// 可读写系统凭据库。
    Available,
    /// 不可用：调用方必须显式降级为 [`crate::connection::profile::CredentialSource::SessionOnly`]。
    Unavailable {
        /// 中文原因，直接可展示给用户。
        reason: String,
    },
}

impl SecretStoreStatus {
    /// 是否可用。
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// 试连接期的一次性凭据。
///
/// 生命周期固定：由输入框构造 → 只用于发第一次带鉴权请求 → 成功则写入凭据库，
/// 失败即随作用域结束被清零。它不实现 `Clone`，也不实现 `Serialize`，
/// 因此无法被顺手塞进全局状态或状态文件。
pub struct PendingCredential {
    /// 明文；`Debug` 输出被遮蔽。
    secret: String,
}

impl PendingCredential {
    /// 由界面输入构造。
    #[must_use]
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into().trim().to_owned(),
        }
    }

    /// 取出明文用于发起一次请求。
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.secret
    }

    /// 明文是否为空（空值不应发起请求）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.secret.is_empty()
    }

    /// 把明文交给凭据库；成功后调用方应立即丢掉本对象。
    ///
    /// # Errors
    ///
    /// 凭据库不可用或写入失败。
    pub fn persist(&self, key_id: &str) -> Result<(), String> {
        store(key_id, &self.secret)
    }
}

impl fmt::Debug for PendingCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PendingCredential(<已隐藏>)")
    }
}

impl Drop for PendingCredential {
    fn drop(&mut self) {
        // 没有 unsafe：把所有权取出来并逐字节清零再释放。
        let mut bytes = std::mem::take(&mut self.secret).into_bytes();
        bytes.fill(0);
        drop(bytes);
    }
}

/// 探测凭据库是否可用。
#[must_use]
pub fn status() -> SecretStoreStatus {
    platform::status()
}

/// 写入一把凭据。
///
/// # Errors
///
/// 凭据库不可用或写入失败；错误文案不含明文。
pub fn store(key_id: &str, secret: &str) -> Result<(), String> {
    platform::store(key_id, secret)
}

/// 读取一把凭据；条目不存在时返回 `Ok(None)`。
///
/// # Errors
///
/// 凭据库不可用或读取失败。
pub fn load(key_id: &str) -> Result<Option<String>, String> {
    platform::load(key_id)
}

/// 删除一把凭据；条目本就不存在时视为成功。
///
/// # Errors
///
/// 凭据库不可用或删除失败。
pub fn delete(key_id: &str) -> Result<(), String> {
    platform::delete(key_id)
}

/// Windows、macOS 与 Linux 走系统凭据库。
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod platform {
    use super::{SERVICE_NAME, SecretStoreStatus};

    fn entry(key_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE_NAME, key_id)
            .map_err(|error| format!("无法打开系统凭据库：{error}"))
    }

    pub(super) fn status() -> SecretStoreStatus {
        // 用一个不存在的探针键试读：能给出「条目不存在」就说明凭据库本身可用。
        match entry("__axon_probe__") {
            Ok(probe) => match probe.get_password() {
                Ok(_) | Err(keyring::Error::NoEntry) => SecretStoreStatus::Available,
                Err(error) => SecretStoreStatus::Unavailable {
                    reason: format!("系统凭据库暂不可用：{error}"),
                },
            },
            Err(reason) => SecretStoreStatus::Unavailable { reason },
        }
    }

    pub(super) fn store(key_id: &str, secret: &str) -> Result<(), String> {
        entry(key_id)?
            .set_password(secret)
            .map_err(|error| format!("写入系统凭据库失败：{error}"))
    }

    pub(super) fn load(key_id: &str) -> Result<Option<String>, String> {
        match entry(key_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("读取系统凭据库失败：{error}")),
        }
    }

    pub(super) fn delete(key_id: &str) -> Result<(), String> {
        match entry(key_id)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("删除系统凭据库条目失败：{error}")),
        }
    }
}

/// 其余平台按合同降级：不落明文，明确要求每次启动重新输入。
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod platform {
    use super::SecretStoreStatus;

    /// 降级原因：同一句话供界面与错误文案共用。
    const REASON: &str =
        "当前平台未接入系统凭据库（缺少 Secret Service），云档案凭据只在本次会话内保留";

    pub(super) fn status() -> SecretStoreStatus {
        SecretStoreStatus::Unavailable {
            reason: REASON.to_owned(),
        }
    }

    pub(super) fn store(_key_id: &str, _secret: &str) -> Result<(), String> {
        Err(REASON.to_owned())
    }

    pub(super) fn load(_key_id: &str) -> Result<Option<String>, String> {
        Err(REASON.to_owned())
    }

    pub(super) fn delete(_key_id: &str) -> Result<(), String> {
        Err(REASON.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{PendingCredential, SecretStoreStatus};

    #[test]
    fn pending_credential_hides_its_secret_from_debug_output() {
        let pending = PendingCredential::new("  sk-live-1234567890  ");
        assert_eq!(pending.expose(), "sk-live-1234567890");
        let rendered = format!("{pending:?}");
        assert!(!rendered.contains("sk-live"), "{rendered}");
        assert!(rendered.contains("已隐藏"), "{rendered}");
        assert!(!pending.is_empty());
    }

    #[test]
    fn empty_input_is_reported_as_empty_instead_of_reaching_the_network() {
        assert!(PendingCredential::new("   ").is_empty());
    }

    #[test]
    fn status_helper_reports_availability_in_the_boolean_form() {
        // 具体可用性随平台而定：本用例只断言两种取值互相自洽。
        let status = super::status();
        assert_eq!(
            status.is_available(),
            matches!(status, SecretStoreStatus::Available)
        );
        if let SecretStoreStatus::Unavailable { reason } = status {
            assert!(!reason.is_empty(), "降级必须给出可展示的中文原因");
        }
    }
}
