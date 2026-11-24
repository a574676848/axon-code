//! Windows 适配器中**不依赖任何 Win32 调用**的纯逻辑。
//!
//! 单独成模块是为了可验证性：扩展名提取、图标位置的环境变量展开、PNG data URL 拼装
//! 只吃字符串与字节，因此能在任意平台（含开发机上的 Windows 与 Linux CI）直接单测；
//! `windows.rs` / `windows_icon.rs` 里真正的 FFI 调用只能靠 `#[ignore]` 的冒烟测试
//! 在本机观察。

use std::path::Path;

/// PNG data URL 的前缀；`icon` 字段的契约就是「这个前缀 + base64」。
const DATA_URL_PREFIX: &str = "data:image/png;base64,";

/// Shell 关联查询使用的扩展名：**含前导点**、保留原始大小写。
///
/// `SHAssocEnumHandlers` 要求传入以点开头的扩展名（`.json`），`Path::extension` 却只给
/// `json`，这里补上点。返回 `None` 的情形一律按「该文件没有扩展名」处理，调用方据此
/// 返回空表：没有文件名、`README` 这类没有点的名字、`.gitignore` 这类整体是隐藏名的
/// 文件、以点结尾的 `report.`，以及扩展名本身不是合法 UTF-8 的路径。
pub(crate) fn shell_extension(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    if extension.is_empty() {
        return None;
    }
    Some(format!(".{extension}"))
}

/// 展开图标位置里的 `%NAME%` 环境变量引用。
///
/// Shell 登记的 `DefaultIcon` 常写成 `%SystemRoot%\system32\shell32.dll,-152`，而
/// `SHDefExtractIcon` **不会**自行展开百分号引用：不在这里展开，这类候选的图标会
/// 全部取不到。规则与 `ExpandEnvironmentStringsW` 对齐：变量未定义时**原样保留**
/// `%NAME%` 字面量（而不是替换成空串），单个不成对的 `%` 也原样保留。
///
/// 不含 `%` 的输入（绝大多数情况）直接原样返回，不做任何分配与扫描。
pub(crate) fn expand_environment_variables(source: &str) -> String {
    if !source.contains('%') {
        return source.to_owned();
    }
    let mut expanded = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            // 没有配对的结尾 `%`：整段当字面量。
            expanded.push_str(&rest[start..]);
            return expanded;
        };
        let name = &after[..end];
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => expanded.push_str(&value),
            _ => {
                expanded.push('%');
                expanded.push_str(name);
                expanded.push('%');
            }
        }
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    expanded
}

/// 把 [`crate::ICON_SIZE`] 见方的 RGBA 像素编码成 `data:image/png;base64,...`。
///
/// 尺寸为零、像素长度与 `width * height * 4` 不符、或 PNG 编码失败时返回 `None`：
/// 图标只是装饰，取不到不应该影响候选本身能否被打开。
pub(crate) fn png_data_url(pixels: &[u8], width: u32, height: u32) -> Option<String> {
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || pixels.len() != expected {
        return None;
    }
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(pixels).ok()?;
    // `finish` 消费 writer 并释放对 `encoded` 的借用，此后才能读取编码结果。
    writer.finish().ok()?;
    use base64::Engine as _;
    Some(format!(
        "{DATA_URL_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(&encoded)
    ))
}

/// 把 Rust 字符串转成以 NUL 结尾的 UTF-16 缓冲。
///
/// 调用方必须让返回值活到使用它的 FFI 调用结束：Win32 的宽字符串 API 只借用指针，
/// 不会复制内容。
pub(crate) fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 平台语义下的「同一个标识」比较：折叠大小写。
///
/// Windows 的文件路径与关联标识都不区分大小写（DSH 的参考实现用 `OrdinalIgnoreCase`）。
/// 这里用 `to_lowercase` 而不是 `eq_ignore_ascii_case`，以便与 [`crate::normalize_candidates`]
/// 的去重口径一致：非 ASCII 路径（`C:\工具\tool.exe`）的大小写差异同样会被折叠。
///
/// 只有 Windows 适配器用得上它：本模块在非 Windows 平台只为单测而编译，
/// 不给它加这一条限定，Linux 上就会多出一个「从未使用的函数」告警。
#[cfg(windows)]
pub(crate) fn eq_ignore_case(left: &str, right: &str) -> bool {
    left == right || left.to_lowercase() == right.to_lowercase()
}
