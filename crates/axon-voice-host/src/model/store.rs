//! 模型目录解析与文件校验。

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::manifest::{Manifest, ManifestError, ModelFamily};
use super::sha256::{self, Sha256};

/// 宿主支持的清单主版本。
const SUPPORTED_MAJOR: u64 = 1;

/// 解析结果。
#[derive(Debug)]
pub enum ModelResolution {
    /// 清单与文件均通过校验。
    Ready {
        /// 清单。
        manifest: Manifest,
        /// 模型目录。
        dir: PathBuf,
        /// 清单字节的 SHA-256。
        manifest_sha256: String,
    },
    /// 模型或清单文件缺失。
    Missing,
    /// 存在但不可用。
    Unusable {
        /// 已脱敏原因。
        reason: String,
    },
}

/// 模型仓库。
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    /// 以运行态根下的 `voice/models` 构造。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 根目录。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 指定模型的目录。
    #[must_use]
    pub fn model_dir(&self, family: ModelFamily, name: &str) -> PathBuf {
        self.root.join(family.as_token()).join(name)
    }

    /// 清单是否存在：只做存在性判断，不读内容也不做哈希。
    ///
    /// 用于「模型已在盘上、但该族没有预热探针」这类回执：想要的是清单在不在，
    /// 用不到全量校验的代价，也不该为非法模型名拼出越界路径。
    #[must_use]
    pub fn has_manifest(&self, family: ModelFamily, name: &str) -> bool {
        valid_name(name) && self.model_dir(family, name).join("manifest.json").is_file()
    }

    /// 根目录是否可读且是目录。
    ///
    /// # Errors
    ///
    /// 路径不存在、不是目录或不可读。
    pub fn check_root(&self) -> Result<(), String> {
        let metadata =
            std::fs::metadata(&self.root).map_err(|error| format!("模型目录不可读：{error}"))?;
        if !metadata.is_dir() {
            return Err("模型目录不是目录".to_owned());
        }
        std::fs::read_dir(&self.root).map_err(|error| format!("模型目录不可读：{error}"))?;
        Ok(())
    }

    /// 启动预检：根目录可读，且已存在模型都能通过全量校验。
    ///
    /// # Errors
    ///
    /// 根目录不可读，或任一已存在模型不可用。
    pub fn preflight(&self) -> Result<(), String> {
        self.check_root()?;
        for family in [ModelFamily::Vad, ModelFamily::Asr, ModelFamily::Tts] {
            let family_dir = self.root.join(family.as_token());
            if !family_dir.is_dir() {
                continue;
            }
            let entries = std::fs::read_dir(&family_dir)
                .map_err(|error| format!("模型族目录不可读：{error}"))?;
            for entry in entries {
                let entry = entry.map_err(|error| format!("模型目录不可读：{error}"))?;
                if !entry.path().is_dir() {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    return Err("模型目录名不是合法 UTF-8".to_owned());
                };
                self.preflight_model(family, &name)?;
            }
        }
        Ok(())
    }

    /// 预检单个模型：只校验清单与文件元信息，不做全量哈希。
    ///
    /// 启动阶段不读整模型，哈希留给 `warmup`，避免 100 到 300 MB 模型拖垮握手。
    fn preflight_model(&self, family: ModelFamily, name: &str) -> Result<(), String> {
        if !valid_name(name) {
            return Err("模型名非法".to_owned());
        }
        let dir = self.model_dir(family, name);
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.is_file() {
            return Ok(());
        }
        let bytes =
            std::fs::read(&manifest_path).map_err(|error| format!("清单不可读：{error}"))?;
        let manifest = Manifest::parse(&bytes).map_err(|error| describe(&error))?;
        if manifest.family != family.as_token() || manifest.name != name {
            return Err("清单与目录不匹配".to_owned());
        }
        if major_version(&manifest.version) > SUPPORTED_MAJOR {
            return Err("模型版本高于宿主支持范围，请升级 Axon".to_owned());
        }
        for entry in &manifest.files {
            let metadata = std::fs::metadata(dir.join(&entry.path))
                .map_err(|error| format!("{} 不可读：{error}", entry.path))?;
            if !metadata.is_file() || metadata.len() != entry.bytes {
                return Err(format!("{} 字节数不符", entry.path));
            }
        }
        Ok(())
    }
    /// 解析并校验指定模型。
    #[must_use]
    pub fn resolve(&self, family: ModelFamily, name: &str) -> ModelResolution {
        if !valid_name(name) {
            return ModelResolution::Unusable {
                reason: "模型名非法".to_owned(),
            };
        }
        let dir = self.model_dir(family, name);
        if !dir.is_dir() {
            return ModelResolution::Missing;
        }
        let manifest_path = dir.join("manifest.json");
        let bytes = match std::fs::read(&manifest_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ModelResolution::Missing;
            }
            Err(error) => {
                return ModelResolution::Unusable {
                    reason: format!("清单不可读：{error}"),
                };
            }
        };
        let manifest = match Manifest::parse(&bytes) {
            Ok(manifest) => manifest,
            Err(error) => {
                return ModelResolution::Unusable {
                    reason: describe(&error),
                };
            }
        };
        if manifest.family != family.as_token() || manifest.name != name {
            return ModelResolution::Unusable {
                reason: "清单与目录不匹配".to_owned(),
            };
        }
        if major_version(&manifest.version) > SUPPORTED_MAJOR {
            return ModelResolution::Unusable {
                reason: "模型版本高于宿主支持范围，请升级 Axon".to_owned(),
            };
        }
        match verify(&dir, &manifest, &self.root, &sha256::hex_digest(&bytes)) {
            Ok(()) => ModelResolution::Ready {
                manifest,
                dir,
                manifest_sha256: sha256::hex_digest(&bytes),
            },
            Err(VerifyError::Missing) => ModelResolution::Missing,
            Err(VerifyError::Unusable(reason)) => ModelResolution::Unusable { reason },
        }
    }

    /// 解析并校验，同时要求清单摘要与调用方给出的值一致。
    #[must_use]
    pub fn resolve_verified(
        &self,
        family: ModelFamily,
        name: &str,
        expected_manifest_sha256: &str,
    ) -> ModelResolution {
        match self.resolve(family, name) {
            ModelResolution::Ready {
                manifest,
                dir,
                manifest_sha256,
            } => {
                if manifest_sha256.eq_ignore_ascii_case(expected_manifest_sha256) {
                    ModelResolution::Ready {
                        manifest,
                        dir,
                        manifest_sha256,
                    }
                } else {
                    ModelResolution::Unusable {
                        reason: "清单摘要与请求不一致".to_owned(),
                    }
                }
            }
            other => other,
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
}

fn describe(error: &ManifestError) -> String {
    match error {
        ManifestError::Json(_) => "清单不是合法 JSON".to_owned(),
        ManifestError::Invalid(field) => format!("清单字段非法：{field}"),
        ManifestError::Family(_) => "清单模型族不支持".to_owned(),
    }
}

enum VerifyError {
    Missing,
    Unusable(String),
}

fn major_version(version: &str) -> u64 {
    version
        .split('.')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn verify(
    dir: &Path,
    manifest: &Manifest,
    root: &Path,
    manifest_sha256: &str,
) -> Result<(), VerifyError> {
    let stamps = load_stamps(root, manifest_sha256);
    let mut refreshed: HashMap<String, Stamp> = HashMap::new();
    for entry in &manifest.files {
        let path = dir.join(&entry.path);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(VerifyError::Missing);
            }
            Err(error) => {
                return Err(VerifyError::Unusable(format!(
                    "{} 不可读：{error}",
                    entry.path
                )));
            }
        };
        if !metadata.is_file() {
            return Err(VerifyError::Unusable(format!("{} 不是文件", entry.path)));
        }
        if metadata.len() != entry.bytes {
            return Err(VerifyError::Unusable(format!("{} 字节数不符", entry.path)));
        }
        let stamp = Stamp {
            bytes: metadata.len(),
            modified_ms: modified_ms(&metadata),
        };
        // 大小与修改时刻都与上次成功校验一致时，跳过重复哈希：整份模型约 0.5 GB，
        // 每次会话都全量重算会把 `start → warm` 顶出 10 s。任何字节改动都会改变
        // 修改时刻，从而回到全量哈希；这不是完整性保证的替代，而是同一摘要下的缓存。
        if stamps.get(&entry.path) == Some(&stamp) {
            refreshed.insert(entry.path.clone(), stamp);
            continue;
        }
        let mut file = File::open(&path)
            .map_err(|error| VerifyError::Unusable(format!("{} 不可读：{error}", entry.path)))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|error| {
                VerifyError::Unusable(format!("{} 读取失败：{error}", entry.path))
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if hasher.finish_hex() != entry.sha256.to_lowercase() {
            return Err(VerifyError::Unusable(format!("{} 摘要不符", entry.path)));
        }
        refreshed.insert(entry.path.clone(), stamp);
    }
    save_stamps(root, manifest_sha256, &refreshed);
    Ok(())
}

/// 单个文件的校验印章：字节数与修改时刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Stamp {
    /// 文件字节数。
    bytes: u64,
    /// 修改时刻（UNIX 毫秒）。
    modified_ms: u64,
}

