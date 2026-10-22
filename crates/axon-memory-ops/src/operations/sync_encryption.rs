use axon_crypto::{fixed_hex_bytes, random_bytes, sha256_tagged_hex};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};

pub(super) const MEMORY_EXPORT_ENCRYPTION_STATUS_PLAINTEXT: &str = "plaintext_ref_only";
pub(super) const MEMORY_EXPORT_ENCRYPTION_STATUS_ENCRYPTED: &str = "encrypted_xchacha20_poly1305";
const MEMORY_EXPORT_ENCRYPTION_ALGORITHM: &str = "xchacha20poly1305";
const MEMORY_EXPORT_ENCRYPTION_KEY_BYTES: usize = 32;
const MEMORY_EXPORT_ENCRYPTION_NONCE_BYTES: usize = 24;
const MEMORY_EXPORT_ENCRYPTION_AAD_PREFIX: &str = "memory-export-encrypted-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryEncryptedExportEnvelope {
    pub(super) status: String,
    pub(super) schema_version: u32,
    pub(super) algorithm: String,
    pub(super) key_id: String,
    pub(super) nonce: String,
    pub(super) aad_digest: String,
    pub(super) payload_digest: String,
    pub(super) ciphertext: String,
    pub(super) plaintext_records_exported: bool,
    pub(super) remote_write_allowed: bool,
}

pub(super) fn decrypt_memory_export_payload(
    envelope: &MemoryEncryptedExportEnvelope,
    key_hex: &str,
) -> Result<String, &'static str> {
    if envelope.status != MEMORY_EXPORT_ENCRYPTION_STATUS_ENCRYPTED
        || envelope.algorithm != MEMORY_EXPORT_ENCRYPTION_ALGORITHM
        || envelope.plaintext_records_exported
        || envelope.remote_write_allowed
    {
        return Err("memory import 加密 envelope 治理元数据无效");
    }
    let key = fixed_hex_bytes(key_hex, MEMORY_EXPORT_ENCRYPTION_KEY_BYTES)
        .ok_or("memory import decryption key hex 无效")?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| "memory import decryption key hex 无效")?;
    let nonce_bytes = STANDARD
        .decode(envelope.nonce.as_bytes())
        .map_err(|_| "memory import 加密 nonce 无效")?;
    if nonce_bytes.len() != MEMORY_EXPORT_ENCRYPTION_NONCE_BYTES {
        return Err("memory import 加密 nonce 无效");
    }
    let nonce =
        XNonce::try_from(nonce_bytes.as_slice()).map_err(|_| "memory import 加密 nonce 无效")?;
    let ciphertext = STANDARD
        .decode(envelope.ciphertext.as_bytes())
        .map_err(|_| "memory import 加密 ciphertext 无效")?;
    let aad = encryption_aad(
        &envelope.payload_digest,
        envelope.schema_version,
        &envelope.key_id,
    );
    if envelope.aad_digest != sha256_tagged_hex(aad.as_bytes()) {
        return Err("memory import 加密 AAD digest 校验失败");
    }
    let plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "memory import 加密 payload 解密失败")?;
    String::from_utf8(plaintext).map_err(|_| "memory import 加密 payload 不是 UTF-8")
}

pub(super) fn plaintext_memory_import_bundle(
    text: &str,
    decryption_key_hex: Option<&str>,
) -> Result<String, &'static str> {
    if let Ok(envelope) = serde_json::from_str::<MemoryEncryptedExportEnvelope>(text) {
        let key_hex =
            decryption_key_hex.ok_or("memory import 加密导出包需要 decryption-key-hex")?;
        return decrypt_memory_export_payload(&envelope, key_hex);
    }
    if decryption_key_hex.is_some() {
        return Err("memory import 明文导出包不能携带 decryption-key-hex");
    }
    Ok(text.to_owned())
}

