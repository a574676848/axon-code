//! 模型目录解析与文件校验。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::manifest::{Manifest, ManifestError, ModelFamily};
use super::sha256::Sha256;

/// 宿主支持的清单主版本。
const SUPPORTED_MAJOR: u64 = 1;

/// 解析结果。
#[derive(Debug)]
pub enum ModelResolution {
    /// 可用。
    Ready {
        /// 清单。
        manifest: Manifest,
        /// 模型目录。
        dir: PathBuf,
    },
    /// 目录不存在。
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

    /// 启动预检：根目录可读，且已存在目录里的清单都能解析。
    ///
    /// # Errors
    ///
    /// 根目录不可读，或任一已存在模型的清单非法。
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
                let manifest_path = entry.path().join("manifest.json");
                if !manifest_path.is_file() {
                    continue;
                }
                let bytes = std::fs::read(&manifest_path)
                    .map_err(|error| format!("清单不可读：{error}"))?;
                Manifest::parse(&bytes).map_err(|error| describe(&error))?;
            }
        }
        Ok(())
    }

    /// 解析并校验指定模型。
    #[must_use]
    pub fn resolve(&self, family: ModelFamily, name: &str) -> ModelResolution {
        let dir = self.root.join(family.as_token()).join(name);
        if !dir.is_dir() {
            return ModelResolution::Missing;
        }
        let manifest_path = dir.join("manifest.json");
        let bytes = match std::fs::read(&manifest_path) {
            Ok(bytes) => bytes,
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
        match verify(&dir, &manifest) {
            Ok(()) => ModelResolution::Ready { manifest, dir },
            Err(VerifyError::Missing) => ModelResolution::Missing,
            Err(VerifyError::Unusable(reason)) => ModelResolution::Unusable { reason },
        }
    }
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

fn verify(dir: &Path, manifest: &Manifest) -> Result<(), VerifyError> {
    for entry in &manifest.files {
        let path = dir.join(&entry.path);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => return Err(VerifyError::Missing),
        };
        if !metadata.is_file() {
            return Err(VerifyError::Unusable(format!("{} 不是文件", entry.path)));
        }
        if metadata.len() != entry.bytes {
            return Err(VerifyError::Unusable(format!("{} 字节数不符", entry.path)));
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
    }
    Ok(())
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

    fn manifest_json(name: &str, sha: &str, bytes: u64, family: &str) -> String {
        format!(
            r#"{{
                "family": "{family}",
                "name": "{name}",
                "version": "1.0.0",
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

    #[test]
    fn ready_when_files_match() {
        let temp = TempRoot::new("ready");
        temp.write("asr/demo/model.bin", b"data");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("demo", &hex_digest(b"data"), 4, "asr").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        match store.resolve(ModelFamily::Asr, "demo") {
            ModelResolution::Ready { manifest, .. } => assert_eq!(manifest.name, "demo"),
            other => panic!("期望 Ready，得到 {other:?}"),
        }
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
    fn unusable_when_digest_mismatches() {
        let temp = TempRoot::new("digest");
        temp.write("asr/demo/model.bin", b"data");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("demo", &"0".repeat(64), 4, "asr").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Unusable { .. }
        ));
    }

    #[test]
    fn unusable_when_manifest_mismatches_directory() {
        let temp = TempRoot::new("mismatch");
        temp.write("asr/demo/model.bin", b"data");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("other", &hex_digest(b"data"), 4, "asr").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Unusable { .. }
        ));
    }

    #[test]
    fn missing_file_is_reported_as_missing() {
        let temp = TempRoot::new("no-file");
        temp.write(
            "asr/demo/manifest.json",
            manifest_json("demo", &"0".repeat(64), 4, "asr").as_bytes(),
        );
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
            ModelResolution::Missing
        ));
    }

    #[test]
    fn future_major_version_is_unusable() {
        let temp = TempRoot::new("future");
        temp.write("asr/demo/model.bin", b"data");
        let json = manifest_json("demo", &hex_digest(b"data"), 4, "asr")
            .replace("\"version\": \"1.0.0\"", "\"version\": \"2.0.0\"");
        temp.write("asr/demo/manifest.json", json.as_bytes());
        let store = ModelStore::new(&temp.0);
        assert!(matches!(
            store.resolve(ModelFamily::Asr, "demo"),
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
