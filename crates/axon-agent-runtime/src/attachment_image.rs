//! 图片准入与归一化。
//!
//! 与 DSH 附件策略对齐：源图按字节、像素与长边准入，随后统一归一化为单帧、
//! 8-bit sRGB 的持久版本（应用 EXIF 方向、按像素预算与长边上限降采样、按质量
//! 阶梯编码到字节预算内），provider 请求阶段只会看到归一化后的字节。
//!
//! 归一化是确定性的：同一份源字节与同一份策略得到同一份输出，因此归档仍按内容
//! 寻址，重复读取不会产生新的 blob。

use std::io::Cursor;

use image::{
    DynamicImage, ExtendedColorType, GenericImageView, ImageDecoder, ImageEncoder, ImageReader,
    Limits, codecs::jpeg::JpegEncoder, codecs::png::PngEncoder, codecs::webp::WebPEncoder,
    imageops::FilterType, metadata::Orientation,
};

/// 单张源图字节上限：50 MiB（对齐 DSH `maxImageBytes`）。
pub const IMAGE_SOURCE_MAX_BYTES: u64 = 50 * 1024 * 1024;
/// 单张源图总像素上限：64,000,000（对齐 DSH `maxImagePixels`）。
pub const IMAGE_SOURCE_MAX_PIXELS: u64 = 64_000_000;
/// 单张源图长边上限：65,535（对齐 DSH `maxImageDimension`）。
pub const IMAGE_SOURCE_MAX_DIMENSION: u32 = 65_535;
/// 归一化后的字节预算：4 MiB（对齐 DSH `normalizedImageMaxBytes`）。
pub const IMAGE_NORMALIZED_MAX_BYTES: usize = 4 * 1024 * 1024;
/// 归一化后的总像素预算：2048 x 2048（对齐 DSH `normalizedImageMaxPixels`）。
pub const IMAGE_NORMALIZED_MAX_PIXELS: u64 = 2048 * 2048;
/// 归一化后的长边上限：8192（对齐 DSH `normalizedImageMaxDimension`）。
pub const IMAGE_NORMALIZED_MAX_DIMENSION: u32 = 8192;
/// 归一化编码的质量阶梯（对齐 DSH），从高到低寻找第一个落在字节预算内的版本。
pub const IMAGE_ENCODING_QUALITIES: [u8; 3] = [85, 75, 60];
/// 字节预算仍不满足时，最多再缩小几轮。
const IMAGE_SHRINK_ROUNDS: usize = 4;
/// 单轮缩小的长边比例。
const IMAGE_SHRINK_FACTOR: f64 = 0.75;
/// 解码期允许的最大内存分配（字节）。
const IMAGE_DECODE_MAX_ALLOC: u64 = 512 * 1024 * 1024;
/// GIF 源 MIME，仅用于判定不可原样通过。
pub const IMAGE_MIME_GIF: &str = "image/gif";
/// 归一化输出的 MIME 常量。
pub const IMAGE_MIME_PNG: &str = "image/png";
/// JPEG 归一化输出的 MIME。
pub const IMAGE_MIME_JPEG: &str = "image/jpeg";
/// WebP 归一化输出的 MIME。
pub const IMAGE_MIME_WEBP: &str = "image/webp";

/// 归一化完成后的图片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedImage {
    /// 归一化后的字节。
    pub bytes: Vec<u8>,
    /// 归一化后的 MIME。
    pub mime_type: &'static str,
    /// 归一化后的宽度。
    pub width: u32,
    /// 归一化后的高度。
    pub height: u32,
    /// 源图宽度。
    pub original_width: u32,
    /// 源图高度。
    pub original_height: u32,
    /// 是否发生了重新编码（false 表示源字节原样通过）。
    pub reencoded: bool,
}

impl NormalizedImage {
    /// 是否发生了降采样。
    #[must_use]
    pub fn downscaled(&self) -> bool {
        self.width != self.original_width || self.height != self.original_height
    }
}