/// 校验印章的落盘位置：模型根下的 `.verify/<清单摘要>.json`。
fn stamp_path(root: &Path, manifest_sha256: &str) -> PathBuf {
    root.join(".verify").join(format!("{manifest_sha256}.json"))
}

fn load_stamps(root: &Path, manifest_sha256: &str) -> HashMap<String, Stamp> {
    let Ok(bytes) = std::fs::read(stamp_path(root, manifest_sha256)) else {
        return HashMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_stamps(root: &Path, manifest_sha256: &str, stamps: &HashMap<String, Stamp>) {
    let path = stamp_path(root, manifest_sha256);
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(stamps) {
        let _ = std::fs::write(&path, bytes);
    }
}

fn modified_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::{ModelResolution, ModelStore};
    use crate::model::manifest::ModelFamily;
    use crate::model::sha256::hex_digest;
    use std::path::PathBuf;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("axon-voice-model-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("创建临时目录");
            Self(path)
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("创建父目录");
            }
            std::fs::write(path, bytes).expect("写文件");
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest_json(name: &str, sha: &str, bytes: u64, family: &str, version: &str) -> String {
        format!(
            r#"{{
                "family": "{family}",
                "name": "{name}",
                "version": "{version}",
                "runtime": "fixture",
                "sampleRate": 16000,
                "languages": ["zh"],
                "license": "MIT",
                "rtfP50": 0.2,
                "rtfP95": 0.3,
                "ramMb": 100,
                "files": [{{"path": "model.bin", "sha256": "{sha}", "bytes": {bytes}}}]
            }}"#
        )
    }

    fn install(temp: &TempRoot, name: &str, version: &str) {
        temp.write(&format!("asr/{name}/model.bin"), b"data");
        temp.write(
            &format!("asr/{name}/manifest.json"),
            manifest_json(name, &hex_digest(b"data"), 4, "asr", version).as_bytes(),
        );
    }

    #[test]
    fn ready_when_files_match() {
        let temp = TempRoot::new("ready");
        install(&temp, "demo", "1.0.0");
        let store = ModelStore::new(&temp.0);
        match store.resolve(ModelFamily::Asr, "demo") {
            ModelResolution::Ready {
                manifest,
                manifest_sha256,
                ..
            } => {
                assert_eq!(manifest.name, "demo");
                assert_eq!(manifest_sha256.len(), 64);
            }
            other => panic!("期望 Ready，得到 {other:?}"),
        }
    }

    #[test]
    fn verification_stamp_is_written_and_invalidated_by_mtime() {
        let temp = TempRoot::new("stamp");
        install(&temp, "demo", "1.0.0");
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Ready { .. }
        ));
        let stamps = std::fs::read_dir(temp.0.join(".verify"))
            .expect("应写下校验印章")
            .count();
        assert_eq!(stamps, 1, "每个清单摘要一份印章");

        // 内容变化但字节数不变：修改时刻必然变化，从而回到全量哈希并发现摘要不符。
        temp.write("asr/demo/model.bin", b"DATA");
        match store.resolve(ModelFamily::Asr, "demo") {
            ModelResolution::Unusable { reason } => {
                assert!(reason.contains("摘要不符"), "{reason}");
            }
            other => panic!("期望摘要不符，得到 {other:?}"),
        }
    }

    #[test]
    fn manifest_probe_rejects_illegal_names_and_absent_files() {
        let temp = TempRoot::new("has-manifest");
        temp.write(
            "tts/demo/manifest.json",
            manifest_json("demo", &"0".repeat(64), 4, "tts", "1.0.0").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(store.has_manifest(ModelFamily::Tts, "demo"));
        assert!(!store.has_manifest(ModelFamily::Tts, "absent"));
        assert!(!store.has_manifest(ModelFamily::Tts, "../demo"));
    }

    #[test]
    fn missing_when_directory_absent() {
        let temp = TempRoot::new("missing");
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Missing
        ));
    }

    #[test]
    fn missing_when_listed_file_absent() {
        let temp = TempRoot::new("no-file");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("demo", &"0".repeat(64), 4, "asr", "1.0.0").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Missing
        ));
    }

    #[test]
    fn unusable_when_digest_mismatches() {
        let temp = TempRoot::new("digest");
        temp.write("asr/demo/model.bin", b"data");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("demo", &"0".repeat(64), 4, "asr", "1.0.0").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Unusable { .. }
        ));
    }

    #[test]
    fn unusable_when_byte_count_mismatches() {
        let temp = TempRoot::new("bytes");
        temp.write("asr/demo/model.bin", b"data");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("demo", &hex_digest(b"data"), 5, "asr", "1.0.0").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Unusable { .. }
        ));
    }

    #[test]
    fn uppercase_digest_is_accepted() {
        let temp = TempRoot::new("upper");
        temp.write("asr/demo/model.bin", b"data");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json(
                "demo",
                &hex_digest(b"data").to_uppercase(),
                4,
                "asr",
                "1.0.0",
            )
            .as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Ready { .. }
        ));
    }

    #[test]
    fn unusable_when_manifest_mismatches_directory() {
        let temp = TempRoot::new("mismatch");
        temp.write("asr/demo/model.bin", b"data");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("other", &hex_digest(b"data"), 4, "asr", "1.0.0").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Unusable { .. }
        ));
    }

    #[test]
    fn future_major_version_is_unusable() {
        let temp = TempRoot::new("future");
        install(&temp, "demo", "2.0.0");
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Unusable { .. }
        ));
    }

    #[test]
    fn traversal_names_are_rejected() {
        let temp = TempRoot::new("traversal");
        let store = ModelStore::new(&temp.0);
        for name in ["..", ".", "a/b", "a\\b", "C:demo", ""] {
            assert!(
                matches!(
                    store.resolve(ModelFamily::Asr, name),
                    ModelResolution::Unusable { .. }
                ),
                "应拒绝 {name}"
            );
        }
    }

    #[test]
    fn manifest_digest_is_bound_to_request() {
        let temp = TempRoot::new("manifest-sha");
        install(&temp, "demo", "1.0.0");
        let store = ModelStore::new(&temp.0);
        let ModelResolution::Ready {
            manifest_sha256, ..
        } = store.resolve(ModelFamily::Asr, "demo")
        else {
            panic!("应可用");
        };
        assert!(matches!(
            store.resolve_verified(ModelFamily::Asr, "demo", &manifest_sha256),
            ModelResolution::Ready { .. }
        ));
        assert!(matches!(
            store.resolve_verified(ModelFamily::Asr, "demo", &"0".repeat(64)),
            ModelResolution::Unusable { .. }
        ));
    }

    #[test]
    fn preflight_rejects_invalid_manifest() {
        let temp = TempRoot::new("preflight");
        temp.write("asr/demo/model.bin", b"data");
        temp.write("asr/demo/manifest.json", b"{not-json}");
        let store = ModelStore::new(&temp.0);
        assert!(store.preflight().is_err());
    }

    #[test]
    fn preflight_accepts_empty_root() {
        let temp = TempRoot::new("preflight-empty");
        let store = ModelStore::new(&temp.0);
        assert_eq!(store.preflight(), Ok(()));
    }

    #[test]
    fn root_check_rejects_missing_and_file() {
        let temp = TempRoot::new("root");
        let store = ModelStore::new(temp.0.join("nope"));
        assert!(store.check_root().is_err());

        temp.write("plain.txt", b"x");
        let store = ModelStore::new(temp.0.join("plain.txt"));
        assert!(store.check_root().is_err());
    }
}
