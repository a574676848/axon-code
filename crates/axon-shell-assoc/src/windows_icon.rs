//! Windows 图标提取：把 Shell 给出的图标位置变成 [`ICON_SIZE`] 见方的 PNG data URL。
//!
//! 与 DSH 参考实现（`file-applications-windows.ts` 的 `IconData` / `PngFromIconSource`）对齐：
//!
//! 1. `IAssocHandler::GetIconLocation` 取图标位置（文件路径 + 序号）；
//! 2. 路径里的 `%NAME%` 环境变量先展开：`SHDefExtractIcon` 不会自行展开；
//! 3. `@{包标识?ms-resource://...}` 形式的**间接引用**先交给 `SHLoadIndirectString` 解析；
//! 4. `SHDefExtractIconW` 抽图标，取不到时退回用候选标识（可执行文件）自身的图标；
//! 5. `GetIconInfo` + `GetDIBits` 把图标读成 RGBA，编码为 PNG。
//!
//! **已知缺口**：参考实现额外有一条「图标位置本身是 `.png` 图片文件」的分支（打包应用经
//! 第 3 步解析后常见）。本实现不引入 GDI+，这类位置会走 `SHDefExtractIconW` 而抽不出图，
//! 随后回退到候选标识自身的图标：结果是「有图标，但可能是可执行文件的图标」。
//!
//! # 为什么必须 unsafe
//!
//! 整条链路都是裸 Win32 句柄（HICON / HBITMAP / HDC）：获取、读取、销毁的顺序由调用方负责，
//! 没有安全封装。本文件的每个 `unsafe` 块都带 `// SAFETY:` 说明对应不变量；所有句柄都由
//! [`OwnedIcon`] / [`OwnedBitmap`] / [`OwnedDc`] 三个 RAII 守卫持有，因此**任何提前返回
//! 都不会漏释放**。

use ::windows::{
    Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS,
            DeleteDC, DeleteObject, GetDIBits, GetObjectW, HBITMAP, HDC,
        },
        UI::{
            Shell::{IAssocHandler, SHDefExtractIconW, SHLoadIndirectString},
            WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO},
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::{
    ICON_SIZE,
    windows::take_com_string,
    windows_helpers::{eq_ignore_case, expand_environment_variables, png_data_url, to_wide},
};

/// `@{...}` 间接引用解析结果的最大长度（UTF-16 码元）；参考实现同样取 1024。
const INDIRECT_ICON_UNITS: usize = 1024;

/// 取某条候选的图标 data URL；任何一步失败都返回 `None`。
///
/// 失败一律降级：图标只是装饰，取不到不应该影响这条候选能否被用来打开文件。
pub(crate) fn data_url(handler: &IAssocHandler, id: &str) -> Option<String> {
    if let Some((source, index)) = icon_location(handler) {
        if let Some(url) = from_source(&source, index) {
            return Some(url);
        }
        // 图标位置与候选标识指向同一个文件时不再重复抽取一次。
        if eq_ignore_case(&source, id) {
            return None;
        }
    }
    // 回退：用处理程序的可执行文件（打包应用则是 AUMID，通常抽不出图标）再试一次。
    from_source(id, 0)
}

/// 读处理程序登记的图标位置；不支持、返回空指针或空路径时返回 `None`。
fn icon_location(handler: &IAssocHandler) -> Option<(String, i32)> {
    let mut path = PWSTR::null();
    let mut index = 0_i32;
    // SAFETY: `handler` 由调用方保证是本线程上存活的有效接口指针；两个出参都是本栈帧上
    // 已初始化的局部变量，`GetIconLocation` 只写不读。成功时 `path` 是待接管的
    // CoTaskMem 字符串，失败时不产生任何需要释放的指针。
    unsafe { handler.GetIconLocation(&mut path, &mut index) }.ok()?;
    // SAFETY: 按 `GetIconLocation` 的契约，成功返回时 `path` 是 CoTaskMem 分配的宽字符串。
    let path = unsafe { take_com_string(path) }?;
    if path.is_empty() {
        return None;
    }
    Some((path, index))
}