/// 图片准入或归一化失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageNormalizeError {
    /// 源字节超过上限。
    SourceTooLarge {
        /// 实际字节数。
        actual: u64,
        /// 字节上限。
        limit: u64,
    },
    /// 源图总像素超过上限。
    PixelsExceeded {
        /// 宽度。
        width: u32,
        /// 高度。
        height: u32,
        /// 像素上限。
        limit: u64,
    },
    /// 源图长边超过上限。
    DimensionExceeded {
        /// 宽度。
        width: u32,
        /// 高度。
        height: u32,
        /// 长边上限。
        limit: u32,
    },
    /// 无法解码为受支持的图片。
    Decode(String),
    /// 无法编码归一化结果。
    Encode(String),
}

impl std::fmt::Display for ImageNormalizeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceTooLarge { actual, limit } => write!(
                formatter,
                "图片 {actual} 字节超过 {limit} 字节上限；请先缩小图片再读取"
            ),
            Self::PixelsExceeded {
                width,
                height,
                limit,
            } => write!(
                formatter,
                "图片 {width}x{height} 超过 {limit} 总像素上限；请先缩小图片再读取"
            ),
            Self::DimensionExceeded {
                width,
                height,
                limit,
            } => write!(
                formatter,
                "图片 {width}x{height} 超过 {limit} 长边上限；请先缩小图片再读取"
            ),
            Self::Decode(message) => write!(
                formatter,
                "图片无法解码为受支持的 PNG/JPEG/WebP/GIF：{message}"
            ),
            Self::Encode(message) => write!(formatter, "图片归一化编码失败：{message}"),
        }
    }
}

impl std::error::Error for ImageNormalizeError {}

/// 判断源字节是否可以原样通过归一化。
#[must_use]
pub fn can_pass_through(
    mime_type: &str,
    color: ExtendedColorType,
    orientation: Orientation,
    byte_len: usize,
    width: u32,
    height: u32,
) -> bool {
    mime_type != IMAGE_MIME_GIF
        && orientation == Orientation::NoTransforms
        && is_eight_bit(color)
        && byte_len <= IMAGE_NORMALIZED_MAX_BYTES
        && u64::from(width) * u64::from(height) <= IMAGE_NORMALIZED_MAX_PIXELS
        && width.max(height) <= IMAGE_NORMALIZED_MAX_DIMENSION
}

/// 把源字节归一化为单帧 8-bit sRGB 图片。
///
/// # Errors
///
/// 当源字节超过字节上限、像素上限或长边上限，或字节无法解码/重新编码时返回错误。
pub fn normalize_image(
    bytes: &[u8],
    source_mime: &str,
) -> Result<NormalizedImage, ImageNormalizeError> {
    let actual = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if actual > IMAGE_SOURCE_MAX_BYTES {
        return Err(ImageNormalizeError::SourceTooLarge {
            actual,
            limit: IMAGE_SOURCE_MAX_BYTES,
        });
    }
    // 先只读文件头：尺寸、原始色深与方向都来自解码器元信息，预算内的小图无需解码像素。
    let (decoder, original_width, original_height, original_color, orientation) = probe(bytes)?;
    let pixels = u64::from(original_width) * u64::from(original_height);
    if pixels > IMAGE_SOURCE_MAX_PIXELS {
        return Err(ImageNormalizeError::PixelsExceeded {
            width: original_width,
            height: original_height,
            limit: IMAGE_SOURCE_MAX_PIXELS,
        });
    }
    if original_width.max(original_height) > IMAGE_SOURCE_MAX_DIMENSION {
        return Err(ImageNormalizeError::DimensionExceeded {
            width: original_width,
            height: original_height,
            limit: IMAGE_SOURCE_MAX_DIMENSION,
        });
    }

    if can_pass_through(
        source_mime,
        original_color,
        orientation,
        bytes.len(),
        original_width,
        original_height,
    ) {
        return Ok(NormalizedImage {
            bytes: bytes.to_vec(),
            mime_type: static_mime(source_mime),
            width: original_width,
            height: original_height,
            original_width,
            original_height,
            reencoded: false,
        });
    }

    let image = DynamicImage::from_decoder(decoder)
        .map_err(|error| ImageNormalizeError::Decode(error.to_string()))?;
    let oriented = apply_orientation(image, orientation);
    let (mut target_width, mut target_height) = target_dimensions(original_width, original_height);
    let mut best: Option<NormalizedImage> = None;
    for _ in 0..=IMAGE_SHRINK_ROUNDS {
        let resized = if (target_width, target_height) == (original_width, original_height) {
            oriented.clone()
        } else {
            oriented.resize_exact(target_width, target_height, FilterType::Lanczos3)
        };
        let candidate = encode_candidate(&resized, original_width, original_height)?;
        let fits = candidate.bytes.len() <= IMAGE_NORMALIZED_MAX_BYTES;
        let smaller = best
            .as_ref()
            .is_none_or(|current| candidate.bytes.len() < current.bytes.len());
        if smaller {
            best = Some(candidate);
        }
        if fits {
            break;
        }
        let (next_width, next_height) = shrink(target_width, target_height);
        if (next_width, next_height) == (target_width, target_height) {
            break;
        }
        target_width = next_width;
        target_height = next_height;
    }

    best.ok_or_else(|| ImageNormalizeError::Encode("没有可用的编码结果".to_owned()))
}

