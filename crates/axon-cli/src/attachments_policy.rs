//! 附件上传的内容策略：类型白名单与「声明解压体积」护栏。
//!
//! 两道检查都在落盘之前完成，且只看文件名与服务端自己读到的字节，不信任客户端
//! 声明的 Content-Type：
//!
//! - **类型白名单**：按扩展名放行常见图片、文档、文本与代码、压缩包与音视频；
//!   未知扩展名直接拒绝，避免后端变成任意文件的暂存区。
//! - **压缩炸弹**：gzip 与 zip 在文件头里写了解压后的体积，读出来超过上限就拒绝。
//!   解压发生在下游（预览、模型读取），因此这道检查必须在上传时做完。

use std::path::Path;

/// 解压后允许的最大字节数：512 MiB。
///
/// 上传本体上限是 64 MiB，正常压缩包远达不到这个量级；512 MiB 已经足够宽容，
/// 又能挡住「30 KiB 解出几十 GiB」这一类。
pub(crate) const MAX_DECLARED_EXPANSION_BYTES: u64 = 512 * 1024 * 1024;

/// 允许上传的扩展名（小写，不含点）。
const ALLOWED_EXTENSIONS: &[&str] = &[
    // 图片
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif", "heic",
    // 文档与办公
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "epub", "pages",
    "numbers", "key", // 文本与数据
    "txt", "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml",
    "ini", "conf", "cfg", "xml", "html", "htm", "css", "scss", "sql", "ipynb", "diff", "patch",
    // 代码
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "kts", "c", "h", "cc",
    "cpp", "hpp", "cs", "rb", "php", "swift", "scala", "sh", "bash", "zsh", "ps1", "psm1", "bat",
    "cmd", "lua", "r", "dart", "ex", "exs", "erl", "hs", "clj", "vue", "svelte", "gradle", "cmake",
    // 压缩包
    "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "tar", // 音视频
    "mp3", "wav", "ogg", "flac", "m4a", "aac", "mp4", "mov", "mkv", "webm", "avi",
    // 其它常见附件
    "ttf", "otf", "woff", "woff2", "sqlite", "db", "parquet", "wasm", "map",
];

/// 策略拒绝的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PolicyRejection {
    /// 扩展名不在白名单内。
    UnsupportedType(String),
    /// 压缩包声明的解压体积超过上限。
    ExpansionTooLarge {
        /// 声明的解压字节数。
        declared: u64,
        /// 允许的上限。
        limit: u64,
    },
    /// 压缩包结构无法解析，不能担保解压体积。
    UnreadableArchive,
}

impl PolicyRejection {
    /// 对外的错误码。
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedType(_) => "unsupported_file_type",
            Self::ExpansionTooLarge { .. } | Self::UnreadableArchive => "archive_too_large",
        }
    }

    /// 一句可执行的中文说明。
    pub(crate) fn message(&self) -> String {
        match self {
            Self::UnsupportedType(name) => format!(
                "不支持的文件类型：{name}。允许图片、文档、文本与代码、压缩包与音视频等常见格式"
            ),
            Self::ExpansionTooLarge { declared, limit } => {
                format!("压缩包解压后约 {declared} 字节，超过 {limit} 字节上限，已拒绝")
            }
            Self::UnreadableArchive => "压缩包结构无法解析，无法确认解压体积，已拒绝".to_string(),
        }
    }
}

/// 校验一次上传：类型白名单 + 压缩炸弹。
pub(crate) fn check(name: &str, content: &[u8]) -> Result<(), PolicyRejection> {
    let extension = extension_of(name);
    if !ALLOWED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(PolicyRejection::UnsupportedType(if extension.is_empty() {
            name.to_owned()
        } else {
            format!("{name}（.{extension}）")
        }));
    }
    // 压缩包必须读得出声明的解压体积：读不出来就不放行，宁可拒绝也不担保。
    let declared = if content.starts_with(&[0x1f, 0x8b]) {
        gzip_expansion(content)
    } else if content.starts_with(b"PK\x03\x04") {
        zip_expansion(content)
    } else {
        return Ok(());
    };
    match declared {
        Some(bytes) if bytes > MAX_DECLARED_EXPANSION_BYTES => {
            Err(PolicyRejection::ExpansionTooLarge {
                declared: bytes,
                limit: MAX_DECLARED_EXPANSION_BYTES,
            })
        }
        Some(_) => Ok(()),
        None => Err(PolicyRejection::UnreadableArchive),
    }
}

/// 取小写扩展名；没有扩展名时返回空串。
fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// gzip 的最后 4 字节是解压后的体积（小端，32 位）。
fn gzip_expansion(content: &[u8]) -> Option<u64> {
    if content.len() < 18 {
        return None;
    }
    let tail = &content[content.len() - 4..];
    Some(u64::from(u32::from_le_bytes([
        tail[0], tail[1], tail[2], tail[3],
    ])))
}

