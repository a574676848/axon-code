//! Gateway 多 Key 存储：只存哈希、可热更新、逐次审计。
//!
//! 三条不可退让的口径：
//!
//! 1. 明文只在 [`GatewayKeyStore::create`] 的回执里出现一次。落盘内容只有每把 Key 独立的盐
//!    与 PBKDF2-HMAC-SHA256 摘要，日志、错误文案与 `Debug` 输出里都不出现明文与摘要。
//! 2. 明文自带 Key 的 `id`。`id` 与 `label` 不是秘密，却能把一次校验收敛为一次派生；否则
//!    每来一个请求都要对整表做 PBKDF2。
//! 3. 校验命中后的 `last_used_at` 不在请求路径上落盘：命中时只改内存并置脏标记，之后最多
//!    每 [`LAST_USED_FLUSH_INTERVAL_MS`] 毫秒批量写回一次，拿不到写锁就直接跳过本次落盘。
//!    取舍：进程在两次落盘之间被杀会丢掉最后若干次使用时间，换来鉴权路径不被磁盘 IO
//!    拖慢。`created_at` 与 `revoked_at` 属于安全语义，一律立即落盘。
//!
//! 文件权限：Unix 上创建即 0600，读盘时再兜底收紧一次。Windows 没有 `std` 层的等价位
//! （ACL 不在 `std` 能力面内），该平台的差异在注释与交付说明里如实登记，不做假装。

#[path = "gateway_keys_primitives.rs"]
mod gateway_keys_primitives;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use gateway_keys_primitives::{
    base64_decode, base64_encode, base64_encode_url, enforce_private_permissions, fingerprint,
    now_ms, open_append_private, pbkdf2_hmac_sha256, resolve_store_path, write_private_file,
};
use serde::{Deserialize, Serialize};

/// 存储文件相对运行时根的默认位置。
pub const DEFAULT_KEY_STORE_RELATIVE_PATH: &str = "data/gateway/auth-keys.json";
/// Key 审计日志相对运行时根的位置。
const AUDIT_LOG_RELATIVE_PATH: &str = "data/logs/gateway-keys-audit.jsonl";
/// 存储格式版本。
const STORE_VERSION: u32 = 1;
/// 摘要算法标识；固定字符串，读盘时按它判定该条记录是否可用。
pub const KEY_HASH_ALGO: &str = "pbkdf2-hmac-sha256";
/// PBKDF2 迭代次数。
pub const KEY_HASH_ITERATIONS: u32 = 200_000;
/// 可信迭代次数的上界：防止被篡改的存储文件把一次鉴权拖成 CPU 事故。
const MAX_TRUSTED_ITERATIONS: u32 = 10_000_000;
/// 每把 Key 独立盐的字节数。
const SALT_BYTES: usize = 16;
/// 明文秘密部分的字节数。
const SECRET_BYTES: usize = 32;
/// 摘要输出字节数。
const HASH_BYTES: usize = 32;
/// 明文前缀；后面依次是 Key 的 `id` 与秘密部分，用下划线分隔。
const PLAINTEXT_PREFIX: &str = "axon_key_";
/// 指纹长度（十六进制字符数）。
const FINGERPRINT_CHARS: usize = 6;
/// `last_used_at` 批量落盘的最小间隔（毫秒）。
const LAST_USED_FLUSH_INTERVAL_MS: u64 = 60_000;
/// 审计日志里 CLI 场景的来源名。
const AUDIT_SOURCE_CLI: &str = "cli";
/// 审计日志里超管的身份名。
const AUDIT_ACTOR_SUPER_ADMIN: &str = "superadmin";

