//! 模型清单：解析与校验。

use serde::{Deserialize, Serialize};

/// 模型族。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// 端点检测。
    Vad,
    /// 识别。
    Asr,
    /// 合成。
    Tts,
}

impl ModelFamily {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Vad => "vad",
            Self::Asr => "asr",
            Self::Tts => "tts",
        }
    }

    /// 从记号解析。
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "vad" => Some(Self::Vad),
            "asr" => Some(Self::Asr),
            "tts" => Some(Self::Tts),
            _ => None,
        }
    }
}

/// 清单中的一个文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestFile {
    /// 相对路径。
    pub path: String,
    /// 64 位十六进制摘要。
    pub sha256: String,
    /// 字节数。
    pub bytes: u64,
}

/// 模型清单。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// 模型族。
    pub family: String,
    /// 模型标识。
    pub name: String,
    /// 版本。
    pub version: String,
    /// 运行时。
    pub runtime: String,
    /// 采样率。
    pub sample_rate: u32,
    /// 语言。
    pub languages: Vec<String>,
    /// 许可证。
    pub license: String,
    /// 实时因子中位数。
    pub rtf_p50: f64,
    /// 实时因子 P95。
    pub rtf_p95: f64,
    /// 常驻内存。
    pub ram_mb: u64,
    /// 文件列表。
    pub files: Vec<ManifestFile>,
}

/// 清单错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// JSON 非法或字段多余。
    Json(String),
    /// 字段非法。
    Invalid(&'static str),
    /// 模型族不支持。
    Family(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(reason) => write!(f, "清单解析失败：{reason}"),
            Self::Invalid(field) => write!(f, "清单字段非法：{field}"),
            Self::Family(family) => write!(f, "不支持的模型族：{family}"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    /// 从 JSON 字节解析并校验。
    ///
    /// # Errors
    ///
    /// JSON 非法、未知字段或字段校验失败。
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| ManifestError::Json(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 字段校验。
    ///
    /// # Errors
    ///
    /// 任一字段越界或不符合约定。
    pub fn validate(&self) -> Result<(), ManifestError> {
        if ModelFamily::parse(&self.family).is_none() {
            return Err(ManifestError::Family(self.family.clone()));
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::Invalid("name"));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::Invalid("version"));
        }
        if self.runtime.trim().is_empty() {
            return Err(ManifestError::Invalid("runtime"));
        }
        if self.sample_rate != 16_000 {
            return Err(ManifestError::Invalid("sampleRate"));
        }
        if !self.rtf_p50.is_finite() || self.rtf_p50 <= 0.0 {
            return Err(ManifestError::Invalid("rtfP50"));
        }
        if !self.rtf_p95.is_finite() || self.rtf_p95 < self.rtf_p50 {
            return Err(ManifestError::Invalid("rtfP95"));
        }
        if self.files.is_empty() {
            return Err(ManifestError::Invalid("files"));
        }
        for file in &self.files {
            if !safe_relative(&file.path) {
                return Err(ManifestError::Invalid("files.path"));
            }
            if !is_sha256_hex(&file.sha256) {
                return Err(ManifestError::Invalid("files.sha256"));
            }
            if file.bytes == 0 {
                return Err(ManifestError::Invalid("files.bytes"));
            }
        }
        Ok(())
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains("..")
        && !path.contains(':')
}

#[cfg(test)]
mod tests {
    use super::{Manifest, ManifestError};

    fn valid_json() -> String {
        r#"{
            "family": "asr",
            "name": "fixture-asr",
            "version": "1.0.0",
            "runtime": "fixture",
            "sampleRate": 16000,
            "languages": ["zh"],
            "license": "MIT",
            "rtfP50": 0.2,
            "rtfP95": 0.3,
            "ramMb": 100,
            "files": [{"path": "model.bin", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bytes": 4}]
        }"#
            .to_owned()
    }

    #[test]
    fn valid_manifest_parses() {
        let manifest = Manifest::parse(valid_json().as_bytes()).expect("应解析成功");
        assert_eq!(manifest.sample_rate, 16_000);
        assert_eq!(manifest.files.len(), 1);
    }

    #[test]
    fn unknown_field_is_rejected() {
        let json = valid_json().replace("\"ramMb\": 100,", "\"ramMb\": 100, \"extra\": 1,");
        assert!(matches!(
            Manifest::parse(json.as_bytes()),
            Err(ManifestError::Json(_))
        ));
    }

    #[test]
    fn invalid_fields_are_rejected() {
        let cases = [
            ("\"sampleRate\": 16000", "\"sampleRate\": 8000"),
            ("\"rtfP50\": 0.2", "\"rtfP50\": 0.0"),
            ("\"family\": \"asr\"", "\"family\": \"image\""),
            ("\"path\": \"model.bin\"", "\"path\": \"../model.bin\""),
            (
                "\"sha256\": \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"",
                "\"sha256\": \"short\"",
            ),
        ];
        for (from, to) in cases {
            let json = valid_json().replace(from, to);
            assert!(
                Manifest::parse(json.as_bytes()).is_err(),
                "应拒绝替换 {from} → {to}"
            );
        }
    }
}