/// 从一个图标位置（文件路径 + 序号）抽图并编码；失败返回 `None`。
fn from_source(source: &str, index: i32) -> Option<String> {
    let source = expand_environment_variables(source);
    if source.is_empty() {
        return None;
    }
    // 打包应用登记的图标是 `@{包标识?ms-resource://...}` 形式的间接引用，
    // `SHDefExtractIconW` 无法直接用它，必须先让 Shell 解析成真实资源路径。
    let source = resolve_indirect(&source).unwrap_or(source);
    let icon = pixels_from_file(&source, index)?;
    png_data_url(&icon.pixels, icon.width, icon.height)
}

/// 把 `@{...}` 形式的间接图标引用交给 Shell 解析；不是间接引用或解析失败时返回 `None`。
fn resolve_indirect(source: &str) -> Option<String> {
    if !source.starts_with("@{") || !source.ends_with('}') {
        return None;
    }
    let wide = to_wide(source);
    let mut buffer = vec![0_u16; INDIRECT_ICON_UNITS];
    // SAFETY: `wide` 是以 NUL 结尾的宽字符串、`buffer` 是 `INDIRECT_ICON_UNITS` 个 `u16`，
    // 两者都活到本次调用之后；`SHLoadIndirectString` 只读 `wide`、只写 `buffer`，并在
    // `buffer.len()` 处停止；第三个参数按约定为 null（保留，必须为空）。
    unsafe {
        SHLoadIndirectString(PCWSTR(wide.as_ptr()), &mut buffer, None).ok()?;
    }
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    let resolved = String::from_utf16_lossy(&buffer[..end]);
    (!resolved.is_empty()).then_some(resolved)
}

/// 用 `SHDefExtractIconW` 从文件抽图标，读成 RGBA 并统一缩放到 [`ICON_SIZE`]。
///
/// `SHDefExtractIconW` 返回的尺寸取决于文件里**实际存在**的图标，并不保证正好是请求的
/// 32 像素；因此这里统一缩放，兑现「图标是 [`ICON_SIZE`] 见方」的公开契约。
fn pixels_from_file(path: &str, index: i32) -> Option<IconPixels> {
    let wide = to_wide(path);
    let mut large = HICON::default();
    let mut small = HICON::default();
    // SAFETY: `wide` 是以 NUL 结尾的宽字符串且活到本函数结束；两个出参都是本栈帧上已初始化
    // 的局部变量。无论成功失败，可能被写入的图标句柄都由紧随其后的 `OwnedIcon` 接管并销毁。
    let result = unsafe {
        SHDefExtractIconW(
            PCWSTR(wide.as_ptr()),
            index,
            0,
            Some(&mut large),
            Some(&mut small),
            ICON_SIZE,
        )
    };
    let large = OwnedIcon::new(large);
    let small = OwnedIcon::new(small);
    if result.is_err() {
        // 失败路径同样要归还句柄：`large` / `small` 在这里析构。
        return None;
    }
    // 大图标优先；`or` 无论取哪一个，另一个都会在内部被丢弃（`OwnedIcon` 的 Drop 负责销毁）。
    let icon = large.or(small)?;
    pixels_from_icon(icon.handle())
}

/// 把一个 HICON 读成 RGBA 像素，并缩放到 [`ICON_SIZE`]。
fn pixels_from_icon(icon: HICON) -> Option<IconPixels> {
    let mut info = ICONINFO::default();
    // SAFETY: `icon` 由 `OwnedIcon` 保证是有效句柄；`info` 是本栈帧上已初始化的结构，
    // `GetIconInfo` 只写不读。成功时 `info` 里的两个位图句柄归调用方所有。
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    // 两个位图句柄无论后续哪一步失败都必须归还，因此先各自加上守卫再判断可用性。
    let color = OwnedBitmap::new(info.hbmColor);
    let mask = OwnedBitmap::new(info.hbmMask);
    let color = color?;
    let (width, height) = bitmap_size(color.handle())?;
    let color_pixels = bitmap_bgra(color.handle(), width, height)?;
    // 掩码读失败不是错误：32 位图标自带 alpha，掩码只是传统图标的兜底。
    let mask_pixels = mask.and_then(|mask| bitmap_bgra(mask.handle(), width, height));
    let rgba = compose_rgba(&color_pixels, mask_pixels.as_deref());
    resize_rgba(&rgba, width, height)
}