/// 从 zip 的中央目录累加各条目的解压体积。
///
/// ZIP64 的条目把体积写在扩展字段里，这里不解析：遇到 0xFFFF_FFFF 直接按
/// 「无法确认」处理，宁可拒绝也不放行一个读不出体积的压缩包。
fn zip_expansion(content: &[u8]) -> Option<u64> {
    const EOCD: &[u8] = b"PK\x05\x06";
    const CENTRAL: &[u8] = b"PK\x01\x02";
    const ZIP64_MARKER: u32 = 0xFFFF_FFFF;

    let search_start = content.len().saturating_sub(65_557);
    let eocd = (search_start..content.len().saturating_sub(3))
        .rev()
        .find(|start| content[*start..].starts_with(EOCD))?;
    let directory_offset =
        u32::from_le_bytes(content.get(eocd + 16..eocd + 20)?.try_into().ok()?) as usize;
    let mut cursor = directory_offset;
    let mut total: u64 = 0;
    while content[cursor..].starts_with(CENTRAL) {
        let uncompressed =
            u32::from_le_bytes(content.get(cursor + 24..cursor + 28)?.try_into().ok()?);
        if uncompressed == ZIP64_MARKER {
            return None;
        }
        total = total.saturating_add(u64::from(uncompressed));
        let name_len =
            u16::from_le_bytes(content.get(cursor + 28..cursor + 30)?.try_into().ok()?) as usize;
        let extra_len =
            u16::from_le_bytes(content.get(cursor + 30..cursor + 32)?.try_into().ok()?) as usize;
        let comment_len =
            u16::from_le_bytes(content.get(cursor + 32..cursor + 34)?.try_into().ok()?) as usize;
        cursor += 46 + name_len + extra_len + comment_len;
        if cursor >= content.len() {
            break;
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::{PolicyRejection, check, gzip_expansion, zip_expansion};

    /// 一个最小 gzip 头：魔数 + 方法 + 标志 + 时间 + 标志位 + 系统 + 尾部体积。
    fn gzip_with_size(size: u32) -> Vec<u8> {
        let mut bytes = vec![0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0x03];
        bytes.extend_from_slice(b"payload");
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes
    }

    /// 一个最小可解析 zip：本地头占位 + 中央目录 + EOCD。
    fn zip_with_sizes(sizes: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&[0; 22]);
        let directory_offset = bytes.len();
        for size in sizes {
            bytes.extend_from_slice(b"PK\x01\x02");
            // 版本、标志、方法、时间、日期与 CRC 共 16 字节。
            bytes.extend_from_slice(&[0; 16]);
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&size.to_le_bytes());
            // 名字长、扩展长、注释长、磁盘号、内外部属性与本地头偏移共 18 字节。
            bytes.extend_from_slice(&[0; 18]);
        }
        let directory_size = bytes.len() - directory_offset;
        bytes.extend_from_slice(b"PK\x05\x06");
        // 磁盘号、CD 起始磁盘、本盘条目数、总条目数各 2 字节。
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&(directory_size as u32).to_le_bytes());
        bytes.extend_from_slice(&(directory_offset as u32).to_le_bytes());
        bytes.extend_from_slice(&[0; 2]);
        bytes
    }

    #[test]
    fn common_types_pass_and_unknown_types_are_rejected() {
        assert!(check("photo.png", b"\x89PNG").is_ok());
        assert!(check("notes.md", b"# hi").is_ok());
        assert!(check("main.rs", b"fn main() {}").is_ok());
        assert!(check("archive.zip", &zip_with_sizes(&[16])).is_ok());

        let rejected = check("binary.exe", b"MZ").expect_err("可执行文件不在白名单内");
        assert_eq!(rejected.code(), "unsupported_file_type");
        assert!(rejected.message().contains("binary.exe"));

        let nameless = check("no-extension", b"data").expect_err("没有扩展名必须拒绝");
        assert_eq!(nameless.code(), "unsupported_file_type");
    }

    #[test]
    fn gzip_declared_expansion_is_read_from_the_trailer() {
        let small = gzip_with_size(1024);
        assert_eq!(gzip_expansion(&small), Some(1024));
        assert!(check("small.gz", &small).is_ok());
    }

    #[test]
    fn a_compression_bomb_is_rejected_before_it_lands() {
        // 声明解压 2 GiB，实际文件只有几十字节。
        let bomb = gzip_with_size(2048 * 1024 * 1024);
        let rejected = check("bomb.gz", &bomb).expect_err("解压体积超限必须拒绝");
        assert_eq!(rejected.code(), "archive_too_large");
        assert!(matches!(
            rejected,
            PolicyRejection::ExpansionTooLarge { .. }
        ));
    }

    #[test]
    fn zip_declared_expansion_is_summed_over_entries() {
        let bytes = zip_with_sizes(&[1024, 2048]);
        assert_eq!(zip_expansion(&bytes), Some(3072));
        assert!(check("bundle.zip", &bytes).is_ok());
    }

    #[test]
    fn an_unreadable_zip_is_rejected_instead_of_trusted() {
        // 只有本地头、没有中央目录：读不出解压体积，不能放行。
        let truncated = b"PK\x03\x04".to_vec();
        let rejected = check("broken.zip", &truncated).expect_err("解析不出体积必须拒绝");
        assert_eq!(rejected.code(), "archive_too_large");
        assert!(matches!(rejected, PolicyRejection::UnreadableArchive));
    }
}