/// 只读文件头得到的图片事实。
type ImageProbe<'a> = (
    Box<dyn image::ImageDecoder + 'a>,
    u32,
    u32,
    ExtendedColorType,
    Orientation,
);

fn probe<'a>(bytes: &'a [u8]) -> Result<ImageProbe<'a>, ImageNormalizeError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ImageNormalizeError::Decode(error.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(IMAGE_SOURCE_MAX_DIMENSION);
    limits.max_image_height = Some(IMAGE_SOURCE_MAX_DIMENSION);
    limits.max_alloc = Some(IMAGE_DECODE_MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| ImageNormalizeError::Decode(error.to_string()))?;
    let (width, height) = decoder.dimensions();
    let color = decoder.original_color_type();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    Ok((Box::new(decoder), width, height, color, orientation))
}

fn apply_orientation(mut image: DynamicImage, orientation: Orientation) -> DynamicImage {
    if orientation != Orientation::NoTransforms {
        image.apply_orientation(orientation);
    }
    image
}

/// 先按总像素预算，再按长边上限收敛目标尺寸，保持宽高比且不放大。
fn target_dimensions(width: u32, height: u32) -> (u32, u32) {
    let pixels = u64::from(width) * u64::from(height);
    let mut target_width = width;
    let mut target_height = height;
    if pixels > IMAGE_NORMALIZED_MAX_PIXELS {
        let scale = (IMAGE_NORMALIZED_MAX_PIXELS as f64 / pixels as f64).sqrt();
        target_width = scaled_edge(width, scale);
        target_height = scaled_edge(height, scale);
    }
    let long_edge = target_width.max(target_height);
    if long_edge > IMAGE_NORMALIZED_MAX_DIMENSION {
        let scale = f64::from(IMAGE_NORMALIZED_MAX_DIMENSION) / f64::from(long_edge);
        target_width = scaled_edge(target_width, scale);
        target_height = scaled_edge(target_height, scale);
    }
    (target_width.max(1), target_height.max(1))
}

fn shrink(width: u32, height: u32) -> (u32, u32) {
    (
        scaled_edge(width, IMAGE_SHRINK_FACTOR).max(1),
        scaled_edge(height, IMAGE_SHRINK_FACTOR).max(1),
    )
}

fn scaled_edge(edge: u32, scale: f64) -> u32 {
    let scaled = (f64::from(edge) * scale).floor();
    if scaled < 1.0 {
        return 1;
    }
    u32::try_from(scaled as u64).unwrap_or(u32::MAX)
}