/// 多 Key 存储错误。
///
/// 错误文案只带 `id`、`label` 与路径，绝不带明文与摘要。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GatewayKeyError {
    /// 密钥存储的目录或文件不可用。
    #[error("Key 存储不可用：{0}")]
    Storage(String),
    /// 存储文件内容不是可识别的格式。
    #[error("Key 存储文件格式无效")]
    Malformed,
    /// 标签为空。
    #[error("Key 标签不能为空")]
    EmptyLabel,
    /// 指定的 Key 不存在。
    #[error("未找到指定的 Key：{0}")]
    UnknownKey(String),
    /// 指定的 Key 已吊销。
    #[error("Key 已被吊销：{0}")]
    AlreadyRevoked(String),
    /// 随机源或派生参数不可用。
    #[error("Key 派生参数不可用")]
    Derivation,
    /// 审计日志无法追加。
    #[error("Key 审计日志写入失败：{0}")]
    Audit(String),
}

/// 审计用的发起方信息；`source` 区分 CLI 与 HTTP，`actor` 是发起方身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyOrigin {
    /// 发起来源，例如 `cli` 与 `http`。
    pub source: String,
    /// 发起方身份，例如 `superadmin`。
    pub actor: String,
    /// 对端地址（尽力而为）：直连时取传输层来源，经反代时取转发头；
    /// 两者都拿不到就是 `None`，审计行写 `null`，绝不伪造一个地址。
    pub peer: Option<String>,
}

impl KeyOrigin {
    /// CLI 场景：来源 `cli`，发起方是超管，没有对端地址。
    #[must_use]
    pub fn cli() -> Self {
        Self {
            source: AUDIT_SOURCE_CLI.to_owned(),
            actor: AUDIT_ACTOR_SUPER_ADMIN.to_owned(),
            peer: None,
        }
    }

    /// HTTP 场景：来源与发起方由调用方给出；对端地址可为空。
    #[must_use]
    pub fn http(actor: &str, peer: Option<&str>) -> Self {
        Self {
            source: "http".to_owned(),
            actor: actor.to_owned(),
            peer: peer.map(str::to_owned),
        }
    }
}

/// 签发结果。
///
/// 不实现 `Debug`：这里带着明文，任何格式化输出都等于把明文写进日志。
pub struct IssuedKey {
    /// Key 标识（不是秘密）。
    pub id: String,
    /// Key 标签（不是秘密）。
    pub label: String,
    /// 明文；只在本次回执里出现一次。
    pub plaintext: String,
}

/// 列表项：只回标签、时间与指纹前 6 位，不回摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySummary {
    /// Key 标识。
    pub id: String,
    /// Key 标签。
    pub label: String,
    /// 创建时间（毫秒）。
    pub created_at_ms: u64,
    /// 最后一次用于鉴权的时间（毫秒）；从未使用为 `None`。
    pub last_used_at_ms: Option<u64>,
    /// 指纹前 6 位十六进制字符。
    pub fingerprint: String,
}

/// 校验命中的 Key 身份；不含明文与摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyIdentity {
    /// Key 标识。
    pub id: String,
    /// Key 标签。
    pub label: String,
}

/// 一条 Key 记录；字段名按 `camelCase` 落盘。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyRecord {
    /// Key 标识。
    id: String,
    /// Key 标签。
    label: String,
    /// 本条 Key 独立的盐（base64）。
    salt: String,
    /// PBKDF2 摘要（base64）。
    hash: String,
    /// 摘要算法标识。
    hash_algo: String,
    /// 派生迭代次数。
    iterations: u32,
    /// 创建时间（毫秒）。
    created_at_ms: u64,
    /// 最后一次用于鉴权的时间（毫秒）。
    #[serde(default)]
    last_used_at_ms: Option<u64>,
    /// 吊销时间（毫秒）；吊销后不再参与鉴权。
    #[serde(default)]
    revoked_at_ms: Option<u64>,
}

/// 存储文件。
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyStoreFile {
    version: u32,
    #[serde(default)]
    keys: Vec<KeyRecord>,
}

/// 内存中的 Key 表。
struct KeyStore {
    records: Vec<KeyRecord>,
}

impl KeyStore {
    /// 空表。
    fn empty() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// 可落盘的快照。
    fn snapshot(&self) -> KeyStoreFile {
        KeyStoreFile {
            version: STORE_VERSION,
            keys: self.records.clone(),
        }
    }