/// 读取位图的像素尺寸。
fn bitmap_size(bitmap: HBITMAP) -> Option<(u32, u32)> {
    let mut description = BITMAP::default();
    let size = i32::try_from(std::mem::size_of::<BITMAP>()).ok()?;
    // SAFETY: `bitmap` 是有效位图句柄（由 `OwnedBitmap` 保证）；`description` 是本栈帧上
    // 已初始化的 `BITMAP`，`size` 与其真实长度一致，`GetObjectW` 只写该缓冲。
    let written = unsafe {
        GetObjectW(
            bitmap.into(),
            size,
            Some(std::ptr::from_mut(&mut description).cast()),
        )
    };
    if written == 0 {
        return None;
    }
    let width = u32::try_from(description.bmWidth).ok()?;
    let height = u32::try_from(description.bmHeight).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

/// 用 `GetDIBits` 把位图读成 32 位自顶向下的 BGRA 缓冲（每像素 4 字节，行序为图像顺序）。
fn bitmap_bgra(bitmap: HBITMAP, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).ok()?;
    info.bmiHeader.biWidth = i32::try_from(width).ok()?;
    // 负高度表示「自顶向下」，于是第 0 行就是图像顶部，省掉一次整块翻转。
    info.bmiHeader.biHeight = -i32::try_from(height).ok()?;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let stride = (width as usize).checked_mul(4)?;
    let mut pixels = vec![0_u8; stride.checked_mul(height as usize)?];
    let dc = OwnedDc::new()?;
    // SAFETY: `dc` 是有效的兼容 DC；`bitmap` 是有效位图且未被选入任何 DC（`GetIconInfo` 归还的
    // 位图不在 DC 里）；`pixels` 的长度恰好是 `height` 行 × 每行 `stride` 字节，`GetDIBits`
    // 按 `info.bmiHeader` 描述的尺寸写入，不越界。
    let lines = unsafe {
        GetDIBits(
            dc.handle(),
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if lines != i32::try_from(height).ok()? {
        return None;
    }
    Some(pixels)
}

/// 把彩色位图与掩码位图合成为 RGBA。
///
/// 32 位图标自带 alpha 通道，直接用；只有 1 位掩码的传统图标在 32 位 DIB 里 alpha 全是 0，
/// 此时按掩码还原透明度（掩码为白表示该像素被挖空），否则整张图会变成不透明的黑块。
fn compose_rgba(color: &[u8], mask: Option<&[u8]>) -> Vec<u8> {
    // 两个缓冲都由 `GetDIBits` 按 32 位写出，长度必然是 4 的整数倍，尾部余数不存在。
    let (pixels, _) = color.as_chunks::<4>();
    let mask_pixels = mask.map(|mask| mask.as_chunks::<4>().0);
    let has_alpha = pixels.iter().any(|pixel| pixel[3] != 0);
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for (index, pixel) in pixels.iter().enumerate() {
        let alpha = if has_alpha {
            pixel[3]
        } else if mask_pixels.is_some_and(|mask| mask.get(index).is_some_and(|p| p[0] == u8::MAX)) {
            0
        } else {
            u8::MAX
        };
        // DIB 的 32 位像素是 BGRA，PNG 要 RGBA。
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], alpha]);
    }
    rgba
}