fn encode_candidate(
    image: &DynamicImage,
    original_width: u32,
    original_height: u32,
) -> Result<NormalizedImage, ImageNormalizeError> {
    let (width, height) = image.dimensions();
    let has_alpha = image.color().has_alpha();
    let mut best: Option<(Vec<u8>, &'static str)> = None;
    if has_alpha {
        // 有 alpha 时不走 JPEG：透明通道会被压掉。先试无损 WebP，再退 PNG。
        let rgba = image.to_rgba8();
        for mime_type in [IMAGE_MIME_WEBP, IMAGE_MIME_PNG] {
            let mut buffer = Vec::new();
            let encoded = match mime_type {
                IMAGE_MIME_WEBP => WebPEncoder::new_lossless(&mut Cursor::new(&mut buffer))
                    .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8),
                _ => PngEncoder::new(&mut Cursor::new(&mut buffer)).write_image(
                    rgba.as_raw(),
                    width,
                    height,
                    ExtendedColorType::Rgba8,
                ),
            };
            encoded.map_err(|error| ImageNormalizeError::Encode(error.to_string()))?;
            let fits = buffer.len() <= IMAGE_NORMALIZED_MAX_BYTES;
            let smaller = best
                .as_ref()
                .is_none_or(|(current, _)| buffer.len() < current.len());
            if smaller {
                best = Some((buffer, mime_type));
            }
            if fits {
                break;
            }
        }
    } else {
        let rgb = image.to_rgb8();
        for quality in IMAGE_ENCODING_QUALITIES {
            let mut buffer = Vec::new();
            JpegEncoder::new_with_quality(&mut Cursor::new(&mut buffer), quality)
                .encode_image(&rgb)
                .map_err(|error| ImageNormalizeError::Encode(error.to_string()))?;
            let fits = buffer.len() <= IMAGE_NORMALIZED_MAX_BYTES;
            let smaller = best
                .as_ref()
                .is_none_or(|(current, _)| buffer.len() < current.len());
            if smaller {
                best = Some((buffer, IMAGE_MIME_JPEG));
            }
            if fits {
                break;
            }
        }
    }
    let (bytes, mime_type) =
        best.ok_or_else(|| ImageNormalizeError::Encode("没有可用的编码结果".to_owned()))?;
    Ok(NormalizedImage {
        bytes,
        mime_type,
        width,
        height,
        original_width,
        original_height,
        reencoded: true,
    })
}

fn is_eight_bit(color: ExtendedColorType) -> bool {
    matches!(
        color,
        ExtendedColorType::L8
            | ExtendedColorType::La8
            | ExtendedColorType::Rgb8
            | ExtendedColorType::Rgba8
    )
}

fn static_mime(mime_type: &str) -> &'static str {
    match mime_type {
        IMAGE_MIME_JPEG => IMAGE_MIME_JPEG,
        IMAGE_MIME_WEBP => IMAGE_MIME_WEBP,
        _ => IMAGE_MIME_PNG,
    }
}

#[cfg(test)]
mod tests {
    use image::{ExtendedColorType, Rgb, RgbImage, Rgba, RgbaImage, codecs::png::PngEncoder};

    use super::*;

    #[test]
    fn small_clean_png_passes_through_byte_identically() {
        let bytes = encode_png_rgb(&gradient_rgb(64, 48));

        let normalized = normalize_image(&bytes, IMAGE_MIME_PNG).expect("小图应可归一化");

        assert!(!normalized.reencoded, "预算内的小图必须原样通过");
        assert_eq!(normalized.bytes, bytes);
        assert_eq!((normalized.width, normalized.height), (64, 48));
        assert!(!normalized.downscaled());
    }

    #[test]
    fn large_photo_is_downscaled_within_pixel_budget_and_byte_budget() {
        let bytes = encode_png_rgb(&gradient_rgb(3000, 2000));

        let normalized = normalize_image(&bytes, IMAGE_MIME_PNG).expect("大图应可归一化");

        assert!(normalized.reencoded);
        let pixels = u64::from(normalized.width) * u64::from(normalized.height);
        assert!(pixels <= IMAGE_NORMALIZED_MAX_PIXELS, "超出像素预算");
        assert!(normalized.width.max(normalized.height) <= IMAGE_NORMALIZED_MAX_DIMENSION);
        assert!(
            normalized.bytes.len() <= IMAGE_NORMALIZED_MAX_BYTES,
            "超出字节预算"
        );
        assert_eq!(
            (normalized.original_width, normalized.original_height),
            (3000, 2000)
        );
        assert!(normalized.downscaled());
        assert_eq!(normalized.mime_type, IMAGE_MIME_JPEG);
    }

    #[test]
    fn sixteen_bit_png_is_normalized_to_eight_bit_output() {
        let bytes = encode_png_rgb16(40, 30);

        let normalized = normalize_image(&bytes, IMAGE_MIME_PNG).expect("16-bit PNG 应可转换");

        assert!(normalized.reencoded);
        let decoded = image::load_from_memory(&normalized.bytes).expect("输出必须可解码");
        assert!(
            is_eight_bit(decoded.color().into()),
            "输出必须是 8-bit：{:?}",
            decoded.color()
        );
        assert_eq!((normalized.width, normalized.height), (40, 30));
    }

