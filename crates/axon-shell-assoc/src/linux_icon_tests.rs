//! Linux 图标查找与编码的单元验证。
//!
//! 只吃路径与字节，因此在任何平台（含开发机上的 Windows 与 CI）都能**真验证**：
//! 用例自己搭一棵临时的 XDG 目录树，逐条核对「尺寸优先顺序、scalable 兜底、
//! pixmaps 兜底、先出现的目录为准」以及编码侧的格式与体积限制。

use std::path::{Path, PathBuf};

use super::linux_icon;

/// 建一棵一次性的临时目录树；由 `TestTempDir` 的守卫负责清理。
///
/// 用仓内共享的唯一目录而不是自拼时间戳：并行跑同一文件的多个用例时，
/// 时间戳的粒度可能撞在一起，两个用例会共用同一棵树而互相删掉对方的文件。
fn scratch() -> axon_test_support::TestTempDir {
    axon_test_support::test_temp_dir()
}

/// 往树里写一个文件，必要时先建好它的目录。
fn write(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("创建父目录");
    }
    std::fs::write(&path, bytes).expect("写入文件");
    path
}

/// 以 PNG 的魔数开头的一段字节；编码侧只看扩展名与体积，不看内容。
fn png_bytes(size: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.resize(size.max(bytes.len()), 7);
    bytes
}

#[test]
fn themed_lookup_prefers_the_larger_size_directory() {
    let root = scratch();
    let root = root.path().to_path_buf();
    let dirs = vec![root.clone()];
    // 同一张图标同时装在 32 与 512 里：必须先取 512，与既有工作台产品同一顺序。
    write(&root, "icons/hicolor/32x32/apps/tool.png", &png_bytes(16));
    let large = write(&root, "icons/hicolor/512x512/apps/tool.png", &png_bytes(16));
    assert_eq!(
        linux_icon::find_themed_icon(&dirs, "tool"),
        Some(large),
        "尺寸大的目录优先"
    );
}

#[test]
fn themed_lookup_falls_back_to_scalable_then_pixmaps() {
    let root = scratch();
    let root = root.path().to_path_buf();
    let dirs = vec![root.clone()];
    let scalable = write(&root, "icons/hicolor/scalable/apps/tool.svg", b"<svg/>");
    assert_eq!(
        linux_icon::find_themed_icon(&dirs, "tool"),
        Some(scalable),
        "没有位图时取矢量图"
    );
    std::fs::remove_file(root.join("icons/hicolor/scalable/apps/tool.svg")).expect("删掉矢量图");
    let pixmap = write(&root, "pixmaps/tool.png", &png_bytes(16));
    assert_eq!(
        linux_icon::find_themed_icon(&dirs, "tool"),
        Some(pixmap),
        "主题目录都没有时退回 pixmaps"
    );
    assert_eq!(
        linux_icon::find_themed_icon(&dirs, "nonesuch"),
        None,
        "找不到时如实返回 None，而不是猜一个路径"
    );
}

#[test]
fn themed_lookup_lets_the_earlier_data_directory_win() {
    let root = scratch();
    let root = root.path().to_path_buf();
    let user = root.join("home/.local/share");
    let system = root.join("usr/share");
    std::fs::create_dir_all(&user).expect("创建用户目录");
    std::fs::create_dir_all(&system).expect("创建系统目录");
    let preferred = write(&user, "icons/hicolor/128x128/apps/tool.png", &png_bytes(16));
    write(
        &system,
        "icons/hicolor/512x512/apps/tool.png",
        &png_bytes(16),
    );
    assert_eq!(
        linux_icon::find_themed_icon(&[user, system], "tool"),
        Some(preferred),
        "先出现的数据目录为准，即使它的图更小：这正是 XDG 的覆盖语义"
    );
}

#[test]
fn encoding_reports_the_mime_type_and_round_trips_the_bytes() {
    let root = scratch();
    let root = root.path().to_path_buf();
    let bytes = png_bytes(32);
    let path = write(&root, "tool.png", &bytes);
    let url = linux_icon::encode_icon(&path).expect("PNG 必须编码成功");
    let encoded = url
        .strip_prefix("data:image/png;base64,")
        .expect("必须是 PNG 的 data URL");
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64 必须可解"),
        bytes,
        "编码必须逐字节还原"
    );

    let svg = write(&root, "tool.svg", b"<svg/>");
    let url = linux_icon::encode_icon(&svg).expect("SVG 必须编码成功");
    assert!(
        url.starts_with("data:image/svg+xml;base64,"),
        "SVG 的 MIME 与 PNG 不同：{url}"
    );
}

#[test]
fn encoding_rejects_formats_and_sizes_it_cannot_serve() {
    let root = scratch();
    let root = root.path().to_path_buf();
    // 浏览器不一定渲染得出来的格式：给不出来好过给一个坏图标。
    let xpm = write(&root, "tool.xpm", b"/* XPM */");
    assert_eq!(linux_icon::encode_icon(&xpm), None);
    // 超过上限的图标会把一次 IPC 撑成几兆，宁可这一条没有图标。
    let huge = write(&root, "huge.png", &png_bytes(300 * 1024));
    assert_eq!(linux_icon::encode_icon(&huge), None);
    // 文件不存在时同样只是「没有图标」。
    assert_eq!(linux_icon::encode_icon(&root.join("absent.png")), None);
}

#[test]
fn icon_data_url_accepts_absolute_paths_and_ignores_empty_names() {
    let root = scratch();
    let root = root.path().to_path_buf();
    let path = write(&root, "absolute.png", &png_bytes(16));
    let url = linux_icon::icon_data_url(&[], Some(&path.to_string_lossy()));
    assert!(
        url.is_some_and(|value| value.starts_with("data:image/png;base64,")),
        "绝对路径必须直接读取"
    );
    assert_eq!(linux_icon::icon_data_url(&[], None), None);
    assert_eq!(linux_icon::icon_data_url(&[], Some("")), None);
}