/// 把任意尺寸的 RGBA 缓冲缩放到 [`ICON_SIZE`] 见方。
///
/// 缩小按目标像素覆盖的源矩形做**alpha 加权平均**（预乘后再平均），避免缩小后细线条消失、
/// 以及透明像素的黑色 RGB 在边缘拖出黑边；放大时同一套算法退化为最近邻的加权版本。
fn resize_rgba(pixels: &[u8], width: u32, height: u32) -> Option<IconPixels> {
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if pixels.len() != expected || width == 0 || height == 0 {
        return None;
    }
    if width == ICON_SIZE && height == ICON_SIZE {
        return Some(IconPixels {
            width,
            height,
            pixels: pixels.to_vec(),
        });
    }
    let side = ICON_SIZE as usize;
    let source_stride = width as usize * 4;
    let mut scaled = vec![0_u8; side * side * 4];
    for target_y in 0..ICON_SIZE {
        // 目标像素覆盖的源行区间 [top, bottom)，至少覆盖一行。
        let top = target_y * height / ICON_SIZE;
        let bottom = ((target_y + 1) * height / ICON_SIZE)
            .max(top + 1)
            .min(height);
        for target_x in 0..ICON_SIZE {
            let left = target_x * width / ICON_SIZE;
            let right = ((target_x + 1) * width / ICON_SIZE)
                .max(left + 1)
                .min(width);
            let mut weighted = [0_u32; 3];
            let mut alpha_total = 0_u32;
            let mut count = 0_u32;
            for source_y in top..bottom {
                for source_x in left..right {
                    let offset = source_y as usize * source_stride + source_x as usize * 4;
                    let pixel = pixels.get(offset..offset + 4)?;
                    let alpha = u32::from(pixel[3]);
                    for (channel, value) in weighted.iter_mut().zip(pixel) {
                        *channel += u32::from(*value) * alpha;
                    }
                    alpha_total += alpha;
                    count += 1;
                }
            }
            if count == 0 {
                return None;
            }
            let offset = target_y as usize * side * 4 + target_x as usize * 4;
            let target = &mut scaled[offset..offset + 4];
            if alpha_total == 0 {
                // 整块全透明：RGB 已被预乘清零，写 0，避免把未定义颜色带进 PNG。
                target.fill(0);
                continue;
            }
            for (destination, channel) in target.iter_mut().zip(weighted) {
                *destination = u8::try_from(channel / alpha_total).ok()?;
            }
            target[3] = u8::try_from(alpha_total / count).ok()?;
        }
    }
    Some(IconPixels {
        width: ICON_SIZE,
        height: ICON_SIZE,
        pixels: scaled,
    })
}

/// 一次图标提取的像素结果。
struct IconPixels {
    /// 宽度（像素）。
    width: u32,
    /// 高度（像素）。
    height: u32,
    /// 自顶向下的 RGBA 字节流，长度为 `width * height * 4`。
    pixels: Vec<u8>,
}

/// `SHDefExtractIconW` 归还的图标句柄守卫；析构时 `DestroyIcon`。
struct OwnedIcon(HICON);

impl OwnedIcon {
    /// 接管一个可能为空的图标句柄；空句柄返回 `None`（调用方据此降级）。
    fn new(icon: HICON) -> Option<Self> {
        (!icon.is_invalid()).then_some(Self(icon))
    }

    /// 借用内部句柄，不转移所有权。
    fn handle(&self) -> HICON {
        self.0
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: 句柄由 `SHDefExtractIconW` 归还且由本守卫独占，`Drop` 只执行一次，
        // 因此不会重复销毁；`is_invalid` 已在构造时排除空与 -1。
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

/// `GetIconInfo` 归还的位图句柄守卫；析构时 `DeleteObject`。
struct OwnedBitmap(HBITMAP);

impl OwnedBitmap {
    /// 接管一个可能为空的位图句柄；空句柄返回 `None`（调用方据此降级）。
    fn new(bitmap: HBITMAP) -> Option<Self> {
        (!bitmap.0.is_null()).then_some(Self(bitmap))
    }

    /// 借用内部句柄，不转移所有权。
    fn handle(&self) -> HBITMAP {
        self.0
    }
}

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        // SAFETY: 句柄由 `GetIconInfo` 归还且由本守卫独占，`Drop` 只执行一次；
        // `DeleteObject` 接受任何 GDI 对象句柄，构造时已排除空句柄。
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

/// `CreateCompatibleDC` 归还的内存 DC 守卫；析构时 `DeleteDC`。
struct OwnedDc(HDC);

impl OwnedDc {
    /// 建一个与屏幕兼容的内存 DC；失败返回 `None`。
    fn new() -> Option<Self> {
        // SAFETY: 传 null 表示「与屏幕兼容的 DC」，是 `CreateCompatibleDC` 文档允许的取值；
        // 返回值在本函数内检查有效性。
        let dc = unsafe { CreateCompatibleDC(None) };
        (!dc.0.is_null()).then_some(Self(dc))
    }

    /// 借用内部句柄，不转移所有权。
    fn handle(&self) -> HDC {
        self.0
    }
}

impl Drop for OwnedDc {
    fn drop(&mut self) {
        // SAFETY: 句柄由 `CreateCompatibleDC` 归还且由本守卫独占，`Drop` 只执行一次；
        // 构造时已排除空句柄。
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}