    #[test]
    fn alpha_image_keeps_an_alpha_capable_encoding() {
        let bytes = encode_png_rgba(&noisy_rgba(2600, 2600));

        let normalized =
            normalize_image(&bytes, IMAGE_MIME_PNG).expect("带 alpha 的大图应可归一化");

        assert!(normalized.reencoded, "超过像素预算的图必须重新编码");
        assert!(
            matches!(normalized.mime_type, IMAGE_MIME_WEBP | IMAGE_MIME_PNG),
            "alpha 必须保留，不得编码成 JPEG：{}",
            normalized.mime_type
        );
        let decoded = image::load_from_memory(&normalized.bytes).expect("输出必须可解码");
        assert!(decoded.color().has_alpha());
    }

    #[test]
    fn gif_is_never_passed_through() {
        let gif = encode_gif(32, 32);

        let normalized = normalize_image(&gif, "image/gif").expect("GIF 应可归一化");

        assert!(normalized.reencoded, "GIF 必须重新编码为单帧");
        assert_ne!(normalized.mime_type, "image/gif");
    }

    #[test]
    fn source_over_byte_limit_is_refused_before_decoding() {
        let oversized = vec![0_u8; usize::try_from(IMAGE_SOURCE_MAX_BYTES).unwrap() + 1];

        let error = normalize_image(&oversized, IMAGE_MIME_PNG).expect_err("超限必须拒绝");

        assert!(matches!(error, ImageNormalizeError::SourceTooLarge { .. }));
    }

    #[test]
    fn undecodable_bytes_are_refused_as_decode_errors() {
        let error = normalize_image(&[0x89, b'P', b'N', b'G'], IMAGE_MIME_PNG)
            .expect_err("截断的 PNG 必须拒绝");

        assert!(matches!(error, ImageNormalizeError::Decode(_)));
    }

    #[test]
    fn target_dimensions_respect_pixel_budget_then_long_edge() {
        // 像素预算先于长边上限生效：2400 万像素的 3:2 图按比例收敛到 419 万像素。
        let (width, height) = target_dimensions(6000, 4000);
        assert!(u64::from(width) * u64::from(height) <= IMAGE_NORMALIZED_MAX_PIXELS);
        assert!(width > height);
        // 长边上限只在像素预算仍宽裕时生效。
        assert_eq!(target_dimensions(20_000, 100), (8192, 40));
        assert_eq!(target_dimensions(100, 100), (100, 100));
    }

    fn gradient_rgb(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        })
    }

    fn noisy_rgba(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let seed = x.wrapping_mul(7_919).wrapping_add(y.wrapping_mul(104_729));
            Rgba([
                (seed % 256) as u8,
                (seed.rotate_left(8) % 256) as u8,
                (seed.rotate_left(16) % 256) as u8,
                (seed % 251) as u8,
            ])
        })
    }

    fn encode_png_rgb(image: &RgbImage) -> Vec<u8> {
        let mut buffer = Vec::new();
        PngEncoder::new(&mut Cursor::new(&mut buffer))
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgb8,
            )
            .expect("PNG 编码应成功");
        buffer
    }

    fn encode_png_rgba(image: &RgbaImage) -> Vec<u8> {
        let mut buffer = Vec::new();
        PngEncoder::new(&mut Cursor::new(&mut buffer))
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgba8,
            )
            .expect("PNG 编码应成功");
        buffer
    }

    fn encode_png_rgb16(width: u32, height: u32) -> Vec<u8> {
        let mut buffer = Vec::new();
        PngEncoder::new(&mut Cursor::new(&mut buffer))
            .write_image(
                &vec![0x12_u8; usize::try_from(width * height * 6).unwrap()],
                width,
                height,
                ExtendedColorType::Rgb16,
            )
            .expect("16-bit PNG 编码应成功");
        buffer
    }

    fn encode_gif(width: u32, height: u32) -> Vec<u8> {
        let image = gradient_rgb(width, height);
        let mut buffer = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut Cursor::new(&mut buffer))
            .encode(image.as_raw(), width, height, ExtendedColorType::Rgb8)
            .expect("GIF 编码应成功");
        buffer
    }
}