pub(super) fn encrypt_memory_export_payload(
    plaintext: &[u8],
    payload_digest: &str,
    schema_version: u32,
    key_id: &str,
    key_hex: &str,
) -> Result<MemoryEncryptedExportEnvelope, &'static str> {
    let key_id = key_id.trim();
    if key_id.is_empty() {
        return Err("memory export encryption key id 不能为空");
    }
    let key = fixed_hex_bytes(key_hex, MEMORY_EXPORT_ENCRYPTION_KEY_BYTES)
        .ok_or("memory export encryption key hex 无效")?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| "memory export encryption key hex 无效")?;
    let nonce_bytes = random_bytes::<MEMORY_EXPORT_ENCRYPTION_NONCE_BYTES>()
        .map_err(|_| "memory export 加密 nonce 生成失败")?;
    let nonce = XNonce::from(nonce_bytes);
    let aad = encryption_aad(payload_digest, schema_version, key_id);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "memory export 加密失败")?;

    Ok(MemoryEncryptedExportEnvelope {
        status: MEMORY_EXPORT_ENCRYPTION_STATUS_ENCRYPTED.to_owned(),
        schema_version,
        algorithm: MEMORY_EXPORT_ENCRYPTION_ALGORITHM.to_owned(),
        key_id: key_id.to_owned(),
        nonce: STANDARD.encode(nonce),
        aad_digest: sha256_tagged_hex(aad.as_bytes()),
        payload_digest: payload_digest.to_owned(),
        ciphertext: STANDARD.encode(ciphertext),
        plaintext_records_exported: false,
        remote_write_allowed: false,
    })
}

fn encryption_aad(payload_digest: &str, schema_version: u32, key_id: &str) -> String {
    format!("{MEMORY_EXPORT_ENCRYPTION_AAD_PREFIX}:{schema_version}:{key_id}:{payload_digest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn encrypted_export_envelope_hides_plaintext_and_keeps_digest_metadata() {
        let envelope = encrypt_memory_export_payload(
            br#"{"records":["memory-ref:secret"]}"#,
            "sha256:payload",
            1,
            "local-key",
            TEST_KEY_HEX,
        )
        .expect("有效 key 应生成加密 envelope");

        assert_eq!(envelope.status, MEMORY_EXPORT_ENCRYPTION_STATUS_ENCRYPTED);
        assert_eq!(envelope.algorithm, MEMORY_EXPORT_ENCRYPTION_ALGORITHM);
        assert_eq!(envelope.payload_digest, "sha256:payload");
        assert!(!envelope.plaintext_records_exported);
        assert!(!envelope.remote_write_allowed);
        assert!(!envelope.ciphertext.contains("memory-ref:secret"));
        assert_eq!(
            STANDARD
                .decode(envelope.nonce.as_bytes())
                .expect("nonce 应为 base64")
                .len(),
            MEMORY_EXPORT_ENCRYPTION_NONCE_BYTES
        );
    }

    #[test]
    fn encrypted_export_rejects_invalid_key_material() {
        assert!(
            encrypt_memory_export_payload(b"{}", "sha256:payload", 1, "local-key", "bad").is_err()
        );
        assert!(
            encrypt_memory_export_payload(b"{}", "sha256:payload", 1, "", TEST_KEY_HEX).is_err()
        );
    }

    #[test]
    fn encrypted_export_round_trips_with_matching_key() {
        let envelope = encrypt_memory_export_payload(
            br#"{"records":["memory-ref:secret"]}"#,
            "sha256:payload",
            1,
            "local-key",
            TEST_KEY_HEX,
        )
        .expect("有效 key 应生成加密 envelope");

        let decrypted =
            decrypt_memory_export_payload(&envelope, TEST_KEY_HEX).expect("匹配 key 应可解密");

        assert!(decrypted.contains("memory-ref:secret"));
    }

    #[test]
    fn encrypted_export_rejects_wrong_decryption_key() {
        let envelope =
            encrypt_memory_export_payload(b"{}", "sha256:payload", 1, "local-key", TEST_KEY_HEX)
                .expect("有效 key 应生成加密 envelope");

        assert!(
            decrypt_memory_export_payload(
                &envelope,
                "1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100",
            )
            .is_err()
        );
    }
}