    /// 按 `id` 找未吊销记录的副本。
    fn active_record(&self, id: &str) -> Option<KeyRecord> {
        self.records
            .iter()
            .find(|record| record.id == id && record.revoked_at_ms.is_none())
            .cloned()
    }
}

/// 多 Key 存储句柄：热更新、可跨线程共享。
///
/// 内部 `Arc<RwLock<KeyStore>>` 之外只有两个原子量：脏标记与上次落盘时间。两者合起来就是
/// 「有界地批量写 `last_used_at`」的全部状态，读鉴权路径不需要任何额外锁。
pub struct GatewayKeyStore {
    path: PathBuf,
    audit_path: PathBuf,
    inner: RwLock<KeyStore>,
    last_used_dirty: AtomicBool,
    last_flush_ms: AtomicU64,
}

impl GatewayKeyStore {
    /// 打开（必要时视为空库）Key 存储。
    ///
    /// `runtime_root` 是后端运行时根；`configured_path` 是配置键
    /// `remote.gateway.auth_keys` 的取值，空串表示走 [`DEFAULT_KEY_STORE_RELATIVE_PATH`]，
    /// 相对路径一律按运行时根解析。
    pub fn open(runtime_root: &Path, configured_path: &str) -> Result<Arc<Self>, GatewayKeyError> {
        let store = Self {
            path: resolve_store_path(runtime_root, configured_path),
            audit_path: runtime_root.join(AUDIT_LOG_RELATIVE_PATH),
            inner: RwLock::new(KeyStore::empty()),
            last_used_dirty: AtomicBool::new(false),
            last_flush_ms: AtomicU64::new(now_ms()),
        };
        let loaded = store.read_store_file()?;
        *store.write_store() = loaded;
        Ok(Arc::new(store))
    }

    /// 存储文件路径；仅供日志与诊断，不含任何秘密。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 从磁盘重新载入。
    ///
    /// 供进程外改动（运维直接替换文件、另一进程写入）后同步；进程内的 `create`/`revoke`
    /// 直接改内存并立即落盘，不需要重新载入，因此热更新不依赖重启。
    pub fn reload(&self) -> Result<(), GatewayKeyError> {
        let loaded = self.read_store_file()?;
        *self.write_store() = loaded;
        self.last_used_dirty.store(false, Ordering::Release);
        Ok(())
    }

    /// 签发一把 Key；明文只在返回值里出现一次。
    pub fn create(&self, label: &str) -> Result<IssuedKey, GatewayKeyError> {
        self.create_as(label, &KeyOrigin::cli())
    }

