//! 多 Key 存储的密码学与文件原语：PBKDF2、base64、指纹、私有文件写入与路径解析。
//!
//! 单独成文件是为了守住单文件 700 行的硬限，同时把「算法与文件权限」和「Key 表语义」
//! 分开：本文件不做任何存储状态判断，存储层也不重复实现算法。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256};

use super::{DEFAULT_KEY_STORE_RELATIVE_PATH, FINGERPRINT_CHARS, GatewayKeyError};

/// 标准 base64 字母表。
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// URL 安全 base64 字母表（明文里的秘密部分用它，避免查询串通道被 `+` 破坏）。
const BASE64_URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 解析存储文件路径：空串走默认相对路径，相对路径按运行时根解析。
pub(super) fn resolve_store_path(runtime_root: &Path, configured_path: &str) -> PathBuf {
    let configured = configured_path.trim();
    if configured.is_empty() {
        return runtime_root.join(DEFAULT_KEY_STORE_RELATIVE_PATH);
    }
    let candidate = Path::new(configured);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        runtime_root.join(candidate)
    }
}

/// 现在的时间（毫秒）；系统时间早于纪元时按 0 处理。
pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// 指纹：`SHA-256(id || 0x00 || 摘要文本)` 的前 6 位十六进制字符。
///
/// 再散一次而不直接截断摘要文本：指纹要被界面展示，截断摘要等于泄露摘要前缀。
pub(super) fn fingerprint(id: &str, hash_text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(id.as_bytes());
    hasher.update([0_u8]);
    hasher.update(hash_text.as_bytes());
    let digest = hasher.finalize();
    let mut text = String::with_capacity(FINGERPRINT_CHARS);
    for byte in digest.iter().take(FINGERPRINT_CHARS / 2) {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// PBKDF2-HMAC-SHA256。
///
/// 只实现本项目需要的固定 PRF，不做通用化：`hmac` 与 `sha2` 是唯一新增依赖，SHA-256 由
/// 库实现，本函数只负责 PBKDF2 的块编号、迭代链与多块输出。
pub(super) fn pbkdf2_hmac_sha256(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    output: &mut [u8],
) -> Result<(), GatewayKeyError> {
    if iterations == 0 || output.is_empty() {
        return Err(GatewayKeyError::Derivation);
    }
    let block_len = <Sha256 as sha2::Digest>::output_size();
    let prf =
        Hmac::<Sha256>::new_from_slice(password).map_err(|_error| GatewayKeyError::Derivation)?;
    for (index, chunk) in output.chunks_mut(block_len).enumerate() {
        let block_index = u32::try_from(index).map_err(|_error| GatewayKeyError::Derivation)? + 1;
        let mut message = Vec::with_capacity(salt.len() + 4);
        message.extend_from_slice(salt);
        message.extend_from_slice(&block_index.to_be_bytes());
        let mut mac = prf.clone();
        mac.update(&message);
        let mut previous = mac.finalize().into_bytes().as_slice().to_vec();
        let mut accumulator = previous.clone();
        for _ in 1..iterations {
            let mut mac = prf.clone();
            mac.update(&previous);
            previous = mac.finalize().into_bytes().as_slice().to_vec();
            for (slot, byte) in accumulator.iter_mut().zip(previous.iter()) {
                *slot ^= *byte;
            }
        }
        for (slot, byte) in chunk.iter_mut().zip(accumulator.iter()) {
            *slot = *byte;
        }
    }
    Ok(())
}

/// 标准 base64 编码（带填充）。
pub(super) fn base64_encode(bytes: &[u8]) -> String {
    encode_base64(bytes, BASE64_ALPHABET, true)
}

/// URL 安全 base64 编码（不带填充）。
pub(super) fn base64_encode_url(bytes: &[u8]) -> String {
    encode_base64(bytes, BASE64_URL_ALPHABET, false)
}

fn encode_base64(bytes: &[u8], alphabet: &[u8; 64], padded: bool) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk.first().copied().unwrap_or(0);
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);
        let symbols = [
            (triple >> 18) & 0x3F,
            (triple >> 12) & 0x3F,
            (triple >> 6) & 0x3F,
            triple & 0x3F,
        ];
        let keep = chunk.len() + 1;
        for (position, symbol) in symbols.iter().enumerate() {
            if position < keep {
                if let Some(byte) = alphabet.get(usize::try_from(*symbol).unwrap_or(0)) {
                    text.push(char::from(*byte));
                }
            } else if padded {
                text.push('=');
            }
        }
    }
    text
}

/// base64 解码；同时接受标准与 URL 安全字母表，忽略填充，遇到非法字符返回 `None`。
pub(super) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4 + 3);
    for byte in text.trim().bytes() {
        if byte == b'=' {
            continue;
        }
        let value = base64_value(byte)?;
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((accumulator >> bits) & 0xFF).ok()?);
        }
    }
    Some(bytes)
}

/// 单个 base64 字符的取值。
fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// 以 0600 创建（或截断）文件并写入。
#[cfg(unix)]
pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), GatewayKeyError> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| GatewayKeyError::Storage(format!("{}：{error}", path.display())))?;
    file.write_all(bytes)
        .map_err(|error| GatewayKeyError::Storage(format!("{}：{error}", path.display())))?;
    file.sync_all()
        .map_err(|error| GatewayKeyError::Storage(format!("{}：{error}", path.display())))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| GatewayKeyError::Storage(format!("{}：{error}", path.display())))?;
    Ok(())
}

/// 以追加方式打开审计日志；Unix 上同样按 0600 创建。
#[cfg(unix)]
pub(super) fn open_append_private(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}
/// Windows 没有 `std` 层的 0600 等价位：ACL 设置不在 `std` 能力面内，这里如实退化为
/// 默认权限，差异在交付说明里登记，不写成「已等价保护」。
#[cfg(not(unix))]
pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), GatewayKeyError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| GatewayKeyError::Storage(format!("{}：{error}", path.display())))?;
    file.write_all(bytes)
        .map_err(|error| GatewayKeyError::Storage(format!("{}：{error}", path.display())))?;
    file.sync_all()
        .map_err(|error| GatewayKeyError::Storage(format!("{}：{error}", path.display())))?;
    Ok(())
}

/// 以追加方式打开审计日志；非 Unix 平台按默认权限创建。
#[cfg(not(unix))]
pub(super) fn open_append_private(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// 已有文件的实际权限若不是 0600 就收紧；失败不影响读盘。
#[cfg(unix)]
pub(super) fn enforce_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.permissions().mode() & 0o777 != 0o600 {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// 非 Unix 平台没有等价权限位，不做任何动作。
#[cfg(not(unix))]
pub(super) fn enforce_private_permissions(_path: &Path) {}
