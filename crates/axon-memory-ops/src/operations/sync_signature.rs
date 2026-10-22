use axon_crypto::{fixed_hex_bytes, hex_bytes};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

pub(super) const MEMORY_EXPORT_SIGNATURE_STATUS_UNSIGNED: &str = "unsigned_local_digest";
pub(super) const MEMORY_EXPORT_SIGNATURE_STATUS_ED25519: &str = "signed_ed25519_digest";
pub(super) const MEMORY_EXPORT_SIGNATURE_ALGORITHM_ED25519: &str = "ed25519";

const MEMORY_EXPORT_SIGNATURE_PAYLOAD_PREFIX: &str = "memory-export-v1";
const ED25519_PUBLIC_KEY_BYTES: usize = 32;
const ED25519_SIGNATURE_BYTES: usize = 64;
const ED25519_SIGNING_SEED_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryExportSignatureView {
    pub(super) algorithm: String,
    pub(super) key_id: String,
    pub(super) public_key: String,
    pub(super) signature: String,
}

pub(super) fn sign_memory_export_digest(
    payload_digest: &str,
    key_id: &str,
    signing_seed_hex: &str,
) -> Result<MemoryExportSignatureView, &'static str> {
    let key_id = key_id.trim();
    if key_id.is_empty() {
        return Err("memory export signing key id 不能为空");
    }
    let seed = fixed_hex_bytes(signing_seed_hex, ED25519_SIGNING_SEED_BYTES)
        .ok_or("memory export signing seed hex 无效")?;
    let mut seed_bytes = [0_u8; ED25519_SIGNING_SEED_BYTES];
    seed_bytes.copy_from_slice(&seed);
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    let signature = signing_key.sign(signature_payload(payload_digest).as_bytes());
    Ok(MemoryExportSignatureView {
        algorithm: MEMORY_EXPORT_SIGNATURE_ALGORITHM_ED25519.to_owned(),
        key_id: key_id.to_owned(),
        public_key: hex_bytes(signing_key.verifying_key().to_bytes()),
        signature: hex_bytes(signature.to_bytes()),
    })
}

pub(super) fn verify_memory_export_signature(
    payload_digest: &str,
    signature: Option<&MemoryExportSignatureView>,
    trusted_public_key_hex: Option<&str>,
) -> Result<&'static str, &'static str> {
    let Some(signature) = signature else {
        return Ok(MEMORY_EXPORT_SIGNATURE_STATUS_UNSIGNED);
    };
    if signature.algorithm != MEMORY_EXPORT_SIGNATURE_ALGORITHM_ED25519 {
        return Err("memory import 签名算法不受支持");
    }
    let trusted_public_key = trusted_public_key_hex
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("memory import 缺少可信公钥，无法验签")?;
    if signature.public_key != trusted_public_key {
        return Err("memory import 签名公钥不在本机信任范围内");
    }
    verify_ed25519_signature(trusted_public_key, payload_digest, &signature.signature)?;
    Ok(MEMORY_EXPORT_SIGNATURE_STATUS_ED25519)
}

fn verify_ed25519_signature(
    public_key_hex: &str,
    payload_digest: &str,
    signature_hex: &str,
) -> Result<(), &'static str> {
    let public_key = fixed_hex_bytes(public_key_hex, ED25519_PUBLIC_KEY_BYTES)
        .ok_or("memory import 签名公钥格式无效")?;
    let signature_bytes = fixed_hex_bytes(signature_hex, ED25519_SIGNATURE_BYTES)
        .ok_or("memory import 签名格式无效")?;
    let public_key_bytes: [u8; ED25519_PUBLIC_KEY_BYTES] = public_key
        .try_into()
        .map_err(|_| "memory import 签名公钥格式无效")?;
    let signature_array: [u8; ED25519_SIGNATURE_BYTES] = signature_bytes
        .try_into()
        .map_err(|_| "memory import 签名格式无效")?;
    let verifying_key = VerifyingKey::from_bytes(&public_key_bytes)
        .map_err(|_| "memory import 签名公钥格式无效")?;
    let signature = Signature::from_bytes(&signature_array);
    verifying_key
        .verify(signature_payload(payload_digest).as_bytes(), &signature)
        .map_err(|_| "memory import 签名校验失败")
}

fn signature_payload(payload_digest: &str) -> String {
    format!("{MEMORY_EXPORT_SIGNATURE_PAYLOAD_PREFIX}:{payload_digest}")
}