    /// 签发一把 Key，并把来源与发起方写进审计。
    ///
    /// 顺序固定为「先审计、后改状态」：审计写不进去就不签发，避免出现「已生效但无审计」。
    pub fn create_as(&self, label: &str, origin: &KeyOrigin) -> Result<IssuedKey, GatewayKeyError> {
        let label = label.trim();
        if label.is_empty() {
            return Err(GatewayKeyError::EmptyLabel);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let salt = axon_crypto::random_bytes::<SALT_BYTES>()
            .map_err(|_error| GatewayKeyError::Derivation)?;
        let secret = axon_crypto::random_bytes::<SECRET_BYTES>()
            .map_err(|_error| GatewayKeyError::Derivation)?;
        let mut hash = [0_u8; HASH_BYTES];
        pbkdf2_hmac_sha256(&secret, &salt, KEY_HASH_ITERATIONS, &mut hash)?;
        let plaintext = format!(
            "{PLAINTEXT_PREFIX}{id}_{}",
            base64_encode_url(secret.as_slice())
        );
        let record = KeyRecord {
            id: id.clone(),
            label: label.to_owned(),
            salt: base64_encode(salt.as_slice()),
            hash: base64_encode(hash.as_slice()),
            hash_algo: KEY_HASH_ALGO.to_owned(),
            iterations: KEY_HASH_ITERATIONS,
            created_at_ms: now_ms(),
            last_used_at_ms: None,
            revoked_at_ms: None,
        };
        self.append_audit("create", &id, label, origin)?;
        let mut store = self.write_store();
        store.records.push(record);
        self.persist(&store)?;
        Ok(IssuedKey {
            id,
            label: label.to_owned(),
            plaintext,
        })
    }

    /// 吊销一把 Key；吊销后立即对下一次鉴权生效，不需要重启。
    pub fn revoke(&self, id: &str) -> Result<(), GatewayKeyError> {
        self.revoke_as(id, &KeyOrigin::cli())
    }

    /// 吊销一把 Key，并把来源与发起方写进审计。
    pub fn revoke_as(&self, id: &str, origin: &KeyOrigin) -> Result<(), GatewayKeyError> {
        let id = id.trim();
        let label = {
            let store = self.read_store();
            let record = store
                .records
                .iter()
                .find(|record| record.id == id)
                .ok_or_else(|| GatewayKeyError::UnknownKey(id.to_owned()))?;
            if record.revoked_at_ms.is_some() {
                return Err(GatewayKeyError::AlreadyRevoked(id.to_owned()));
            }
            record.label.clone()
        };
        self.append_audit("revoke", id, &label, origin)?;
        let mut store = self.write_store();
        let revoked_at = now_ms();
        if let Some(record) = store
            .records
            .iter_mut()
            .find(|record| record.id == id && record.revoked_at_ms.is_none())
        {
            record.revoked_at_ms = Some(revoked_at);
        }
        self.persist(&store)
    }

    /// 列出未吊销的 Key；已吊销记录只保留在文件里，供审计追溯。
    #[must_use]
    pub fn list(&self) -> Vec<KeySummary> {
        let store = self.read_store();
        store
            .records
            .iter()
            .filter(|record| record.revoked_at_ms.is_none())
            .map(|record| KeySummary {
                id: record.id.clone(),
                label: record.label.clone(),
                created_at_ms: record.created_at_ms,
                last_used_at_ms: record.last_used_at_ms,
                fingerprint: fingerprint(&record.id, &record.hash),
            })
            .collect()
    }

    /// 校验明文；命中给出身份，未命中给出 `None`。
    ///
    /// 派生在锁外完成：只在锁内取一次记录副本，避免 20 万次迭代期间挡住 `create`/`revoke`。
    #[must_use]
    pub fn verify(&self, plaintext: &str) -> Option<KeyIdentity> {
        let plaintext = plaintext.trim();
        let (id, encoded_secret) = plaintext.strip_prefix(PLAINTEXT_PREFIX)?.split_once('_')?;
        let (record, salt, hash) = {
            let store = self.read_store();
            let record = store.active_record(id)?;
            let salt = base64_decode(&record.salt)?;
            let hash = base64_decode(&record.hash)?;
            (record, salt, hash)
        };
        if record.hash_algo != KEY_HASH_ALGO
            || hash.is_empty()
            || hash.len() > 64
            || record.iterations == 0
            || record.iterations > MAX_TRUSTED_ITERATIONS
        {
            return None;
        }
        let secret = base64_decode(encoded_secret)?;
        let mut derived = vec![0_u8; hash.len()];
        pbkdf2_hmac_sha256(&secret, &salt, record.iterations, &mut derived).ok()?;
        if !super::constant_time_equal(&derived, &hash) {
            return None;
        }
        self.touch_last_used(id);
        Some(KeyIdentity {
            id: record.id,
            label: record.label,
        })
    }

    /// 命中后只改内存并置脏标记；有界地批量落盘。
    fn touch_last_used(&self, id: &str) {
        let touched_at = now_ms();
        // 用 `try_write` 而不是排队：create/revoke 落盘期间持写锁，鉴权路径不许在这里等。
        if let Ok(mut store) = self.inner.try_write()
            && let Some(record) = store
                .records
                .iter_mut()
                .find(|record| record.id == id && record.revoked_at_ms.is_none())
        {
            record.last_used_at_ms = Some(touched_at);
        }
        self.last_used_dirty.store(true, Ordering::Release);
        self.flush_last_used_if_due();
    }

    /// 距上次落盘超过间隔才批量写一次；拿不到读锁直接跳过，绝不阻塞鉴权。
    fn flush_last_used_if_due(&self) {
        if !self.last_used_dirty.load(Ordering::Acquire) {
            return;
        }
        let now = now_ms();
        if now.saturating_sub(self.last_flush_ms.load(Ordering::Acquire))
            < LAST_USED_FLUSH_INTERVAL_MS
        {
            return;
        }
        let Ok(store) = self.inner.try_read() else {
            return;
        };
        if self.write_store_file(&store).is_ok() {
            self.last_flush_ms.store(now, Ordering::Release);
            self.last_used_dirty.store(false, Ordering::Release);
        }
    }

    /// 立即落盘并清脏标记；调用方必须持有读锁或写锁。
    fn persist(&self, store: &KeyStore) -> Result<(), GatewayKeyError> {
        self.write_store_file(store)?;
        self.last_flush_ms.store(now_ms(), Ordering::Release);
        self.last_used_dirty.store(false, Ordering::Release);
        Ok(())
    }

    /// 原子写存储文件：同目录临时文件加改名，避免半截文件。
    fn write_store_file(&self, store: &KeyStore) -> Result<(), GatewayKeyError> {
        let json = serde_json::to_vec_pretty(&store.snapshot())
            .map_err(|_error| GatewayKeyError::Malformed)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                GatewayKeyError::Storage(format!("{}：{error}", parent.display()))
            })?;
        }
        let temp = self.path.with_extension("json.tmp");
        write_private_file(&temp, &json)?;
        fs::rename(&temp, &self.path).map_err(|error| {
            GatewayKeyError::Storage(format!("{}：{error}", self.path.display()))
        })?;
        enforce_private_permissions(&self.path);
        Ok(())
    }

    /// 读存储文件；文件不存在等同空库。
    fn read_store_file(&self) -> Result<KeyStore, GatewayKeyError> {
        if !self.path.exists() {
            return Ok(KeyStore::empty());
        }
        let mut file = File::open(&self.path).map_err(|error| {
            GatewayKeyError::Storage(format!("{}：{error}", self.path.display()))
        })?;
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(|error| {
            GatewayKeyError::Storage(format!("{}：{error}", self.path.display()))
        })?;
        // 读盘时兜底收紧一次权限：文件可能被别人放宽过。
        enforce_private_permissions(&self.path);
        if text.trim().is_empty() {
            return Ok(KeyStore::empty());
        }
        let file: KeyStoreFile =
            serde_json::from_str(&text).map_err(|_error| GatewayKeyError::Malformed)?;
        if file.version != STORE_VERSION {
            return Err(GatewayKeyError::Malformed);
        }
        Ok(KeyStore { records: file.keys })
    }

    /// 追加一行审计；不写明文也不写摘要。
    fn append_audit(
        &self,
        action: &str,
        id: &str,
        label: &str,
        origin: &KeyOrigin,
    ) -> Result<(), GatewayKeyError> {
        if let Some(parent) = self.audit_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                GatewayKeyError::Audit(format!("{}：{error}", parent.display()))
            })?;
        }
        let line = serde_json::json!({
            "time": chrono::Utc::now().to_rfc3339(),
            "timeMs": now_ms(),
            "action": action,
            "id": id,
            "label": label,
            "source": origin.source,
            "actor": origin.actor,
            // 对端地址尽力而为：拿不到就是 null，不猜、不填占位。
            "peer": origin.peer,
        });
        let mut text = serde_json::to_string(&line)
            .map_err(|_error| GatewayKeyError::Audit("序列化失败".to_owned()))?;
        text.push('\n');
        let mut file = open_append_private(&self.audit_path).map_err(|error| {
            GatewayKeyError::Audit(format!("{}：{error}", self.audit_path.display()))
        })?;
        file.write_all(text.as_bytes()).map_err(|error| {
            GatewayKeyError::Audit(format!("{}：{error}", self.audit_path.display()))
        })?;
        Ok(())
    }

    fn read_store(&self) -> RwLockReadGuard<'_, KeyStore> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_store(&self) -> RwLockWriteGuard<'_, KeyStore> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "gateway_keys_tests.rs"]
mod gateway_keys_tests;
