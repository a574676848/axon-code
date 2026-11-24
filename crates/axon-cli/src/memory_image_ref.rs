//! 多模态 Memory 检索的图片来源归一化。
//!
//! https URL 与 data:image base64 原样透传，本地图片按扩展名编码成 data URL；
//! 由 `memory search --image` 与本轮自动召回共用，保持单一事实来源。

use std::{
    fs,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};

/// 单张图片允许的最大字节数；超限直接拒绝，避免 base64 撑大请求体。
pub(crate) const MEMORY_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// 按扩展名判定图片 MIME；非图片返回 None。
pub(crate) fn image_mime_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

/// 归一化单张图片引用：https/data URL 透传，本地路径转 data URL。
pub(crate) fn memory_image_ref(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("图片引用不能为空".to_owned());
    }
    if input.starts_with("https://") || input.starts_with("data:image/") {
        return Ok(input.to_owned());
    }
    let path = Path::new(input);
    let mime = image_mime_type(path).ok_or_else(|| format!("不支持的图片类型：{input}"))?;
    let metadata = fs::metadata(path).map_err(|error| format!("读取失败 {input}: {error}"))?;
    if metadata.len() > MEMORY_IMAGE_MAX_BYTES {
        return Err(format!(
            "图片超过 {} MiB 上限：{input}",
            MEMORY_IMAGE_MAX_BYTES / 1024 / 1024
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("读取失败 {input}: {error}"))?;
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

/// 归一化一组图片引用，保持输入顺序。
pub(crate) fn memory_image_refs(inputs: &[String]) -> Result<Vec<String>, String> {
    inputs.iter().map(|input| memory_image_ref(input)).collect()
}

/// 本轮附件里的第一张图片 → 多模态 embedding 引用。
///
/// 多模态 embedding 只消费一张图，这里只编码首个可读图片；非图片附件与读取失败一律跳过，
/// 召回不因附件异常而失败。
pub(crate) fn first_attachment_image_ref(files: &[PathBuf]) -> Option<String> {
    for path in files.iter().map(PathBuf::as_path) {
        if image_mime_type(path).is_none() {
            continue;
        }
        match memory_image_ref(&path.to_string_lossy()) {
            Ok(image_ref) => return Some(image_ref),
            Err(error) => axon_logger::warning(&format!("Memory 召回跳过附件图片：{error}")),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_test_support::test_temp_dir;

    #[test]
    fn passthrough_accepts_https_and_data_url() {
        let refs = memory_image_refs(&[
            " https://example.com/a.png ".to_owned(),
            "data:image/png;base64,AAAA".to_owned(),
        ])
        .expect("透传应当成功");
        assert_eq!(
            refs,
            vec![
                "https://example.com/a.png".to_owned(),
                "data:image/png;base64,AAAA".to_owned(),
            ]
        );
    }

    #[test]
    fn local_image_becomes_data_url_with_matched_mime() {
        let dir = test_temp_dir();
        let path = dir.path().join("shot.WebP");
        fs::write(&path, b"webp-bytes").expect("写入测试图片");
        let image_ref = memory_image_ref(&path.to_string_lossy()).expect("本地图片应转 data URL");
        assert_eq!(image_ref, "data:image/webp;base64,d2VicC1ieXRlcw==");
    }

    #[test]
    fn rejects_missing_file_and_unsupported_extension() {
        let missing = memory_image_ref("C:/no-such-dir/missing.png").expect_err("缺失图片应报错");
        assert!(missing.contains("读取失败"), "{missing}");
        let unsupported = memory_image_ref("C:/tmp/notes.txt").expect_err("非图片扩展名应报错");
        assert!(unsupported.contains("不支持的图片类型"), "{unsupported}");
    }

    #[test]
    fn first_attachment_image_ref_skips_non_images_and_missing_files() {
        let dir = test_temp_dir();
        let note = dir.path().join("notes.txt");
        fs::write(&note, b"text").expect("写入测试文件");
        let missing = dir.path().join("gone.png");
        let image = dir.path().join("kept.png");
        fs::write(&image, b"png-bytes").expect("写入测试图片");
        let picked = first_attachment_image_ref(&[note, missing, image]).expect("应挑出可用图片");
        assert_eq!(picked, "data:image/png;base64,cG5nLWJ5dGVz");
    }
}
